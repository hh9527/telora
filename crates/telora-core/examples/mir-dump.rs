//! Inspect the new MIR without linking the old Engine or any VM.
//! cargo run -p telora-core --example mir-dump -- @src/main @src/main=main.telora
use std::{collections::BTreeMap, error::Error, path::PathBuf};
use telora_core::{
    mir::ModuleKind,
    module_resolve::{self, ModuleSpec},
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1).peekable();
    let types = args.peek().is_some_and(|arg| arg == "--types");
    let symbols = types || args.peek().is_some_and(|arg| arg == "--symbols");
    if symbols {
        args.next();
    }
    let root = args
        .next()
        .ok_or("expected ROOT followed by NAME=PATH entries")?;
    let mut files = BTreeMap::new();
    let mut inventory = Vec::new();
    let mut intrinsic_names = Vec::new();
    for arg in args {
        if let Some(name) = arg.strip_prefix("--intrinsic=") {
            intrinsic_names.push(name.to_owned());
            continue;
        }
        let (name, path) = arg.split_once('=').ok_or("expected NAME=PATH")?;
        let path = PathBuf::from(path);
        let kind = match path.extension().and_then(|extension| extension.to_str()) {
            Some("json" | "yaml" | "yml" | "toml") => ModuleKind::Data,
            Some("telora") => ModuleKind::Source,
            _ => return Err(format!("unsupported source path {}", path.display()).into()),
        };
        if files.insert(name.to_owned(), path).is_some() {
            return Err(format!("duplicate inventory name {name}").into());
        }
        inventory.push(ModuleSpec {
            name: name.to_owned(),
            kind,
            implicit_imports: vec![],
        });
    }
    let mut mir = module_resolve::resolve(inventory, &[root], |_, name| {
        std::fs::read_to_string(&files[name]).map_err(|error| error.to_string())
    });
    if symbols {
        telora_core::symbol_resolve::resolve(
            &mut mir,
            &intrinsic_names
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
    }
    if types {
        telora_core::type_resolve::resolve(&mut mir);
    }
    print!("{}", mir.dump());
    Ok(())
}
