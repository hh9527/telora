use crate::source_arg::{NamedSource, collect_eval_sources, eval_source_names, parse_named_source};
use clap::Args;
use std::collections::BTreeMap;
use std::path::PathBuf;
use telora::package_host;

#[derive(Clone)]
struct EvalSelector {
    module_id: String,
    export: String,
}

#[derive(Args)]
pub(crate) struct EvalArgs {
    #[arg(value_name = "MODULE:NAME", value_parser = parse_eval_selector)]
    selector: EvalSelector,
}

#[derive(Args)]
pub(crate) struct EvalWithArgs {
    #[arg(value_name = "MODULE:NAME", value_parser = parse_eval_selector)]
    selector: EvalSelector,
    /// Provide a named Value source: NAME=PATH or NAME=(file|stdin)+(json|yaml|toml)://PATH.
    #[arg(long = "source", value_name = "NAME=SOURCE", value_parser = parse_named_source)]
    sources: Vec<NamedSource>,
    #[arg(last = true, value_name = "ARG")]
    args: Vec<String>,
}

fn parse_eval_selector(value: &str) -> Result<EvalSelector, String> {
    let (module_id, export) = value
        .rsplit_once(':')
        .ok_or_else(|| "expected MODULE:NAME".to_owned())?;
    if module_id.is_empty() {
        return Err("eval module selector must not be empty".into());
    }
    let mut characters = export.chars();
    if !characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err("eval export name must be an identifier".into());
    }
    Ok(EvalSelector {
        module_id: module_id.to_owned(),
        export: export.to_owned(),
    })
}

pub(crate) fn run(context: PathBuf, arguments: EvalArgs) -> Result<i32, String> {
    use telora_core::mir::{ModuleTarget, ResolveState, TypeConstructor, TypeState};
    let mut inventory = crate::static_input::Inventory::new(
        &context,
        arguments.selector.module_id.starts_with("std/"),
    )?;
    let root = inventory.select(&arguments.selector.module_id)?;
    let mir = inventory.solve(&root);
    let render = |diagnostics: Vec<telora_core::Diagnostic>| {
        diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let sealed = mir.seal().map_err(|diagnostics| {
        mir.diagnostics
            .iter()
            .chain(&diagnostics)
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let ModuleTarget::Bound(module) = mir.roots[0] else {
        return Err("unresolved eval module".into());
    };
    let symbol = *mir.exports[module.index()]
        .iter()
        .find(|id| mir.symbols[id.index()].name == arguments.selector.export)
        .ok_or_else(|| format!("module has no export {:?}", arguments.selector.export))?;
    let ResolveState::Bound(target) = mir.symbols[symbol.index()].resolution else {
        return Err("unresolved eval export".into());
    };
    if !mir.symbol_generics[target.index()].is_empty() {
        return Err("eval export must not be polymorphic".into());
    }
    // Explicit CLI output contract, selected from authoritative exports.
    // This is not a type-name recognition rule in the solver.
    let value_type = mir
        .modules
        .iter()
        .position(|module| module.name == "std/value")
        .and_then(|module| {
            mir.exports[module]
                .iter()
                .find(|id| mir.symbols[id.index()].name == "Value")
        })
        .and_then(
            |id| match mir.ty_slots[mir.symbol_types[id.index()].index()] {
                TypeState::Known(meta)
                    if mir.types[meta.index()].constructor == TypeConstructor::Meta =>
                {
                    mir.types[meta.index()].arguments.first().copied()
                }
                _ => None,
            },
        )
        .ok_or("eval export must have type std/value.Value")?;
    if mir.ty_slots[mir.symbol_types[target.index()].index()] != TypeState::Known(value_type) {
        return Err("eval export must have type std/value.Value".into());
    }
    let artifact = telora_core::codegen::compile(sealed, symbol).map_err(&render)?;
    let linked = telora_core::execution_link::link_entry(artifact).map_err(&render)?;
    let mut vm =
        telora_core::Vm::new().with_debug_sink(std::sync::Arc::new(crate::StderrDebugSink));
    let result = vm
        .execute_linked(linked, crate::engine_config().session_quota)
        .map_err(|error| error.to_string())?;
    let output = result.to_json(value_type)?;
    println!("{output}");
    Ok(0)
}

pub(crate) fn run_with(context: PathBuf, arguments: EvalWithArgs) -> Result<i32, String> {
    let (engine, pending) = prepare(&context, &arguments.selector)?;
    let _ = eval_source_names(&arguments.sources)?;
    let env = std::env::vars().collect::<BTreeMap<_, _>>();
    let sources = collect_eval_sources(arguments.sources, engine.config().data_limits.file_size)?;
    let output = engine
        .eval_pending_export_with(
            pending,
            &arguments.selector.export,
            telora_core::EvalContext {
                sources,
                env,
                args: arguments.args,
            },
        )
        .map_err(|error| error.to_string())?;
    println!("{output}");
    Ok(0)
}

fn prepare(
    context: &PathBuf,
    selector: &EvalSelector,
) -> Result<(telora_core::Engine, telora_core::PendingModule), String> {
    let prepared = package_host::prepare(context)?;
    let engine = crate::engine();
    let pending = engine
        .prepare_module_id_in_workspace(prepared, context, &selector.module_id)
        .map_err(|error| error.to_string())?;
    Ok((engine, pending))
}
