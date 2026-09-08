#[test]
fn unit_contracts_preserve_empty_tuple_identity() {
    let analysis = analyze_source(
        "unit.telora",
        r#"
        type Empty = ();
        type Grouped = ((()));
        type Old = Tuple([]);
        type Alias = Unit;
        type Holder = struct { value: () };
        type Wrapped = struct(());
        type Choice = enum { Empty(()) };
        def zero: Fn() -> () = fn() {};
        def one: Fn(()) -> () = fn(value: ()) -> () { value };
        def nested: Fn(Array(())) -> () = fn(values) {};
        def generic: for(A) Fn(A) -> A = fn(value) { value };
        do {
            let a: () = zero();
            let b: Unit = one(a);
            let Unit = 42;
            let c: () = b;
            generic@[()](c)
        }
    "#,
    )
    .unwrap();
    let resolve = |mut id| {
        while let TypeNode::Ref(target) = analysis.types.node(id) {
            id = *target;
        }
        id
    };
    let unit = resolve(analysis.declared_types["Empty"]);
    for name in ["Grouped", "Old", "Alias"] {
        assert_eq!(
            analysis.types.node(unit),
            analysis.types.node(resolve(analysis.declared_types[name])),
            "{name}"
        );
    }
    assert_eq!(
        analysis.types.node(unit),
        analysis.types.node(resolve(analysis.result_type))
    );
    assert_ne!(unit, analysis.declared_types["Wrapped"]);
    assert_eq!(
        analysis.types.display(analysis.binding_types["zero"]),
        "Fn() -> ()"
    );
    assert_eq!(
        analysis.types.display(analysis.binding_types["one"]),
        "Fn(()) -> ()"
    );
}

#[test]
fn block_tails_and_statements_infer_normal_fallthrough() {
    for (body, expected) in [
        ("do {}", "()"),
        ("do { let a = 1; }", "()"),
        ("do { 1; }", "()"),
        ("do { 1; let a = 2; a; }", "()"),
        ("do { 1; let a = 2; a }", "Int"),
        ("do { 1; let (a, b) = (2, 3); a + b }", "Int"),
        ("if Bool.True { 1; } else {}", "()"),
        (
            "match Bool.True { Bool.True => do { 1; }, Bool.False => do {} }",
            "()",
        ),
        ("fn() { 1; }", "Fn() -> ()"),
    ] {
        let analysis = analyze_source("blocks.telora", body)
            .unwrap_or_else(|error| panic!("{body}: {error:?}"));
        assert_eq!(
            analysis.types.display(analysis.result_type),
            expected,
            "{body}"
        );
    }
}

#[test]
fn terminated_never_paths_do_not_become_unit() {
    for body in [
        "do { fail!(\"stopped\"); }",
        "do { let a = fail!(\"stopped\"); }",
        "do { let () = (); fail!(\"stopped\"); }",
        "do { let (a, b) = (1, 2); fail!(\"stopped\"); }",
        "do { panic!(\"stopped\"); }",
    ] {
        let analysis = analyze_source("never.telora", body).unwrap();
        assert_eq!(
            analysis.types.display(analysis.result_type),
            "Never",
            "{body}"
        );
    }
    for source in [
        "def f: Fn() -> Int = fn() { fail!(\"stopped\"); }; f",
        "def f: Fn() -> Int = fn() { return 42; }; f",
        "if Bool.True { fail!(\"stopped\"); } else { 42 }",
        "if Bool.True { 42 } else { fail!(\"stopped\"); }",
    ] {
        analyze_source("never-context.telora", source)
            .unwrap_or_else(|error| panic!("{source}: {error:?}"));
    }
}

#[test]
fn discarded_expressions_are_checked_and_metadata_arguments_stay_data() {
    for source in [
        "do { 1 + \"wrong\"; }",
        "def f: Fn() -> Int = fn() { 1; }; f",
        "do { 1; let a = ; }",
        "do { 1 2 }",
        "type Wrong = Array(()); 0",
        "1;",
    ] {
        assert!(
            analyze_source("invalid-unit.telora", source).is_err(),
            "{source}"
        );
    }
    analyze_source(
        "metadata-data.telora",
        r#"
        def accept: Fn(Unit) -> Type = fn(value) { Int };
        type Number = accept(());
        let value: Number = 1;
        value
    "#,
    )
    .unwrap();
}
