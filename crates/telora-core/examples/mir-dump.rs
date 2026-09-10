//! Inspect the new MIR without linking the old Engine or any VM.
//! cargo run -p telora-core --example mir-dump -- @src/main @src/main=main.telora
use std::{collections::BTreeMap, error::Error, path::PathBuf};
use telora_core::{mir::ModuleKind, module_resolve::{self, ModuleSpec}};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let root = args.next().ok_or("expected ROOT followed by NAME=PATH entries")?;
    let mut files = BTreeMap::new();
    let mut inventory = Vec::new();
    for arg in args {
        let (name, path) = arg.split_once('=').ok_or("expected NAME=PATH")?;
        let path = PathBuf::from(path);
        let kind = match path.extension().and_then(|extension| extension.to_str()) {
            Some("json" | "yaml" | "yml" | "toml") => ModuleKind::Data,
            Some("telora") => ModuleKind::Source,
            _ => return Err(format!("unsupported source path {}", path.display()).into()),
        };
        if files.insert(name.to_owned(), path).is_some() { return Err(format!("duplicate inventory name {name}").into()); }
        inventory.push(ModuleSpec { name: name.to_owned(), kind, implicit_imports: vec![] });
    }
    let mir = module_resolve::resolve(inventory, &[root], |_, name| {
        std::fs::read_to_string(&files[name]).map_err(|error| error.to_string())
    });
    print!("{}", mir.dump());
    Ok(())
}
