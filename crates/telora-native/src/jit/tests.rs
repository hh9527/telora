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
fn unused_bindings_still_execute_and_propagate_failure_once() {
    let (mir, root) = graph("export def answer: Fn() -> Int = fn() { let unused = [1][2]; 42 };");
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert!(context.diagnostics()[0].message.contains("out of bounds"));
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
#[test]
fn machine_code_constructs_native_objects_without_old_vm() {
    for (source, kind) in [
        (
            "export def answer = \"a long string from generated machine code\";",
            "string",
        ),
        ("export def answer = [7, 8];", "array"),
        (
            "export def answer = {label: \"long shared record text\", count: 7};",
            "record",
        ),
        ("export def answer: Dict(Int) = {z: 8, a: 7};", "dict"),
    ] {
        let (mir, root) = graph(source);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut ctx = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        let value = compiled
            .call(&mut ctx, &[])
            .unwrap_or_else(|e| panic!("{e}: {:?}", ctx.diagnostics()));
        let rt = ctx.runtime().unwrap();
        match kind {
            "string" => assert_eq!(
                rt.text(value.as_ref()).unwrap().as_str(),
                "a long string from generated machine code"
            ),
            "array" => assert_eq!(rt.scalar_bits(rt.array_get(&value, 0).unwrap()).unwrap(), 7),
            "record" => assert_eq!(rt.scalar_bits(rt.field(&value, 0).unwrap()).unwrap(), 7),
            "dict" => {
                let (key, found) = rt.dict_entry(&value, 0).unwrap();
                assert_eq!(rt.text(key).unwrap().as_str(), "a");
                assert_eq!(rt.scalar_bits(found).unwrap(), 7);
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn generated_index_reads_published_main_and_reports_work_bounds_errors() {
    let (mir, root) = graph(
        "export def answer: Fn(Array(Int), Int) -> Int = fn(values, index) { values[index] };",
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let array_ty = compiled.arguments()[0];
    let int = compiled.arguments()[1];
    let origin = Origin::from_loc(Some(mir.hir[root.index()].location));
    let mut rt = crate::runtime::Runtime::new(&sealed).unwrap();
    let item = rt.scalar(int, origin.words(), 42).unwrap();
    let array = rt.array(array_ty, origin.words(), &[item]).unwrap();
    let roots = rt.publish(&[array]).unwrap();
    let mut ctx = CallContext::with_runtime(rt);
    let zero = compiled
        .layouts()
        .value(int, Origin::default(), &[0])
        .unwrap();
    let value = compiled.call(&mut ctx, &[roots[0].clone(), zero]).unwrap();
    assert_eq!(value.words()[2], 42);
    assert_eq!(value.origin(), origin);
    let bad_index = compiled
        .layouts()
        .value(int, Origin::default(), &[1])
        .unwrap();
    assert!(
        compiled
            .call(&mut ctx, &[roots[0].clone(), bad_index])
            .is_err()
    );
    assert_eq!(ctx.diagnostics().len(), 1);
    assert!(ctx.diagnostics()[0].message.contains("out of bounds"));
    assert_ne!(ctx.diagnostics()[0].origin, Origin::default());
}

#[test]
fn direct_functions_have_independent_frames_and_support_recursion() {
    for source in [
        "def choose: Fn(Bool, Int, Int) -> Int = fn(flag, x, y) { if flag {x} else {y} }; export def answer = do { let left = 41; let right = 42; choose(False, left, right) };",
        "def recur: Fn(Bool) -> Int = fn(flag) { if flag { recur(False) } else { 42 } }; export def answer = recur(True);",
        "export def answer = do { def local: Fn(Int) -> Int = fn(value) { let saved = value; saved }; local(42) };",
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let result = compiled.call(&mut CallContext::default(), &[]).unwrap();
        assert_eq!(result.words()[2], 42);
    }
}
#[test]
fn scalar_machine_code_handles_recursion_and_checked_arithmetic() {
    let (mir, root) = graph(include_str!("../../tests/fixtures/factorial.telora"));
    let compiled = compile(&mir.seal().unwrap(), root).unwrap();
    assert_eq!(
        compiled
            .call(&mut CallContext::default(), &[])
            .unwrap()
            .words()[2],
        120
    );
    for (source, message) in [
        ("export def answer = 9223372036854775807 + 1;", "overflowed"),
        (
            "def divide: Fn(Int, Int) -> Int = fn(a, b) { a / b }; export def answer = divide(1, 0);",
            "division by zero",
        ),
        ("export def answer = 1.0 / 0.0;", "non-finite"),
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let mut ctx = CallContext::default();
        assert!(compiled.call(&mut ctx, &[]).is_err());
        assert_eq!(ctx.diagnostics().len(), 1);
        assert!(
            ctx.diagnostics()[0].message.contains(message),
            "{:?}",
            ctx.diagnostics()
        );
        assert_ne!(ctx.diagnostics()[0].origin, Origin::default());
    }
    for source in [
        "export def answer = False && [True][1];",
        "export def answer = True || [False][1];",
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let mut ctx = CallContext::default();
        compiled.call(&mut ctx, &[]).unwrap(); // RHS requires a runtime and would fail if executed.
        assert!(ctx.diagnostics().is_empty());
    }
}
#[test]
fn generic_calls_consume_closed_instances_without_substituting_types_at_runtime() {
    let (mir, root) = graph(include_str!("../../tests/fixtures/generics.telora"));
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let result = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime().unwrap();
    assert_eq!(rt.scalar_bits(rt.field(&result, 0).unwrap()).unwrap(), 42);
    assert_eq!(
        rt.text(rt.field(&result, 1).unwrap()).unwrap().as_str(),
        "native generic text"
    );
    let array = rt.field(&result, 2).unwrap().to_owned();
    assert_eq!(rt.scalar_bits(rt.array_get(&array, 0).unwrap()).unwrap(), 7);
}
#[test]
fn generated_enums_preserve_inline_and_boxed_payloads_through_publication() {
    let (mir, root) = graph(include_str!("../../tests/fixtures/enums.telora"));
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let value = compiled
        .call(&mut context, &[])
        .unwrap_or_else(|e| panic!("{e}: {:?}", context.diagnostics()));
    let roots = context.runtime_mut().unwrap().publish(&[value]).unwrap();
    let rt = context.runtime().unwrap();
    let tree = rt.field(&roots[0], 0).unwrap().to_owned();
    assert_eq!(
        rt.variant_name(tree.type_key(), rt.enum_tag(&tree).unwrap())
            .unwrap(),
        "Children"
    );
    let array = rt.enum_payload(&tree).unwrap().unwrap().to_owned();
    let leaf = rt.array_get(&array, 0).unwrap().to_owned();
    assert_eq!(
        rt.scalar_bits(rt.enum_payload(&leaf).unwrap().unwrap())
            .unwrap(),
        42
    );
    let empty = rt.array_get(&array, 1).unwrap().to_owned();
    assert!(rt.enum_payload(&empty).unwrap().is_none());
    let link = rt.field(&roots[0], 1).unwrap().to_owned();
    let end = rt.enum_payload(&link).unwrap().unwrap().to_owned();
    assert_eq!(
        rt.variant_name(end.type_key(), rt.enum_tag(&end).unwrap())
            .unwrap(),
        "End"
    );
    assert!(rt.enum_payload(&end).unwrap().is_none());
}
#[test]
fn patterns_select_payloads_and_preserve_diverging_paths() {
    for source in [
        "export def answer = match 2 { 1 => 0, 2 => 42, _ => 7 };",
        "export def answer = match Some(42) { Some(x) => x, None => 0 };",
        "export def answer = match (2, 40) { (a, b) => a + b };",
        "export def answer = if let Some(x) = Some(42) { x } else { 0 };",
        "export def answer = match \"yes\" { \"no\" => 0, \"yes\" => 42, _ => 7 };",
        "export def answer = match 2 { 1 => fail!(\"wrong\"), _ => 42 };",
    ] {
        let (mir, root) = graph(source);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        assert_eq!(
            compiled.call(&mut context, &[]).unwrap().words()[2],
            42,
            "{source}"
        );
        assert!(context.diagnostics().is_empty());
    }
}

#[test]
fn never_paths_do_not_allocate_values_or_force_a_join_result() {
    for source in [
        "export def answer: Fn(Bool) -> Int = fn(flag) { if flag { return 42; } else { 7 } };",
        "export def answer: Fn(Bool) -> Int = fn(flag) { if flag { return 42; } else { return 7; } };",
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        for (flag, expected) in [(1, 42), (0, 7)] {
            let flag = compiled
                .layouts()
                .value(compiled.arguments()[0], Origin::default(), &[flag])
                .unwrap();
            assert_eq!(
                compiled
                    .call(&mut CallContext::default(), &[flag])
                    .unwrap()
                    .words()[2],
                expected
            );
        }
    }
    let (mir, root) = graph(
        "def explode: Fn() -> Never = fn() { fail!(\"boom\") }; export def answer = explode();",
    );
    let compiled = compile(&mir.seal().unwrap(), root).unwrap();
    assert!(compiled.layouts().is_never(compiled.output()).unwrap());
    let mut context = CallContext::default();
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "boom");
    for source in [
        "export def answer = True || fail!(\"unreachable\");",
        "export def answer = False && fail!(\"unreachable\");",
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let mut context = CallContext::default();
        compiled.call(&mut context, &[]).unwrap();
        assert!(context.diagnostics().is_empty());
    }
}
