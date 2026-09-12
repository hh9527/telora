use crate::source_arg::{NamedSource, collect_eval_sources, eval_source_names, parse_named_source};
use clap::Args;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone)]
struct EvalSelector {
    module_id: String,
    export: String,
}

#[derive(Args)]
pub(crate) struct EvalArgs {
    #[arg(long, hide = true)]
    native: bool,
    #[arg(value_name = "MODULE:NAME", value_parser = parse_eval_selector)]
    selector: EvalSelector,
}

#[derive(Args)]
pub(crate) struct EvalWithArgs {
    #[arg(long, hide = true)]
    native: bool,
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

fn prepare_solved(
    context: PathBuf,
    selector: &EvalSelector,
    with_context: bool,
) -> Result<
    (
        telora_core::execution_link::LinkedEntry,
        telora_core::mir::TypeId,
        telora_core::SourceDatabase,
    ),
    String,
> {
    use telora_core::mir::{ModuleTarget, ResolveState, TypeConstructor, TypeState};
    let mut inventory =
        crate::static_input::Inventory::new(&context, selector.module_id.starts_with("std/"))?;
    let root = inventory.select(&selector.module_id)?;
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
        .find(|id| mir.symbols[id.index()].name == selector.export)
        .ok_or_else(|| format!("module has no export {:?}", selector.export))?;
    let ResolveState::Bound(target) = mir.symbols[symbol.index()].resolution else {
        return Err("unresolved eval export".into());
    };
    if !mir.symbol_generics[target.index()].is_empty() {
        return Err("eval export must not be polymorphic".into());
    }
    // Explicit CLI output contract, selected from authoritative exports.
    // This is not a type-name recognition rule in the solver.
    let contract = |module_name: &str, export_name: &str| {
        mir.modules
            .iter()
            .position(|module| module.name == module_name)
            .and_then(|module| {
                mir.exports[module]
                    .iter()
                    .find(|id| mir.symbols[id.index()].name == export_name)
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
    };
    let expected_message = if with_context {
        "eval-with export: expected Eval (std/entry.Eval)"
    } else {
        "eval export: expected Value (std/value.Value)"
    };
    let value_type = contract("std/value", "Value").ok_or(expected_message)?;
    let expected = if with_context {
        contract("std/entry", "Eval").ok_or(expected_message)?
    } else {
        value_type
    };
    if mir.ty_slots[mir.symbol_types[target.index()].index()] != TypeState::Known(expected) {
        return Err(expected_message.into());
    }
    let artifact = if with_context {
        telora_core::codegen::compile_eval(sealed, symbol, value_type)
    } else {
        telora_core::codegen::compile(sealed, symbol)
    }
    .map_err(&render)?;
    let linked = telora_core::execution_link::link_entry_with_data(artifact, |link| {
        inventory.read_data(link, crate::execution_config().data_limits.file_size)
    })
    .map_err(&render)?;
    Ok((linked, value_type, mir.sources))
}

pub(crate) fn run(context: PathBuf, arguments: EvalArgs) -> Result<i32, String> {
    if arguments.native {
        return crate::native_cli::eval(context, &arguments.selector.module_id, &arguments.selector.export);
    }
    let (linked, value_type, mut sources) = prepare_solved(context, &arguments.selector, false)?;
    let mut vm =
        telora_core::Vm::new().with_debug_sink(std::sync::Arc::new(crate::StderrDebugSink));
    let result = vm
        .execute_linked(
            linked,
            crate::execution_config().session_quota,
            crate::execution_config().data_limits,
            &mut sources,
        )
        .map_err(|error| error.to_string())?;
    let output = result.to_json(value_type)?;
    println!("{output}");
    Ok(0)
}

pub(crate) fn run_with(context: PathBuf, arguments: EvalWithArgs) -> Result<i32, String> {
    if arguments.native {
        return crate::native_cli::eval_with(context, &arguments.selector.module_id, &arguments.selector.export, arguments.sources, arguments.args);
    }
    let (linked, value_type, mut source_database) =
        prepare_solved(context, &arguments.selector, true)?;
    let config = crate::execution_config();
    let _ = eval_source_names(&arguments.sources)?;
    let env = std::env::vars().collect::<BTreeMap<_, _>>();
    let sources = collect_eval_sources(arguments.sources, config.data_limits.file_size)?;
    let mut vm =
        telora_core::Vm::new().with_debug_sink(std::sync::Arc::new(crate::StderrDebugSink));
    let result = vm.execute_eval_with(
        linked,
        telora_core::EvalContext {
            sources,
            env,
            args: arguments.args,
        },
        config.data_limits,
        config.session_quota,
        &mut source_database,
    )?;
    let output = result.to_json(value_type)?;
    println!("{output}");
    Ok(0)
}
