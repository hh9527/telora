//! Source execution through Wasm. The frontend stops at SealedExecutable.
mod diagnostics;
mod eval_contract;
pub(crate) mod run;
mod test_fixtures;
pub(crate) mod testing;
mod timing;
use crate::static_input::Inventory;
use std::path::PathBuf;
use telora_core::{
    Diagnostic,
    mir::{ModuleTarget, SealedExecutable, SealedMir},
    source::Severity,
};
use timing::PhaseTimer;

fn load_session(
    bytes: &[u8],
    runtime: telora_core::RuntimeOptions,
) -> Result<telora_wasm::session::Session, String> {
    timing::artifact_size(bytes.len());
    let config = crate::execution_config_for(runtime)?;
    let mut session =
        telora_wasm::session::Session::load_with_limits(bytes, config.fuel, config.memory_limit)?;
    if config.report_usage {
        session.usage_reporter = Some(|usage| {
            let _ = crate::emit_stderr(serde_json::json!({
                "schema": "telora.execution/v1", "record": "diagnostic",
                "severity": "info", "code": "execution-usage",
                "message": "Wasm execution resource usage", "labels": [], "notes": [],
                "usage": {
                    "fuel": {"limit": usage.fuel_budget,
                        "consumed": usage.fuel_budget.saturating_sub(usage.fuel_remaining),
                        "remaining": usage.fuel_remaining},
                    "linear_memory": {"bytes": usage.memory_bytes, "limit_bytes": usage.memory_limit}
                }
            }));
        });
    }
    Ok(session)
}

pub(crate) fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        message: message.into(),
        labels: vec![],
        notes: vec![],
    }
}

fn compile(
    executable: &SealedExecutable<'_>,
    runtime: telora_core::RuntimeOptions,
) -> Result<telora_wasm::session::Session, String> {
    let bytes = {
        let _timer = PhaseTimer::new("codegen_link");
        telora_wasm::compile_executable(executable)?
    };
    let _timer = PhaseTimer::new("engine_load");
    // Use the engine's stopping boundary, including linked Rust library work.
    // No conversion to Telora operations or allocation costs is required.
    load_session(&bytes, runtime)
}

pub(crate) fn compile_check(
    sealed: SealedMir<'_>,
    runtime: telora_core::RuntimeOptions,
) -> Result<telora_wasm::session::Session, String> {
    compile_modules(sealed, true, runtime)
}

pub(crate) fn compile_tests(
    sealed: SealedMir<'_>,
    runtime: telora_core::RuntimeOptions,
) -> Result<telora_wasm::session::Session, String> {
    compile_modules(sealed, false, runtime)
}

fn compile_modules(
    sealed: SealedMir<'_>,
    check: bool,
    runtime: telora_core::RuntimeOptions,
) -> Result<telora_wasm::session::Session, String> {
    let graph = sealed.mir();
    let modules = graph
        .hir
        .iter()
        .map(|node| node.module)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let executable = sealed.seal_modules(&modules).map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| graph.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    if !check {
        return compile(&executable, runtime);
    }
    let bytes = {
        let _timer = PhaseTimer::new("codegen_link");
        telora_wasm::compile_check(&executable)?
    };
    let _timer = PhaseTimer::new("engine_load");
    load_session(&bytes, runtime)
}

pub(crate) fn initialize(
    session: &mut telora_wasm::session::Session,
    inventory: &Inventory,
    sources: &mut telora_core::SourceDatabase,
) -> Result<(), String> {
    initialize_diagnostics(session, inventory, sources).map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

pub(crate) fn initialize_diagnostics(
    session: &mut telora_wasm::session::Session,
    inventory: &Inventory,
    sources: &mut telora_core::SourceDatabase,
) -> Result<(), Vec<Diagnostic>> {
    let timer = PhaseTimer::new("data_input");
    session
        .set_debug_enabled(true)
        .map_err(|e| vec![error(e)])?;
    let mut diagnostics = vec![];
    let mut prepared = vec![];
    for module in session.manifest.data_modules.clone() {
        let (format, text) = match inventory.read_data_text(
            &module.name,
            crate::execution_config().data_limits.file_size,
        ) {
            Ok(data) => data,
            Err(message) => {
                diagnostics.push(error(message));
                continue;
            }
        };
        let source = sources
            .try_add_data(module.name, text)
            .map_err(|e| vec![error(e.to_string())])?;
        let value = match session
            .parse_data_source(sources.get(source), format)
            .map_err(|e| vec![error(e)])?
        {
            Ok(value) => value,
            Err(errors) => {
                diagnostics
                    .extend(diagnostics::parsed(errors, sources).map_err(|e| vec![error(e)])?);
                continue;
            }
        };
        prepared.push((module.symbol, value));
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    for (symbol, value) in prepared {
        session
            .inject_data_value(symbol, value)
            .map_err(|e| vec![error(e)])?;
    }
    drop(timer);
    let _timer = PhaseTimer::new("initialize");
    let result = session.initialize();
    if result.is_ok() {
        timing::initialization_heap(session).map_err(|e| vec![error(e)])?;
    }
    if result.is_err() {
        Err(check_diagnostics(session, sources, result))
    } else {
        Ok(())
    }
}

pub(crate) fn check_diagnostics(
    session: &telora_wasm::session::Session,
    sources: &telora_core::SourceDatabase,
    result: Result<(), String>,
) -> Vec<Diagnostic> {
    let mut diagnostics = match diagnostics::collect(session, sources) {
        Ok(diagnostics) => diagnostics,
        Err(message) => vec![error(message)],
    };
    if let Err(message) = result {
        if !diagnostics.iter().any(|d| d.severity == Severity::Error) {
            diagnostics.push(error(message));
        }
    }
    diagnostics
}

pub(crate) fn eval(context: PathBuf, module: &str, export: &str) -> Result<i32, String> {
    let timer = PhaseTimer::new("frontend");
    let mut inventory = Inventory::new(&context, module.starts_with("std/"))?;
    let root = inventory.select(module)?;
    let mut mir = inventory.solve(&root);
    let sealed = mir.seal().map_err(|diagnostics| {
        mir.diagnostics
            .iter()
            .chain(&diagnostics)
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let ModuleTarget::Bound(module) = mir.roots[0] else {
        return Err("unresolved Wasm eval module".into());
    };
    let symbol = *mir.exports[module.index()]
        .iter()
        .find(|id| mir.symbols[id.index()].name == export)
        .ok_or_else(|| format!("module has no export {export:?}"))?;
    eval_contract::validate(&mir, symbol)?;
    let executable = sealed.seal_export(symbol).map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    drop(timer);
    let mut session = compile(&executable, inventory.runtime_options())?;
    if session.manifest.value_type != Some(session.manifest.entry_type) {
        return Err("eval export: expected Value (std/value.Value)".into());
    }
    let result = initialize(&mut session, &inventory, &mut mir.sources);
    diagnostics::finish(&session, &mir.sources, 0, result)?;
    let before = session.diagnostics()?.len();
    let result = {
        let _timer = PhaseTimer::new("export_output");
        session.eval()
    };
    println!(
        "{}",
        diagnostics::finish(&session, &mir.sources, before, result)?
    );
    Ok(0)
}
