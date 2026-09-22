use super::*;
use crate::module_resolve::{self, ModuleSpec};

mod contracts;
mod convergence;
mod diagnostics;
mod generics;
mod inference;
mod properties;
mod shapes;

fn graph(sources: &[(&str, &str)]) -> Mir {
    let authored_root = sources[0].0;
    let root = authored_root.split('/').next().unwrap();
    let declarations = sources
        .iter()
        .skip(1)
        .filter(|(name, _)| name.starts_with("@src/") && !name.contains('.'))
        .map(|(name, _)| format!("mod {};", name.rsplit('/').next().unwrap()))
        .collect::<Vec<_>>()
        .join(" ");
    let mut owned = sources
        .iter()
        .enumerate()
        .map(|(index, (name, source))| {
            (
                if index == 0 {
                    root.to_owned()
                } else if name.starts_with("@src/") {
                    format!("{root}/{}", name.rsplit('/').next().unwrap())
                } else {
                    (*name).to_owned()
                },
                if index == 0 {
                    format!("{declarations} {source}")
                } else {
                    (*source).to_owned()
                },
            )
        })
        .collect::<Vec<_>>();
    for &(name, source) in crate::static_sources::BUILTINS {
        if !owned.iter().any(|(existing, _)| existing == name) {
            owned.push((name.into(), source.into()));
        }
    }
    let inventory = owned
        .iter()
        .map(|(name, _)| ModuleSpec {
            native: crate::static_sources::native_module(name),
            name: name.clone(),
            kind: if name.ends_with(".json") {
                ModuleKind::Data
            } else {
                ModuleKind::Source
            },
            implicit_imports: if name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &[root.into()], |_, name| {
        Ok(owned.iter().find(|(key, _)| key == name).unwrap().1.clone())
    });
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    crate::symbol_resolve::resolve(&mut mir);
    mir
}
fn symbol_type(mir: &Mir, name: &str) -> TypeState {
    let id = mir
        .symbols
        .iter()
        .position(|symbol| symbol.name == name && matches!(symbol.kind, SymbolKind::Declaration(_)))
        .unwrap();
    mir.ty_slots[mir.symbol_types[id].index()]
}
