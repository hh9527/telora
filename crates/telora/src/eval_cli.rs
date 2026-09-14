use crate::source_arg::{NamedSource, parse_named_source};
use clap::Args;
use std::path::PathBuf;

#[derive(Clone)]
pub(crate) struct EvalSelector {
    pub(crate) module_id: String,
    pub(crate) export: String,
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

pub(crate) fn parse_eval_selector(value: &str) -> Result<EvalSelector, String> {
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
    crate::wasm_cli::eval(
        context,
        &arguments.selector.module_id,
        &arguments.selector.export,
    )
}

pub(crate) fn run_with(context: PathBuf, arguments: EvalWithArgs) -> Result<i32, String> {
    crate::wasm_cli::eval_with(
        context,
        &arguments.selector.module_id,
        &arguments.selector.export,
        arguments.sources,
        arguments.args,
    )
}
