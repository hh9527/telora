use super::*;

#[test]
fn explicit_type_application_reports_contract_arity_and_inherits_resolution_failures() {
    for (declaration, application, expected) in [
        (
            "def identity: Fn(Int) -> Int = fn(value) {value};",
            "identity@[Int](1)",
            "monomorphic binding",
        ),
        (
            "def identity: for(T) Fn(T) -> T = fn(value) {value};",
            "identity@[Int, String](1)",
            "expects 1 arguments, found 2",
        ),
        (
            "def pair: for(A, B) Fn(A, B) -> A = fn(left, right) {left};",
            "pair@[Int](1, 2)",
            "expects 2 arguments, found 1",
        ),
    ] {
        let source = format!(
            "{declaration} pub def bad: Int = {application}; pub def independent: Int = 42;"
        );
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics.iter().any(|d| d.message.contains(expected)),
            "{}",
            mir.dump()
        );
        assert!(matches!(
            symbol_type(&mir, "independent"),
            TypeState::Known(_)
        ));
        assert!(mir.seal().is_err());
    }
    let mut mir = graph(&[(
        "@src/main",
        "pub def bad: Int = missing@[Int](1); pub def independent: Int = 42;",
    )]);
    resolve(&mut mir);
    assert_eq!(mir.diagnostics.len(), 1, "{}", mir.dump());
    assert!(mir.diagnostics[0].message.contains("unknown binding"));
    let application = mir
        .hir
        .iter()
        .position(|node| matches!(node.kind, HirKind::TypeApply))
        .unwrap();
    let TypeState::Conflicted(conflict) = mir.ty_slots[application] else {
        panic!("{}", mir.dump())
    };
    assert!(
        mir.type_conflicts[conflict.index()]
            .resolve_origin
            .is_some()
    );
    assert!(matches!(
        symbol_type(&mir, "independent"),
        TypeState::Known(_)
    ));
    assert!(mir.seal().is_err());
}

#[test]
fn bottom_tails_wait_for_explicit_return_evidence_from_generalized_constructors() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        pub def answer: Bool = do {
        def choose = fn(flag) {
            if flag { return Ok("hi"); } else { return Err(2); }
        };
        choose(True) == Ok("hi") && choose(False) == Err(2)
        };
    "#,
    )]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let TypeState::Known(signature) = symbol_type(&mir, "choose") else {
        panic!("closed signature")
    };
    let result = *mir.types[signature.index()].arguments.last().unwrap();
    assert_eq!(
        mir.types[result.index()].constructor,
        TypeConstructor::Result
    );
    let arguments = &mir.types[result.index()].arguments;
    assert_eq!(
        mir.types[arguments[0].index()].constructor,
        TypeConstructor::String
    );
    assert_eq!(
        mir.types[arguments[1].index()].constructor,
        TypeConstructor::Int
    );
}

#[test]
fn let_else_checks_divergence_without_overwriting_the_inferred_branch_type() {
    for branch in ["0", "()", "if flag { 0 } else { fail!(\"stop\") }"] {
        let source = format!(
            "pub def read: Fn(Option(Int), Bool) -> Int = fn(value, flag) {{ let Some(item) = value else {{ {branch} }}; item }}; pub def independent: Int = 42;"
        );
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "let else branch must have type Never"),
            "{}",
            mir.dump()
        );
        let node = mir
            .hir
            .iter()
            .find(|node| matches!(node.kind, HirKind::LetElse))
            .unwrap();
        let branch = node
            .children
            .iter()
            .find(|edge| edge.role == Role::Else)
            .unwrap()
            .node;
        let TypeState::Known(ty) = mir.ty_slots[branch.index()] else {
            panic!("branch retains solved evidence")
        };
        assert_ne!(mir.types[ty.index()].constructor, TypeConstructor::Never);
        let TypeState::Known(ty) = symbol_type(&mir, "independent") else {
            panic!("independent type")
        };
        assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::Int);
        mir.diagnostics.clear();
        assert!(mir.seal().is_err(), "seal must enforce divergence itself");
    }
    for branch in [
        "return 42;",
        "fail!(\"stop\")",
        "if flag { return 1; } else { return 2; }",
    ] {
        let source = format!(
            "pub def read: Fn(Option(Int), Bool) -> Int = fn(value, flag) {{ let Some(item) = value else {{ {branch} }}; item }};"
        );
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        mir.seal()
            .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    }
}

#[test]
fn nonreturning_array_items_supply_bottom_only_after_live_element_evidence() {
    for (expression, expected) in [
        ("[stop(), 1]", TypeConstructor::Int),
        ("[1, stop()]", TypeConstructor::Int),
        ("[stop(), stop()]", TypeConstructor::Never),
        ("if True { [stop()] } else { [1] }", TypeConstructor::Int),
        ("if True { [1] } else { [stop()] }", TypeConstructor::Int),
    ] {
        let source = format!(
            "def stop: Fn() -> Never = fn() {{ fail!(\"stop\") }}; pub def checked: Bool = do {{ let answer = {expression}; True }};"
        );
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        mir.seal()
            .unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let TypeState::Known(ty) = symbol_type(&mir, "answer") else {
            panic!("closed array")
        };
        assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::Array);
        assert_eq!(
            mir.types[mir.types[ty.index()].arguments[0].index()].constructor,
            expected,
            "{source}"
        );
        for node in mir.hir.iter().enumerate().filter_map(|(index, node)| {
            (node.module == ModuleId(0) && matches!(node.kind, HirKind::Call))
                .then_some(HirId(index as u32))
        }) {
            let TypeState::Known(ty) = mir.ty_slots[node.index()] else {
                continue;
            };
            assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::Never);
        }
    }
}

#[test]
fn propagation_keeps_never_tail_error_evidence_and_infers_operand_from_return_context() {
    let mut mir = graph(&[(
        "@src/main",
        "pub def checked: Bool = do { def stopped = fn(value: Result(Int, String)) { value?; fail!(\"tail\") }; def contextual = fn(value) -> Option(Int) { Some(value? + 1) }; True };",
    )]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let TypeState::Known(stopped) = symbol_type(&mir, "stopped") else {
        panic!("closed function")
    };
    let result = *mir.types[stopped.index()].arguments.last().unwrap();
    assert_eq!(
        mir.types[result.index()].constructor,
        TypeConstructor::Result
    );
    assert_eq!(
        mir.types[mir.types[result.index()].arguments[0].index()].constructor,
        TypeConstructor::Never
    );
    let TypeState::Known(contextual) = symbol_type(&mir, "contextual") else {
        panic!("closed function")
    };
    let input = mir.types[contextual.index()].arguments[0];
    assert_eq!(
        mir.types[input.index()].constructor,
        TypeConstructor::Option
    );
    assert_eq!(
        mir.types[mir.types[input.index()].arguments[0].index()].constructor,
        TypeConstructor::Int
    );
}

#[test]
fn empty_record_fields_receive_context_before_bottom_and_seal_checks_the_boundary() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/issue-202/empty-record-result.telora"),
    )
    .unwrap();
    let mut mir = graph(&[("@src/main", &source)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}"));
    let arrays = mir
        .hir
        .iter()
        .enumerate()
        .filter(|(_, n)| n.module == ModuleId(0) && matches!(n.kind, HirKind::Array))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(arrays.len(), 2);
    let TypeState::Known(ty) = mir.ty_slots[arrays[0]] else {
        panic!("array type")
    };
    assert!(matches!(
        mir.types[mir.types[ty.index()].arguments[0].index()].constructor,
        TypeConstructor::Nominal(_)
    ));
    // A fully known but wrong field type must not reach codegen.
    mir.ty_slots[arrays[0]] = mir.ty_slots[arrays[1]];
    let errors = mir
        .seal()
        .err()
        .expect("invalid field boundary must reject seal");
    assert!(errors.iter().any(|d| {
        d.message.contains("diagnostics")
            && d.labels
                .iter()
                .any(|label| label.location == mir.hir[arrays[0]].location)
    }));
}

#[test]
fn branch_completion_does_not_unify_candidate_and_checked_identity() {
    for expression in [
        "if True { candidate } else { good }",
        "match True { True => good, False => candidate }",
    ] {
        let source = format!(
            "type Point = struct {{x: Int}}; def candidate: Unchecked(Point) = {{x: 0}}; def good: Point = {{x: 42}}; pub def checked: Bool = do {{ let answer = {expression}; True }};"
        );
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
        mir.seal().unwrap();
        let TypeState::Known(candidate) = symbol_type(&mir, "candidate") else {
            panic!("candidate");
        };
        let TypeState::Known(answer) = symbol_type(&mir, "answer") else {
            panic!("answer");
        };
        assert_eq!(
            mir.types[candidate.index()].constructor,
            TypeConstructor::Unchecked
        );
        assert_eq!(mir.types[candidate.index()].arguments, [answer]);
        assert_eq!(
            mir.value_adjustments.iter().flatten().count(),
            1,
            "{}",
            mir.dump()
        );
    }
}

#[test]
fn solves_function_calls_across_modules_in_one_arena() {
    let mut mir = graph(&[
        (
            "@src/main",
            "use self::math::{ inc }; pub def answer: Int = inc(41); pub def pair: (Int, Int) = (answer, 2);",
        ),
        (
            "@src/math",
            "pub def inc: Fn(Int) -> Int = fn(x) { x + 1 };",
        ),
    ]);
    let hir = mir.hir.as_ptr();
    let references = mir.resolve_slots.clone();
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert_eq!(hir, mir.hir.as_ptr());
    assert_eq!(references, mir.resolve_slots);
    let TypeState::Known(answer) = symbol_type(&mir, "answer") else {
        panic!("{}", mir.dump());
    };
    assert_eq!(mir.types[answer.index()].constructor, TypeConstructor::Int);
    let TypeState::Known(pair) = symbol_type(&mir, "pair") else {
        panic!("{}", mir.dump());
    };
    assert_eq!(mir.types[pair.index()].arguments, vec![answer, answer]);
    assert!(
        mir.ty_slots
            .iter()
            .all(|state| !matches!(state, TypeState::ProxyTo(_) | TypeState::Structure(_)))
    );
}

#[test]
fn equal_children_canonicalize_structures_without_merging_unrelated_evidence() {
    assert_eq!(std::mem::size_of::<TypeState>(), 8);
    assert!(!std::mem::needs_drop::<TypeState>());
    let mut mir = Mir::default();
    let mut solver = Solver::new(&mut mir);
    let a = solver.fresh();
    let b = solver.fresh();
    let array_a = solver.structure(TypeConstructor::Array, vec![a]);
    let array_b = solver.structure(TypeConstructor::Array, vec![b]);
    solver.equal(a, b, None);
    let integer = solver.structure(TypeConstructor::Int, vec![]);
    solver.equal(b, integer, None);
    solver.finalize();
    assert_eq!(
        solver.mir.ty_slots[array_a.index()],
        solver.mir.ty_slots[array_b.index()]
    );
    assert!(matches!(
        solver.mir.ty_slots[array_a.index()],
        TypeState::Known(_)
    ));
}

#[test]
fn solves_match_boolean_and_never_branches() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        type Choice = enum { Number(Int), Missing };
        def read: Fn(Choice) -> Int = fn(value) {
            match value {
                Choice.Number(n) => if !(n < 0) && True { -n } else { fail!("bad") },
                Choice.Missing => fail!("missing"),
            }
        };
        pub def answer: Int = read(Choice.Number(3));
        pub def projection: Int = (1, "ok").0;
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
}

#[test]
fn native_slot_identity_does_not_depend_on_the_declared_name() {
    let mut mir = graph(&[
        ("@src/main", "pub def answer: Quantity = 42;"),
        (
            "std/prelude",
            "native type Quantity @4; pub use self::{ Quantity };",
        ),
    ]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let TypeState::Known(id) = symbol_type(&mir, "answer") else {
        panic!("{}", mir.dump());
    };
    assert_eq!(mir.types[id.index()].constructor, TypeConstructor::Int);
}

#[test]
fn unregistered_native_slots_cannot_create_intrinsic_types() {
    for source in [
        "native type Forged @4; pub def value: Forged = 1;",
        "native type Int @999; pub def value: Int = 1;",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.contains("no registered static contract"))
        );
        assert!(
            mir.symbols
                .iter()
                .filter(
                    |s| s.kind == SymbolKind::Declaration(BindingKind::NativeType)
                        && s.module
                            .is_some_and(|id| mir.modules[id.index()].name == "@src")
                )
                .all(|s| s.native_type.is_none())
        );
    }
}

#[test]
fn data_contract_resolves_the_exported_value_type_without_reading_data() {
    let mut mir = graph(&[
        (
            "@src/main",
            "data data = import(json) \"./payload.json\"; use std::value::{Value}; pub def answer: Value = data;",
        ),
        ("@src/payload.json", "THIS IS NOT JSON OR TELORA"),
        ("std/value", "pub type Value = Int;"),
    ]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
    let TypeState::Known(answer) = symbol_type(&mir, "answer") else {
        panic!("{}", mir.dump());
    };
    assert_eq!(mir.types[answer.index()].constructor, TypeConstructor::Int);
    assert!(
        mir.modules
            .iter()
            .any(|m| matches!(m.state, ModuleState::Data { .. }))
    );
}
