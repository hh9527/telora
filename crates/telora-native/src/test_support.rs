use telora_core::{
    mir::{HirId, Mir, Role, TypeState},
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};
pub fn graph(source: &str) -> Mir {
    graph_with(source, &[])
}
pub fn graph_with(source: &str, dependencies: &[(&str, &str)]) -> Mir {
    graph_with_data(source, dependencies, &[])
}
pub fn graph_with_data(source: &str, dependencies: &[(&str, &str)], data_modules: &[&str]) -> Mir {
    let mut inputs = vec![
        ("@src/main", source),
        (
            "std/prelude",
            include_str!("../../telora-core/modules/std/prelude.telora"),
        ),
    ];
    for &(name, source) in dependencies {
        if !inputs.iter().any(|(existing, _)| *existing == name) {
            inputs.push((name, source));
        }
    }
    let mut inventory: Vec<_> = inputs
        .iter()
        .map(|(name, _)| ModuleSpec {
            name: (*name).into(),
            kind: telora_core::mir::ModuleKind::Source,
            native: static_sources::native_module(name),
            implicit_imports: if *name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    for &name in data_modules {
        inventory.push(ModuleSpec {
            name: name.into(),
            kind: telora_core::mir::ModuleKind::Data,
            native: None,
            implicit_imports: vec!["std/prelude".into()],
        });
    }
    let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        Ok(inputs.iter().find(|(n, _)| *n == name).unwrap().1.into())
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    mir
}
pub fn value_node(mir: &Mir, name: &str) -> HirId {
    let declaration = mir
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap()
        .declarations[0];
    mir.hir[declaration.index()]
        .children
        .iter()
        .find(|e| e.role == Role::Value)
        .unwrap()
        .node
}
pub fn value_type(mir: &Mir, name: &str) -> crate::abi::TypeKey {
    let TypeState::Known(ty) = mir.ty_slots[value_node(mir, name).index()] else {
        panic!("unsolved test type")
    };
    ty.try_into().unwrap()
}
