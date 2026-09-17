//! File publication is separate from the existing source run/serve path.
use clap::Args;
use std::{
    io::{Cursor, Write},
    path::PathBuf,
};
use telora_core::{mir::ModuleTarget, source::Severity};
use telora_wasm::{
    artifact::Manifest,
    transform_service::{SourceReader, TransformSession},
};

#[derive(Args)]
pub struct BuildArgs {
    #[arg(value_name = "MODULE")]
    module: String,
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Run initialization and freeze the ready service into the output Wasm.
    #[arg(long)]
    snapshot: bool,
    #[arg(long = "source", requires = "snapshot", value_name = "NAME=SOURCE", value_parser = crate::parse_named_source)]
    sources: Vec<crate::NamedSource>,
}

pub fn execute(context: PathBuf, args: BuildArgs) -> Result<i32, String> {
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
    let bytes = telora_wasm::compile_service(&executable)?;
    let config = crate::execution_config_for(inventory.runtime_options())?;
    let mut plans = vec![];
    for data in Manifest::read(&bytes)?.data_modules {
        let (format, text) = inventory.read_data_text(&data.name, config.data_limits.file_size)?;
        let source = mir
            .sources
            .try_add_data(data.name, text)
            .map_err(|e| e.to_string())?;
        plans.push((data.symbol, source, format));
    }
    let bytes = telora_wasm::bundle::build(&bytes, &mir.sources, &plans)?;
    let bytes = if args.snapshot {
        let (context, instrumented) = telora_wasm::snapshot::instrument(&bytes)?;
        let session = telora_wasm::session::Session::load_with_limits(
            &instrumented,
            config.fuel,
            config.memory_limit,
        )?;
        let mut service = TransformSession::new(session)?;
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
            let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
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
        service.snapshot(&context)?
    } else {
        bytes
    };
    let bytes =
        telora_wasm::publication::finish(&bytes, args.snapshot, config.fuel, config.memory_limit)?;
    // Do not replace an existing artifact until all compilation/initialization has succeeded.
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
