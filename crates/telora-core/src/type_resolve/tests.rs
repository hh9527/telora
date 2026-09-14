use super::*;
use crate::module_resolve::{self, ModuleSpec};

mod diagnostics;
mod generics;
mod inference;
mod properties;
mod shapes;
mod convergence;

fn graph(sources: &[(&str, &str)]) -> Mir {
    let mut sources = sources.to_vec();
    if !sources.iter().any(|(name, _)| *name == "std/prelude") {
        sources.push((
            "std/prelude",
            crate::static_sources::BUILTINS
                .iter()
                .find(|(name, _)| *name == "std/prelude")
                .unwrap()
                .1,
        ));
    }
    let inventory = sources
        .iter()
        .map(|(name, _)| ModuleSpec {
            native: crate::static_sources::native_module(name),
            name: (*name).into(),
            kind: if name.ends_with(".json") {
                ModuleKind::Data
            } else {
                ModuleKind::Source
            },
            implicit_imports: if *name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &[sources[0].0.into()], |_, name| {
        Ok(sources
            .iter()
            .find(|(key, _)| *key == name)
            .unwrap()
            .1
            .into())
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
