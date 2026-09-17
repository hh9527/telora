//! File publication is separate from the existing source run/serve path.
use clap::Args;
use std::{io::Write, path::PathBuf};
use telora_core::{mir::ModuleTarget, source::Severity};
use telora_wasm::artifact::Manifest;

#[derive(Args)]
pub struct BuildArgs {
    #[arg(value_name = "MODULE")]
    module: String,
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
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
    let bytes = telora_wasm::publication::finish(&bytes, config.fuel, config.memory_limit)?;
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
