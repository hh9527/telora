//! Source execution through Wasm. The frontend stops at SealedExecutable.
pub(crate) mod run;
pub(crate) mod testing;
mod test_fixtures;
mod diagnostics;
mod eval_contract;
mod timing;
mod runtime_sources;
use crate::static_input::Inventory;
use std::path::PathBuf;
use telora_core::{
    Diagnostic,
    mir::{ModuleTarget, SealedExecutable, SealedMir},
    source::Severity,
};
use timing::PhaseTimer;

pub(crate) fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        message: message.into(),
        labels: vec![],
        notes: vec![],
    }
}

fn compile(executable: &SealedExecutable<'_>) -> Result<telora_wasm::session::Session, String> {
    let bytes = {
        let _timer = PhaseTimer::new("codegen_link");
        telora_wasm::compile_executable(executable)?
    };
    let _timer = PhaseTimer::new("engine_load");
    // Use the engine's stopping boundary, including linked Rust library work.
    // No conversion to Telora operations or allocation costs is required.
    telora_wasm::session::Session::load(
        &bytes,
        crate::execution_config().fuel,
    )
}

pub(crate) fn compile_check(
    sealed: SealedMir<'_>,
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
    compile(&executable)
}

pub(crate) fn initialize(
    session: &mut telora_wasm::session::Session,
    inventory: &Inventory,
    sources: &mut telora_core::SourceDatabase,
) -> Result<(), String> {
    initialize_diagnostics(session, inventory, sources).map_err(|diagnostics| {
        diagnostics.iter().map(|d| sources.render(d)).collect::<Vec<_>>().join("\n")
    })
}

pub(crate) fn initialize_diagnostics(
    session: &mut telora_wasm::session::Session,
    inventory: &Inventory,
    sources: &mut telora_core::SourceDatabase,
) -> Result<(), Vec<Diagnostic>> {
    let timer = PhaseTimer::new("data_input");
    session.set_debug_enabled(true).map_err(|e| vec![error(e)])?;
    let mut diagnostics = vec![];
    let mut prepared = vec![];
    for module in session.manifest.data_modules.clone() {
        let (format, text) = match inventory.read_data_text(
            &module.name,
            crate::execution_config().data_limits.file_size,
        ) {
            Ok(data) => data,
            Err(message) => { diagnostics.push(error(message)); continue; }
        };
        let source = sources
            .try_add(module.name, &text)
            .map_err(|e| vec![error(e.to_string())])?;
        let plan = match telora_core::data_plan::parse_registered(sources, source, format) {
            Ok(plan) => plan,
            Err(errors) => { diagnostics.extend(errors); continue; }
        };
        if let Err(message) = telora_core::data_plan::enforce_limits(
            &plan,
            crate::execution_config().data_limits,
            text.len(),
        ) { diagnostics.push(error(message)); continue; }
        prepared.push((module.symbol, plan));
    }
    if !diagnostics.is_empty() { return Err(diagnostics); }
    for (symbol, plan) in prepared {
        session.register_data_sources(sources, &plan).map_err(|e| vec![error(e)])?;
        session.inject_data(symbol, &plan).map_err(|e| vec![error(e)])?;
    }
    drop(timer);
    let _timer = PhaseTimer::new("initialize");
    let result = session.initialize();
    if result.is_err() { Err(check_diagnostics(session, sources, result)) } else { Ok(()) }
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
    eval_contract::validate(&mir, symbol, false)?;
    let executable = sealed.seal_export(symbol).map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    drop(timer);
    let mut session = compile(&executable)?;
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

pub(crate) fn eval_with(
    context: PathBuf,
    module: &str,
    export: &str,
    inputs: Vec<crate::source_arg::NamedSource>,
    args: Vec<String>,
) -> Result<i32, String> {
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
        return Err("unresolved Wasm eval-with module".into());
    };
    let symbol = *mir.exports[module.index()]
        .iter()
        .find(|id| mir.symbols[id.index()].name == export)
        .ok_or_else(|| format!("module has no export {export:?}"))?;
    eval_contract::validate(&mir, symbol, true)?;
    let executable = sealed.seal_export(symbol).map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    drop(timer);
    let mut session = compile(&executable)?;
    if session.manifest.eval_type != Some(session.manifest.entry_type) {
        return Err("eval-with export: expected Eval (std/entry.Eval)".into());
    }
    let result = initialize(&mut session, &inventory, &mut mir.sources);
    diagnostics::finish(&session, &mir.sources, 0, result)?;
    execute_with(&mut session, &mut mir.sources, inputs, args)
}

pub(super) fn execute_with(
    session: &mut telora_wasm::session::Session,
    sources: &mut telora_core::SourceDatabase,
    inputs: Vec<crate::source_arg::NamedSource>,
    args: Vec<String>,
) -> Result<i32, String> {
    let timer = PhaseTimer::new("entry_input");
    let before = session.diagnostics()?.len();
    let config = session.eval_config()?;
    let names = |field: &str| -> Result<Vec<String>, String> {
        let mut names = config
            .get(field)
            .and_then(|v| v.as_array())
            .ok_or("Wasm: invalid entry config")?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or("Wasm: entry config name must be String")
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        if names.iter().any(String::is_empty) || names.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(format!(
                "entry.Eval.config.{field} must contain unique non-empty names"
            ));
        }
        Ok(names)
    };
    let declared_sources = names("sources")?;
    let provided = crate::source_arg::eval_source_names(&inputs)?;
    if declared_sources != provided {
        return Err(format!(
            "eval sources do not match entry.Eval config: declared {declared_sources:?}, provided {provided:?}"
        ));
    }
    if config.get("args").and_then(|v| v.as_bool()) != Some(true) && !args.is_empty() {
        return Err("entry.Eval config does not accept command-line arguments".into());
    }
    let env = names("envs")?
        .into_iter()
        .map(|name| {
            std::env::var(&name)
                .map(|text| (name.clone(), text))
                .map_err(|_| format!("cannot read declared environment variable {name:?}"))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    let inputs = crate::source_arg::collect_eval_sources(
        inputs,
        crate::execution_config().data_limits.file_size,
    )?;
    let mut plans = vec![];
    for (name, input) in inputs {
        let source = sources
            .try_add(input.source_name, &input.text)
            .map_err(|e| e.to_string())?;
        let format = match input.format {
            telora_core::SystemDataFormat::Json => telora_core::data_plan::Format::Json,
            telora_core::SystemDataFormat::Yaml => telora_core::data_plan::Format::Yaml,
            telora_core::SystemDataFormat::Toml => telora_core::data_plan::Format::Toml,
        };
        let plan =
            telora_core::data_plan::parse_registered(sources, source, format).map_err(|ds| {
                ds.iter()
                    .map(|d| sources.render(d))
                    .collect::<Vec<_>>()
                    .join("\n")
            })?;
        telora_core::data_plan::enforce_limits(
            &plan,
            crate::execution_config().data_limits,
            input.text.len(),
        )?;
        session.register_data_sources(sources, &plan)?;
        plans.push((name, plan));
    }
    drop(timer);
    let result = {
        let _timer = PhaseTimer::new("entry_output");
        session.eval_with(&args, &env, &plans)
    };
    println!(
        "{}",
        diagnostics::finish(session, sources, before, result)?
    );
    Ok(0)
}
