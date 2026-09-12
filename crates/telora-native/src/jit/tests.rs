use super::*;
use telora_core::{
    mir::ModuleKind,
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};

fn graph(source: &str) -> (Mir, HirId) {
    let inputs = [
        ("@src/main", source),
        (
            "std/prelude",
            include_str!("../../../telora-core/modules/std/prelude.telora"),
        ),
    ];
    let inventory = inputs
        .iter()
        .map(|(name, _)| ModuleSpec {
            name: (*name).into(),
            kind: ModuleKind::Source,
            native: static_sources::native_module(name),
            implicit_imports: if *name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        Ok(inputs.iter().find(|(n, _)| *n == name).unwrap().1.into())
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    let declaration = mir
        .symbols
        .iter()
        .find(|s| s.name == "answer")
        .unwrap()
        .declarations[0];
    let root = child(&mir, declaration, Role::Value).unwrap();
    (mir, root)
}

#[test]
fn machine_code_returns_materialized_scalar_and_unit() {
    for (source, data) in [
        ("export def answer = 42;", Some(42)),
        ("export def answer = 1.25;", Some(1.25f64.to_bits())),
        ("export def answer = ();", None),
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let value = compiled.call(&mut CallContext::default(), &[]).unwrap();
        assert_eq!(value.type_key(), compiled.output());
        assert_eq!(
            value.origin(),
            Origin::from_loc(Some(mir.hir[root.index()].location))
        );
        assert_eq!(value.words().get(2).copied(), data);
    }
}

#[test]
fn machine_code_branches_on_argument_and_preserves_selected_value_origin() {
    let (mir, root) = graph(
        "export def answer: Fn(Bool, Int, Int) -> Int = fn(condition, yes, no) { if condition { yes } else { no } };",
    );
    let compiled = compile(&mir.seal().unwrap(), root).unwrap();
    let ids = compiled.arguments();
    let origin = Origin::from_loc(Some(mir.hir[root.index()].location));
    let yes = compiled.layouts().value(ids[1], origin, &[41]).unwrap();
    let no = compiled
        .layouts()
        .value(ids[2], Origin::default(), &[42])
        .unwrap();
    for (condition, expected) in [(1, &yes), (0, &no)] {
        let condition = compiled
            .layouts()
            .value(ids[0], Origin::default(), &[condition])
            .unwrap();
        assert_eq!(
            &compiled
                .call(
                    &mut CallContext::default(),
                    &[condition, yes.clone(), no.clone()]
                )
                .unwrap(),
            expected
        );
    }
    assert!(compiled.call(&mut CallContext::default(), &[]).is_err());
    assert!(
        compiled
            .call(&mut CallContext::default(), &[yes.clone(), yes.clone(), no])
            .is_err()
    );
}

#[test]
fn unsupported_body_is_rejected_instead_of_dropping_statements() {
    let (mir, root) = graph("export def answer: Fn() -> Int = fn() { let unused = 1; 42 };");
    let error = match compile(&mir.seal().unwrap(), root) {
        Ok(_) => panic!("unexpected supported body"),
        Err(e) => e,
    };
    assert!(error.contains("unsupported block statements"), "{error}");
}
#[test]
fn failed_call_never_decodes_the_result_and_invalid_success_is_rejected() {
    // The wrapper is also the boundary used for generated calls to failing
    // helpers. A Failed status must not inspect the zeroed/unwritten result.
    unsafe extern "C" fn failed(_: *mut CallContext, _: *const u64, _: *mut u64) -> u32 {
        1
    }
    unsafe extern "C" fn unwritten(_: *mut CallContext, _: *const u64, _: *mut u64) -> u32 {
        0
    }
    let (mir, root) = graph("export def answer = 42;");
    let mut compiled = compile(&mir.seal().unwrap(), root).unwrap();
    compiled.entry = failed;
    assert_eq!(
        compiled.call(&mut CallContext::default(), &[]).unwrap_err(),
        "native execution failed"
    );
    compiled.entry = unwritten;
    // Zeroed data has no valid stamp for this graph's concrete Int type.
    assert!(compiled.call(&mut CallContext::default(), &[]).is_err());
}
