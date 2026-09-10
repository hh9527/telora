//! Inspect the new MIR without the old Engine; --run EXPORT exercises codegen.
//! cargo run -p telora-core --example mir-dump -- @src/main @src/main=main.telora
use std::{collections::BTreeMap, error::Error, path::PathBuf};
use telora_core::{
    mir::ModuleKind,
    module_resolve::{self, ModuleSpec},
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1).peekable();
    let run = if args.peek().is_some_and(|arg| arg == "--run") {
        args.next();
        Some(args.next().ok_or("--run requires an export name")?)
    } else {
        None
    };
    let types = run.is_some() || args.peek().is_some_and(|arg| arg == "--types");
    let symbols = types || args.peek().is_some_and(|arg| arg == "--symbols");
    if symbols && run.is_none() {
        args.next();
    }
    let root = args
        .next()
        .ok_or("expected ROOT followed by NAME=PATH entries")?;
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
        if files.insert(name.to_owned(), path).is_some() {
            return Err(format!("duplicate inventory name {name}").into());
        }
        inventory.push(ModuleSpec {
            native: None,
            name: name.to_owned(),
            kind,
            implicit_imports: vec!["std/prelude".into()],
        });
    }
    for &(name, _) in telora_core::static_sources::BUILTINS {
        if files.contains_key(name) {
            continue;
        }
        inventory.push(ModuleSpec {
            name: name.into(),
            kind: ModuleKind::Source,
            native: telora_core::static_sources::native_module(name),
            implicit_imports: if name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        });
    }
    let mut mir = module_resolve::resolve(inventory, &[root], |_, name| {
        if let Some(path) = files.get(name) {
            std::fs::read_to_string(path).map_err(|error| error.to_string())
        } else {
            Ok(telora_core::static_sources::BUILTINS
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap()
                .1
                .into())
        }
    });
    if symbols {
        telora_core::symbol_resolve::resolve(&mut mir);
    }
    if types {
        telora_core::type_resolve::resolve(&mut mir);
    }
    if let Some(export) = run {
        let telora_core::mir::ModuleTarget::Bound(root) = mir.roots[0] else {
            return Err("unresolved root".into());
        };
        let symbol = *mir.exports[root.index()]
            .iter()
            .find(|id| mir.symbols[id.index()].name == export)
            .ok_or("entry export is missing")?;
        let artifact = telora_core::codegen::compile(&mir, symbol)
            .map_err(|diagnostics| format!("codegen: {diagnostics:?}"))?;
        let mut vm = telora_core::Vm::new();
        let bytecode = telora_core::execution_link::link_builtins(&artifact)
            .map_err(|diagnostics| format!("link: {diagnostics:?}"))?;
        let result = vm.execute(&bytecode, 1_000_000)?;
        println!("{}", result.value());
    } else {
        print!("{}", mir.dump());
    }
    Ok(())
}
