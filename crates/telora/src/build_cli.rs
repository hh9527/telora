//! File publication is separate from source execution.
use clap::{Args, ValueEnum};
use std::{
    io::{Cursor, Write},
    path::PathBuf,
};
use telora_core::{mir::ModuleTarget, source::Severity};
use telora_wasm::{
    service_build::{ServiceBuild, SnapshotMode},
    transform_service::SourceReader,
};

#[derive(Args)]
pub struct BuildArgs {
    /// Experimental direct JavaScript backend.
    #[arg(long, value_enum, default_value = "wasm")]
    backend: Backend,
    #[arg(value_name = "MODULE")]
    module: String,
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Embed initialized state; runtime specializes code, full keeps all bodies.
    #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "runtime", require_equals = true)]
    snapshot: Option<SnapshotArg>,
    /// Execute a closed build-time stage and publish its runtime and information.
    #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "runtime", require_equals = true, conflicts_with_all = ["snapshot", "sources"])]
    staged: Option<SnapshotArg>,
    #[arg(long = "source", requires = "snapshot", value_name = "NAME=SOURCE", value_parser = crate::parse_named_source)]
    sources: Vec<crate::NamedSource>,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum Backend {
    Wasm,
    Js,
}

#[derive(Clone, Copy, ValueEnum)]
enum SnapshotArg {
    Runtime,
    Full,
}

// Build phases include cleanup and publication; diagnostics stay on stderr.
struct PhaseTimer {
    phase: &'static str,
    start: Option<std::time::Instant>,
}

impl PhaseTimer {
    fn new(phase: &'static str) -> Self {
        let enabled = crate::execution_config().report_usage
            || std::env::var_os("TELORA_WASM_TIMINGS").as_deref()
                == Some(std::ffi::OsStr::new("1"));
        Self {
            phase,
            start: enabled.then(std::time::Instant::now),
        }
    }
}

impl Drop for PhaseTimer {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            let elapsed = start.elapsed();
            report_phase(self.phase, elapsed);
        }
    }
}

fn report_phase(phase: &str, elapsed: std::time::Duration) {
    if crate::execution_config().report_usage
        || std::env::var_os("TELORA_WASM_TIMINGS").as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        let parent = match phase {
            "total" => None,
            "runtime_specialization" => Some("snapshot"),
            "load_metadata"
            | "load_engine"
            | "load_module"
            | "load_store"
            | "load_instance_start"
            | "load_exports" => Some("engine_load"),
            _ => Some("total"),
        };
        eprintln!(
            "{}",
            serde_json::json!({
                "schema": "telora.build.timing/v1", "record": "diagnostic",
                "parent_phase": parent,
                "severity": "info", "code": "build-phase-timing",
                "message": "Build phase elapsed time", "locs": [],
                "phase": phase, "elapsed_seconds": elapsed.as_secs_f64(),
            })
        );
    }
}

pub fn execute(context: PathBuf, args: BuildArgs) -> Result<i32, String> {
    let _total = PhaseTimer::new("total");
    let frontend = PhaseTimer::new("frontend");
    if args.backend == Backend::Js {
        return build_js(context, args);
    }
    if args.module.contains(':') {
        return Err("build expects a module exporting MainService".into());
    }
    let mut inventory =
        crate::static_input::Inventory::new(&context, args.module.starts_with("std/"))?;
    inventory.normalize_eol();
    let root = inventory.select(&args.module)?;
    let mut mir = inventory.solve_transform(&root)?;
    if mir
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        return Err(mir
            .diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let sealed = mir.seal().map_err(|ds| {
        ds.iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let ModuleTarget::Bound(root) = mir.roots[0] else {
        return Err("unresolved entry".into());
    };
    let symbol = *mir.exports[root.index()]
        .iter()
        .find(|s| mir.symbols[s.index()].name == "main")
        .ok_or("missing static service plan")?;
    let executable = sealed.seal_export(symbol).map_err(|ds| {
        ds.iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    drop(frontend);
    let codegen = PhaseTimer::new("codegen_link");
    let build = if args.staged.is_some() {
        ServiceBuild::compile_staged(&executable)?
    } else {
        ServiceBuild::compile(&executable)?
    };
    drop(codegen);
    let bundle = PhaseTimer::new("data_bundle");
    drop(executable);
    let config = crate::execution_config_for(inventory.runtime_options())?;
    let mut plans = vec![];
    for data in build.manifest()?.data_modules {
        let (format, text) = inventory.read_data_text(&data.name, config.data_limits.file_size)?;
        let source = mir
            .sources
            .try_add_data(data.name, text)
            .map_err(|e| e.to_string())?;
        plans.push((data.symbol, source, format));
    }
    let build = build.bundle_data(
        &mir.sources,
        &plans,
        config.buildtime_fuel,
        config.buildtime_memory_limit,
    )?;
    drop(plans);
    drop(mir);
    drop(inventory);
    drop(bundle);
    let bytes = if let Some(mode) = args.snapshot.or(args.staged) {
        let engine = PhaseTimer::new("engine_load");
        let loaded = build.load(
            config.buildtime_fuel,
            config.buildtime_memory_limit,
            config.request_fuel,
            &mut |phase, elapsed| report_phase(phase, elapsed),
        )?;
        drop(engine);
        // Source enumeration prepares the static Service definition and its entry.
        let prepare = PhaseTimer::new("service_prepare");
        let mut service = loaded.prepare()?;
        drop(prepare);
        if config.report_usage {
            let usage = service.usage();
            let gc = service.gc_stats()?;
            crate::emit_stderr(
                serde_json::json!({"schema":"telora.build.usage/v1", "record":"buildtime-usage",
                "fuel_budget":usage.fuel_budget,"fuel_consumed":usage.fuel_budget.saturating_sub(usage.fuel_remaining),
                "linear_memory_bytes":usage.memory_bytes,"memory_limit_bytes":usage.memory_limit,
                "managed_live_bytes":gc.managed_bytes,"managed_committed_bytes":gc.words_committed,
                "allocated_total_bytes":gc.allocated_total,"collections":gc.collections}),
            )?;
        }

        let initialize = PhaseTimer::new("service_initialize");
        let names = crate::source_arg::service_source_names(&args.sources)?;
        if names != service.sources() {
            return Err(format!(
                "service sources differ: declared {:?}, supplied {names:?}",
                service.sources()
            ));
        }
        crate::source_arg::reject_stdin_sources(&args.sources)?;
        let readers = crate::source_arg::service_source_readers(args.sources).map(|source| {
            let source = source?;
            let bytes = crate::static_input::read_limited(
                source.reader,
                config.data_limits.file_size,
                &source.name,
            )?;
            let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
            Ok(SourceReader {
                name: source.name,
                format: source.format,
                reader: Box::new(Cursor::new(
                    crate::static_input::normalize_lf(text).into_bytes(),
                )),
            })
        });
        let result = service.initialize_readers(readers, config.data_limits.file_size)?;
        for diagnostic in result.diagnostics.as_array().into_iter().flatten() {
            crate::emit_stderr(diagnostic.clone())?;
        }
        if !result.success {
            return Ok(1);
        }
        drop(initialize);
        let seal = PhaseTimer::new("service_seal");
        let prepared = service.seal(match mode {
            SnapshotArg::Runtime => SnapshotMode::Runtime,
            SnapshotArg::Full => SnapshotMode::Full,
        })?;
        drop(seal);
        let _snapshot = PhaseTimer::new("snapshot");
        let (bytes, report) =
            prepared.publish_observed(&mut |phase, elapsed| report_phase(phase, elapsed))?;
        if let Some(report) = report {
            if crate::execution_config().report_usage
                || std::env::var_os("TELORA_WASM_TIMINGS").as_deref()
                    == Some(std::ffi::OsStr::new("1"))
            {
                eprintln!(
                    "{}",
                    serde_json::to_string(&report).map_err(|e| e.to_string())?
                );
            }
        }
        bytes
    } else {
        build.bytes().to_vec()
    };
    let _output = PhaseTimer::new("artifact_output");
    let bytes = telora_wasm::publication::finish(
        &bytes,
        config.memory_limit,
        config.initialization_fuel,
        config.request_fuel,
    )?;
    // Publish only after compilation has succeeded.
    let parent = args
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.persist(&args.output).map_err(|e| e.to_string())?;
    Ok(0)
}

fn build_js(context: PathBuf, args: BuildArgs) -> Result<i32, String> {
    if args.snapshot.is_some() || !args.sources.is_empty() {
        return Err("JS backend supports --staged, but not snapshot/source CLI options".into());
    }
    let (module, export) = args
        .module
        .rsplit_once(':')
        .map(|(m, e)| (m, Some(e)))
        .unwrap_or((&args.module, None));
    let mut inventory = crate::static_input::Inventory::new(&context, module.starts_with("std/"))?;
    inventory.normalize_eol();
    let root = inventory.select(module)?;
    let mir = if export.is_some() {
        inventory.solve(&root)
    } else {
        inventory.solve_transform(&root)?
    };
    if mir
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        return Err(mir
            .diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let ModuleTarget::Bound(root) = mir.roots[0] else {
        return Err("unresolved JS entry".into());
    };
    let symbol = *mir.exports[root.index()]
        .iter()
        .find(|s| mir.symbols[s.index()].name == export.unwrap_or("main"))
        .ok_or("missing JS export")?;
    let executable = mir.seal_export(symbol).map_err(|ds| {
        ds.iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let js = if args.staged.is_some() {
        let source = telora_js::compile_staging(&executable)?;
        let config = crate::execution_config_for(inventory.runtime_options())?;
        build_staged_js(
            &source,
            export.is_none(),
            config.buildtime_memory_limit,
            matches!(args.staged, Some(SnapshotArg::Full)),
        )?
    } else {
        telora_js::compile_executable(&executable)?
    };
    let parent = args
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(js.as_bytes()).map_err(|e| e.to_string())?;
    file.persist(&args.output).map_err(|e| e.to_string())?;
    Ok(0)
}

// A runs in an isolated cgroup; runtime JS only restores its closed result graph.
fn build_staged_js(
    source: &str,
    service: bool,
    memory: usize,
    full: bool,
) -> Result<String, String> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let temp_root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/tmp")))
        .ok_or("TMPDIR or HOME is required for JS build staging")?;
    std::fs::create_dir_all(&temp_root).map_err(|e| e.to_string())?;
    let dir = tempfile::Builder::new()
        .prefix("telora-js-stage-")
        .tempdir_in(temp_root)
        .map_err(|e| e.to_string())?;
    let model = dir.path().join("prepare-model.mjs");
    let script = dir.path().join("prepare.mjs");
    let image = dir.path().join("image.json");
    let errors = dir.path().join("errors.log");
    std::fs::write(&model, source).map_err(|e| e.to_string())?;
    std::fs::write(&script, telora_js::staging::PREPARE).map_err(|e| e.to_string())?;
    let unit = format!(
        "telora-js-build-{}-{}",
        std::process::id(),
        dir.path().file_name().unwrap().to_string_lossy()
    );
    let mut child = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            "--unit",
            &unit,
            "-p",
            &format!("MemoryMax={memory}"),
            "-p",
            "MemorySwapMax=0",
            "-p",
            "TasksMax=32",
            "--",
            "node",
        ])
        .arg(format!(
            "--max-old-space-size={}",
            (memory / 1024 / 1024 * 6 / 10).max(16)
        ))
        .args(["--max-semi-space-size=8"])
        .arg(&script)
        .arg(&model)
        .arg(&image)
        .arg(if service { "service" } else { "value" })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&errors).map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| format!("JS build process: {e}"))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                return Err(format!(
                    "JS build stage failed: {}",
                    std::fs::read_to_string(&errors).unwrap_or_default()
                ));
            }
            break;
        }
        if started.elapsed() > Duration::from_secs(60) {
            let _ = Command::new("systemctl")
                .args([
                    "--user",
                    "kill",
                    "--kill-whom=all",
                    &format!("{unit}.scope"),
                ])
                .status();
            let _ = child.kill();
            let _ = child.wait();
            return Err("JS build stage exceeded 60 second wall-clock budget".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let image = std::fs::read_to_string(image).map_err(|e| e.to_string())?;
    telora_js::staging::residual(source, &image, service, full)
}
