//! Hidden publication interface. File execution does not load a workspace.
use crate::{
    eval_cli::{EvalSelector, parse_eval_selector},
    source_arg::{NamedSource, parse_named_source},
    static_input::Inventory,
};
use clap::{Args, Subcommand};
use std::{fs, path::PathBuf};
use telora_core::{SourceDatabase, mir::ModuleTarget};
use telora_wasm::{artifact::Manifest, session::Session};

#[derive(Args)]
pub(crate) struct ArtifactArgs {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Build {
        #[arg(value_parser = parse_eval_selector)]
        selector: EvalSelector,
        #[arg(short, long)]
        output: PathBuf,
    },
    Check {
        file: PathBuf,
    },
    Eval {
        file: PathBuf,
    },
    EvalWith {
        file: PathBuf,
        #[arg(long = "source", value_parser = parse_named_source)]
        sources: Vec<NamedSource>,
        #[arg(last = true)]
        args: Vec<String>,
    },
}

pub(crate) fn run(context: PathBuf, arguments: ArtifactArgs) -> Result<i32, String> {
    let command = arguments.command;
    if let Command::Build { selector, output } = command {
        return build(context, selector, output);
    }
    let file = match &command {
        Command::Check { file } | Command::Eval { file } | Command::EvalWith { file, .. } => file,
        Command::Build { .. } => unreachable!(),
    };
    let bytes =
        fs::read(file).map_err(|e| format!("cannot read artifact {}: {e}", file.display()))?;
    let mut session = Session::load(
        &bytes,
        (crate::execution_config().session_quota.fuel as u64).saturating_mul(100),
    )?;
    match &command {
        Command::Eval { .. }
            if session.manifest.value_type != Some(session.manifest.entry_type) =>
        {
            return Err("eval artifact must export std/value.Value".into());
        }
        Command::EvalWith { .. }
            if session.manifest.eval_type != Some(session.manifest.entry_type) =>
        {
            return Err("eval-with artifact must export std/entry.Eval".into());
        }
        _ => {}
    }
    let initialized = session.initialize();
    super::diagnostics::finish_portable(&session, 0, initialized)?;
    match command {
        Command::Check { .. } => Ok(0),
        Command::Eval { .. } => {
            let before = session.diagnostics()?.len();
            let result = session.eval();
            println!(
                "{}",
                super::diagnostics::finish_portable(&session, before, result)?
            );
            Ok(0)
        }
        Command::EvalWith { sources, args, .. } => {
            // Reserve the published file identities without restoring their text.
            let mut database = SourceDatabase::default();
            let mut files = session.manifest.sources.clone();
            files.sort_by_key(|file| file.id);
            for file in files {
                if file.id as usize != database.files().len() + 1 {
                    return Err("Wasm: published source identities are not contiguous".into());
                }
                database.try_add(file.name, "").map_err(|e| e.to_string())?;
            }
            super::execute_with(&mut session, &mut database, sources, args, true)
        }
        Command::Build { .. } => unreachable!(),
    }
}

fn build(context: PathBuf, selector: EvalSelector, output: PathBuf) -> Result<i32, String> {
    let mut inventory = Inventory::new(&context, selector.module_id.starts_with("std/"))?;
    let root = inventory.select(&selector.module_id)?;
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
        return Err("unresolved Wasm publication module".into());
    };
    let symbol = *mir.exports[module.index()]
        .iter()
        .find(|id| mir.symbols[id.index()].name == selector.export)
        .ok_or("module has no selected export")?;
    let executable = sealed.seal_export(symbol).map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let bytes = telora_wasm::compile_executable(&executable)?;
    let manifest = Manifest::read(&bytes)?;
    let mut plans = vec![];
    for module in manifest.data_modules {
        let (format, text) = inventory.read_data_text(
            &module.name,
            crate::execution_config().data_limits.file_size,
        )?;
        let source = mir
            .sources
            .try_add(module.name, &text)
            .map_err(|e| e.to_string())?;
        let plan = telora_core::data_plan::parse_registered(&mir.sources, source, format).map_err(
            |diagnostics| {
                diagnostics
                    .iter()
                    .map(|d| mir.sources.render(d))
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        )?;
        telora_core::data_plan::enforce_limits(
            &plan,
            crate::execution_config().data_limits,
            text.len(),
        )?;
        plans.push((module.symbol, plan));
    }
    let bytes = telora_wasm::bundle::build(&bytes, &mir.sources, &plans)?;
    fs::write(&output, bytes)
        .map_err(|e| format!("cannot write artifact {}: {e}", output.display()))?;
    Ok(0)
}
