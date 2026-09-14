use super::*;

#[test]
fn native_value_shapes_are_diagnosed_before_codegen_and_enforced_by_seal() {
    let mut mir = graph(&[(
        "@src/main",
        "native value: Int; export { value }; export def independent = 42;",
    )]);
    resolve(&mut mir);
    assert!(
        mir.diagnostics
            .iter()
            .any(|d| d.message == "native declaration requires a function signature"),
        "{}",
        mir.dump()
    );
    assert!(matches!(
        symbol_type(&mir, "independent"),
        TypeState::Known(_)
    ));
    mir.diagnostics.clear();
    assert!(mir.seal().is_err());
}

#[test]
fn conflicts_render_both_type_shapes_before_poisoning_the_slots() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        def values: Array(Int) = [1];
        export def bad: Bool = values;
        export def independent = 42;
    "#,
    )]);
    resolve(&mut mir);
    let conflict = mir
        .type_conflicts
        .iter()
        .find(|conflict| conflict.message.contains("type mismatch"))
        .unwrap();
    assert!(
        conflict.message.contains("Array<Int>"),
        "{}",
        conflict.message
    );
    assert!(conflict.message.contains("Bool"), "{}", conflict.message);
    assert!(!conflict.message.contains("SymbolId"));
    assert!(mir.diagnostics.iter().any(|d| {
        d.message == conflict.message
            && d.labels
                .iter()
                .any(|label| Some(label.location) == conflict.location)
    }));
    assert!(matches!(
        symbol_type(&mir, "independent"),
        TypeState::Known(_)
    ));
    assert!(mir.seal().is_err());
}

#[test]
fn diagnostic_type_rendering_is_bounded_and_read_only() {
    let mut mir = graph(&[("@src/main", "export def answer = 42;")]);
    let mut solver = Solver::new(&mut mir);
    let integer = solver.structure(TypeConstructor::Int, vec![]);
    let wide = solver.structure(TypeConstructor::Tuple, vec![integer; 1000]);
    let mut deep = wide;
    for _ in 0..31 {
        deep = solver.structure(TypeConstructor::Array, vec![deep]);
    }
    let before = solver.mir.ty_slots.clone();
    let terms = solver.mir.type_terms.len();
    for slot in [wide, deep] {
        let text = solver.diagnostic_type(slot);
        assert!(text.contains('…'));
        assert!(text.len() < 2048);
    }
    assert_eq!(solver.mir.ty_slots, before);
    assert_eq!(solver.mir.type_terms.len(), terms);
}

#[test]
fn call_arity_diagnostics_use_the_solved_signature() {
    for (source, expected) in [
        (
            "def f = fn(a) {a}; export def bad = f(1, 2);",
            "call expects 1 arguments, found 2",
        ),
        (
            "def f: Fn(Int, Int) -> Int = fn(a, b) {a + b}; export def bad = 1 |> f;",
            "call expects 2 arguments, found 1",
        ),
        (
            "type Id = struct(Int); export def bad = Id();",
            "call expects 1 arguments, found 0",
        ),
        (
            "def f: Fn() -> Int = fn() {42}; export def bad = f(1);",
            "call expects 0 arguments, found 1",
        ),
    ] {
        let source = format!("{source} export def independent = 42;");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics.iter().any(|d| d.message == expected),
            "{}",
            mir.dump()
        );
        assert!(matches!(
            symbol_type(&mir, "independent"),
            TypeState::Known(_)
        ));
        assert!(mir.seal().is_err());
    }
}

#[test]
fn list_type_constructors_reject_variadic_and_non_list_arguments() {
    for (expression, message) in [
        ("Tuple(Int, String)", "expected 1 arguments, got 2"),
        ("Tuple()", "expected 1 arguments, got 0"),
        ("Tuple(Int)", "requires a static type list"),
        ("Func(Int, String)", "requires a static type list"),
        ("Func([Int])", "expected 2 arguments, got 1"),
        ("Func([Int], String, Bool)", "expected 2 arguments, got 3"),
    ] {
        let source = format!("type Invalid = {expression}; export def independent = 42;");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics.iter().any(|d| d.message.contains(message)),
            "{expression}\n{}",
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
        "import \"std/prelude\" {Tuple as Product}; type Bad = Product(Int, String); export {Bad};",
    )]);
    resolve(&mut mir);
    assert!(
        mir.diagnostics
            .iter()
            .any(|d| d.message.contains("expected 1 arguments, got 2"))
    );
    assert!(mir.seal().is_err());
    let mut mir = graph(&[(
        "@src/main",
        "type Pair = Tuple([Int, String]); type Empty = Tuple([]); type Function = Func([Int], String); export {Pair, Empty, Function};",
    )]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
}

#[test]
fn propagation_rejects_wrong_families_and_incompatible_error_evidence() {
    for source in [
        "export def bad = fn(value: Int) { value? };",
        "export def bad = fn(value: Option(Int)) -> Int { value? };",
        "export def bad = fn(a: Option(Int), b: Result(Int, String)) { let x = a?; let y = b?; Some(x + y) };",
        "export def bad = fn(value: Result(Int, String)) -> Result((), Int) { value?; fail!(\"tail\") };",
        "type Fake = enum {Some(Int), None}; export def bad = fn(value: Fake) -> Option(Int) { Some(value?) };",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(!mir.type_conflicts.is_empty(), "{source}\n{}", mir.dump());
    }
}

#[test]
fn syntax_recovery_keeps_independent_type_conflicts_without_a_fake_result_obligation() {
    let mut mir = module_resolve::resolve(
        vec![ModuleSpec {
            native: None,
            name: "main".into(),
            kind: ModuleKind::Source,
            implicit_imports: vec![],
        }],
        &["main".into()],
        |_, _| {
            Ok(
        "export def broken = match A { A 1, _ => 2 }; export def healthy = 42; export def bad = 1 + \"x\";".into()
    )
        },
    );
    assert!(
        mir.diagnostics
            .iter()
            .any(|d| d.message == "missing FatArrow")
    );
    crate::symbol_resolve::resolve(&mut mir);
    resolve(&mut mir);
    assert!(matches!(symbol_type(&mir, "healthy"), TypeState::Known(_)));
    assert!(matches!(symbol_type(&mir, "bad"), TypeState::Conflicted(_)));
    assert!(
        mir.diagnostics
            .iter()
            .any(|d| d.message.contains("type mismatch"))
    );
    assert!(
        !mir.diagnostics.iter().any(|d| d.message == "unknown type"),
        "{}",
        mir.dump()
    );
    assert!(mir.seal().is_err());
}

#[test]
fn retains_independent_conflicts_and_does_not_poison_intrinsic_types() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        def first: Int = "bad"; def good: Int = 42; def second: String = 1;
        export { first, good, second };
    "#,
    )]);
    resolve(&mut mir);
    assert!(matches!(
        symbol_type(&mir, "first"),
        TypeState::Conflicted(_)
    ));
    assert!(matches!(
        symbol_type(&mir, "second"),
        TypeState::Conflicted(_)
    ));
    assert_eq!(mir.type_conflicts.len(), 2, "{}", mir.dump());
    let TypeState::Known(good) = symbol_type(&mir, "good") else {
        panic!("{}", mir.dump());
    };
    assert_eq!(mir.types[good.index()].constructor, TypeConstructor::Int);
}

#[test]
fn unresolved_imports_are_inherited_without_new_type_diagnostics() {
    let mut mir = graph(&[
        (
            "@src/main",
            "import \"@src/other\" {missing}; export def bad = missing; export def good = 42;",
        ),
        ("@src/other", "export def present = 1;"),
    ]);
    let references = mir.resolve_slots.clone();
    let diagnostics = mir.diagnostics.len();
    resolve(&mut mir);
    assert_eq!(references, mir.resolve_slots);
    assert!(matches!(symbol_type(&mir, "bad"), TypeState::Conflicted(_)));
    assert!(
        mir.type_conflicts
            .iter()
            .any(|failure| matches!(failure.resolve_origin, Some(ResolveFailure::Symbol(_))))
    );
    assert!(matches!(symbol_type(&mir, "good"), TypeState::Known(_)));
    assert_eq!(mir.diagnostics.len(), diagnostics, "{}", mir.dump());
    assert!(mir.seal().is_err());
}

#[test]
fn unresolved_symbols_remain_authoritative_while_other_slots_are_solved() {
    let mut mir = graph(&[(
        "@src/main",
        "def missing = absent; def dependent = [missing.item]; export def good = 1;",
    )]);
    let references = mir.resolve_slots.clone();
    let diagnostics = mir.diagnostics.len();
    resolve(&mut mir);
    assert_eq!(references, mir.resolve_slots);
    let TypeState::Conflicted(failure) = symbol_type(&mir, "missing") else {
        panic!("{}", mir.dump());
    };
    assert!(matches!(
        mir.type_conflicts[failure.index()].resolve_origin,
        Some(ResolveFailure::Reference(_))
    ));
    assert_eq!(
        symbol_type(&mir, "dependent"),
        TypeState::Conflicted(failure)
    );
    assert!(matches!(symbol_type(&mir, "good"), TypeState::Known(_)));
    assert_eq!(
        mir.diagnostics.len(),
        diagnostics,
        "the type pass must not repeat the resolve failure: {}",
        mir.dump()
    );
    assert!(mir.seal().is_err());
}

#[test]
fn diagnostic_macros_accept_the_native_error_identity_without_evaluation() {
    let mut mir = graph(&[
        (
            "@src/main",
            r#"
        import "std/blame" { BlameError as Error };
        def error: Error = blame!("bad");
        export def abort: Fn(Error) -> Never = fn(e) { raise!(e) };
        export def warning: Option(Int) = warn!(error);
        export def text_warning: Option(String) = warn!("bad");
    "#,
        ),
        (
            "std/blame",
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/telora-core/modules/std/blame.telora"
            ))
            .expect("read test source"),
        ),
    ]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_conflicts.is_empty(), "{:?}", mir.type_conflicts);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
}
