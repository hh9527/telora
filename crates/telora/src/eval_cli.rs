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
        .ok_or_else(|| "expected @src/MODULE:NAME".to_owned())?;
    if !module_id.starts_with("@src/") || module_id.len() == "@src/".len() {
        return Err("eval module must use an @src/MODULE selector".into());
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
    let (engine, pending) = prepare(&context, &arguments.selector)?;
    let output = engine
        .eval_pending_export(pending, &arguments.selector.export)
        .map_err(|error| error.to_string())?;
    println!("{output}");
    Ok(0)
}

pub(crate) fn run_with(context: PathBuf, arguments: EvalWithArgs) -> Result<i32, String> {
    let (engine, pending) = prepare(&context, &arguments.selector)?;
    let declared_sources = declared_names(pending.option_actions(), "eval-ctx.sources")?;
    let provided = eval_source_names(&arguments.sources)?;
    if declared_sources != provided {
        return Err(format!(
            "eval sources do not match option eval-ctx.sources: declared {declared_sources:?}, provided {provided:?}"
        ));
    }
    let env_names = declared_names(pending.option_actions(), "eval-ctx.env")?;
    let env = env_names
        .into_iter()
        .map(|name| {
            std::env::var(&name)
                .map(|value| (name.clone(), value))
                .map_err(|error| {
                    format!("cannot read declared environment variable {name:?}: {error}")
                })
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
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

fn declared_names(
    options: &[telora_core::LoadedOptionAction],
    key: &str,
) -> Result<Vec<String>, String> {
    let declarations = options
        .iter()
        .filter(|option| option.key == key)
        .collect::<Vec<_>>();
    if declarations.len() > 1 {
        return Err(format!("option {key} may be declared only once"));
    }
    let Some(declaration) = declarations.first() else {
        return Ok(Vec::new());
    };
    let value = declaration.value.value();
    let length = value
        .sequence_len()
        .ok_or_else(|| format!("option {key} must be Array(String)"))?;
    let mut names = (0..length)
        .map(|index| {
            value
                .sequence_get(index)
                .and_then(|item| item.as_str())
                .map(|item| item.as_str().to_owned())
                .ok_or_else(|| format!("option {key} must be Array(String)"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(format!("option {key} must contain unique names"));
    }
    Ok(names)
}
