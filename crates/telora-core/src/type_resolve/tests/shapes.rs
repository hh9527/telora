use super::*;

#[test]
fn seal_rejects_record_nodes_with_non_record_skeletons() {
    let mut mir = graph(&[(
        "@src/main",
        "pub def value: Dict(Int) = { item: 42 }; pub def scalar: Int = 42;",
    )]);
    resolve(&mut mir);
    mir.seal().unwrap();
    let TypeState::Known(scalar) = symbol_type(&mir, "scalar") else {
        panic!("scalar type")
    };
    let record = mir
        .hir
        .iter()
        .position(|node| matches!(node.kind, HirKind::Dict))
        .unwrap();
    mir.ty_slots[record] = TypeState::Known(scalar);
    assert!(mir.seal().is_err());
}

#[test]
fn patterns_report_missing_coverage_unreachable_arms_and_refutable_lets() {
    for (body, message) in [
        ("match value { Some(x) => x }", "missing None"),
        ("match value { None => 0, Some(1) => 1 }", "missing Some(_)"),
        (
            "match value { _ => 0, None => 1 }",
            "prior arms cover every value",
        ),
        (
            "match value { None => 0, Some(_) => 1, _ => 2 }",
            "prior arms cover every value",
        ),
        (
            "match value { Some(_) => 0, Some(1) => 1, None => 2 }",
            "prior arms cover Some",
        ),
        (
            "match value { None => 0, None => 1, Some(_) => 2 }",
            "prior arms cover None",
        ),
        (
            "match value { Some(x) if True => x, None if True => 0 }",
            "non-exhaustive match",
        ),
        ("do { let (x, 1) = (1, 2); x }", "refutable let pattern"),
        (
            "do { let {x} = {x: 1} else { fail!(\"never\") }; x }",
            "let else pattern is irrefutable",
        ),
        (
            "match 1 { {} => 0, _ => 1 }",
            "Struct pattern cannot match Int",
        ),
        (
            "do { let dict: Dict(Int) = {x: 1}; match dict { {x} => x, _ => 0 } }",
            "Struct pattern cannot match Dict",
        ),
    ] {
        let source = format!("pub def read: Fn(Option(Int)) -> Int = fn(value) {{ {body} }};");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(message)),
            "{source}\n{}",
            mir.dump()
        );
        assert!(mir.seal().is_err(), "{source}");
    }
}

#[test]
fn static_type_uses_reject_value_metadata_and_ordinary_function_results() {
    for source in [
        "def metadata: TypeOf(Int) = Int.type; type Invalid = metadata; pub def independent: Int = 42;",
        "def choose: for(A) Fn(A) -> A = fn(value) {value}; type Invalid = choose(Int); pub def independent: Int = 42;",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.contains("metadata data cannot become a type")),
            "{}",
            mir.dump()
        );
        assert!(matches!(
            symbol_type(&mir, "independent"),
            TypeState::Known(_)
        ));
        assert!(mir.seal().is_err());
    }
    let mut mir = graph(&[
        (
            "@src/main",
            "use crate::types::{metadata}; type Invalid = metadata; pub def independent: Int = 42;",
        ),
        ("@src/types", "pub def metadata: TypeOf(Int) = Int.type;"),
    ]);
    resolve(&mut mir);
    assert!(
        mir.diagnostics
            .iter()
            .any(|d| d.message.contains("metadata data cannot become a type"))
    );
    assert!(mir.seal().is_err());
    let mut mir = graph(&[
        (
            "@src/main",
            "use crate::types::{Family as Renamed}; type Alias(T) = Renamed(T); pub def answer: Alias(Int) = {value: 42};",
        ),
        ("@src/types", "pub type Family(T) = struct {value: T};"),
    ]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
}

#[test]
fn newtype_reference_facets_preserve_declarations_and_reject_function_patterns() {
    let mut mir = graph(&[(
        "@src/main",
        "type Id = struct(Int); def make: Fn(Int) -> Id = Id; pub def value: Id = make(42);",
    )]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let TypeState::Known(declaration) = symbol_type(&mir, "Id") else {
        panic!("type declaration")
    };
    let TypeState::Known(constructor) = symbol_type(&mir, "make") else {
        panic!("constructor value")
    };
    assert_eq!(
        mir.types[declaration.index()].constructor,
        TypeConstructor::Meta
    );
    assert_eq!(
        mir.types[constructor.index()].constructor,
        TypeConstructor::Function
    );
    assert_eq!(
        mir.types[declaration.index()].arguments[0],
        mir.types[constructor.index()].arguments[1]
    );
    for constructor in [
        "def Make: Fn(Int) -> Id = fn(value) { Id(value) };",
        "def Make = Id;",
    ] {
        let source = format!(
            "type Id = struct(Int); {constructor} pub def value = do {{ let Make(payload) = Id(42); payload }};"
        );
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err());
        assert!(
            mir.type_conflicts.iter().any(|conflict| conflict
                .message
                .contains("constructor pattern requires a type declaration")),
            "{}",
            mir.dump()
        );
    }
}

#[test]
fn record_construction_uses_whole_graph_context_deterministically() {
    let sources = [(
        "@src/main",
        "type Item = struct {value: Int}; def item: Item = {value: 42}; pub def answer: Bool = do { let raw = {value: 42}; [item] == [{value: 42}] && [raw] != [item] };",
    )];
    let mut first = graph(&sources);
    resolve(&mut first);
    first
        .seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", first.dump()));
    let TypeState::Known(raw) = symbol_type(&first, "raw") else {
        panic!("closed raw value")
    };
    let TypeState::Known(item) = symbol_type(&first, "item") else {
        panic!("closed nominal value")
    };
    assert_eq!(raw, item);
    assert!(matches!(
        first.types[item.index()].constructor,
        TypeConstructor::Nominal(_)
    ));
    let mut second = graph(&sources);
    resolve(&mut second);
    assert_eq!(first.dump(), second.dump());
}

#[test]
fn metadata_equality_does_not_unify_represented_types_or_narrow_a_join() {
    for source in [
        "pub def answer: Bool = Int.type != String.type;",
        "pub def answer: Bool = do { def chosen = if True { Int.type } else { String.type }; chosen == Int.type };",
        "type Choice = enum { Selected(Type), Empty }; pub def answer: Bool = do { def chosen = match Choice.Selected(Int.type) { Choice.Selected(value) => value, Choice.Empty => String.type }; chosen == Int.type };",
        "pub def answer: Bool = do { def matches = fn(value) { value == Int.type }; matches(String.type) };",
        "pub def answer: Bool = do { def chosen = if True { Int.type } else { Int.type }; chosen == String.type };",
        "pub def answer: Bool = do { def same = fn(left, right) { left == right }; same(1, 1) };",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        mir.seal()
            .unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let TypeState::Known(ty) = symbol_type(&mir, "answer") else {
            panic!("closed comparison")
        };
        assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::Bool);
        if source.contains("def chosen") {
            let TypeState::Known(ty) = symbol_type(&mir, "chosen") else {
                panic!("closed join")
            };
            let expected = if source.contains("else { Int.type }") {
                TypeConstructor::TypeOf
            } else {
                TypeConstructor::Type
            };
            assert_eq!(mir.types[ty.index()].constructor, expected, "{source}");
        }
        if source.contains("def matches") {
            let TypeState::Known(ty) = symbol_type(&mir, "matches") else {
                panic!("closed predicate")
            };
            let parameter = mir.types[ty.index()].arguments[0];
            assert_eq!(
                mir.types[parameter.index()].constructor,
                TypeConstructor::Type
            );
        }
    }
    for source in [
        "pub def answer = Int.type == 1;",
        "pub def answer = 1 == \"1\";",
        "type A = struct { x: Int }; type B = struct { x: Int }; def a: A = { x: 1 }; def b: B = { x: 1 }; pub def answer = a == b;",
        "pub def answer: TypeOf(Int) = if True { Int.type } else { String.type };",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(!mir.type_conflicts.is_empty(), "{source}");
    }
}

#[test]
fn metadata_joins_preserve_witnesses_or_widen_without_equating_represented_types() {
    for (expression, constructor) in [
        (
            "if True { Int.type } else { String.type }",
            TypeConstructor::Type,
        ),
        (
            "if True { Array(Int).type } else { Array(Int).type }",
            TypeConstructor::TypeOf,
        ),
        (
            "if True { Int.type } else { fail!(\"stop\") }",
            TypeConstructor::TypeOf,
        ),
        (
            "match 1 { 0 => Int.type, 1 => String.type, _ => Bool.type }",
            TypeConstructor::Type,
        ),
        (
            "if True { if False { Int.type } else { String.type } } else { Bool.type }",
            TypeConstructor::Type,
        ),
    ] {
        let source = format!("pub def checked: Bool = do {{ let answer = {expression}; True }};");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        mir.seal()
            .unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let TypeState::Known(ty) = symbol_type(&mir, "answer") else {
            panic!("closed metadata")
        };
        assert_eq!(mir.types[ty.index()].constructor, constructor);
    }
    for source in [
        "pub def bad: TypeOf(Int) = if True { Int.type } else { String.type };",
        "def broad: Type = Int.type; pub def bad: TypeOf(Int) = broad;",
        "pub def bad = if True { Int.type } else { 42 };",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(!mir.type_conflicts.is_empty(), "{source}");
    }
}

#[test]
fn sequence_spreads_reject_wrong_containers_and_conflicting_element_evidence() {
    for source in [
        "pub def bad = [...(1, 2)];",
        "pub def bad = (...[1, 2], 3);",
        "type Wrapped = struct((Int, String)); pub def bad = (...Wrapped((1, \"x\")), 3);",
        "pub def bad = (...(1,), Int);",
        "def empty = []; def numbers = [...empty, 1]; pub def bad = [...empty, \"x\"];",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(!mir.type_conflicts.is_empty(), "{source}\n{}", mir.dump());
    }
}

#[test]
fn record_spreads_reject_incompatible_modes_and_duplicate_explicit_fields() {
    for (source, message) in [
        (
            "type Item = struct {x: Int}; def base: Item = {x: 1}; pub def bad = base <~ {x: 2, ...base, x: 3};",
            "duplicate update field",
        ),
        (
            "type Item = struct {x: Int}; def base: Item = {x: 1}; def dict: Dict(Int) = {x: 2}; pub def bad: Item = {...base, ...dict};",
            "cannot mix Dict and named struct spreads",
        ),
        (
            "type Item = struct {x: Int}; def base: Item = {x: 1}; pub def bad = {...base};",
            "record spread requires a named struct target context",
        ),
        (
            "def base: Dict(Int) = {x: 1}; pub def bad = {...base, y: \"wrong\"};",
            "type mismatch",
        ),
        (
            "type Item = struct {x: Int}; def base: Item = {x: 1}; pub def bad = base <~ {extra: 1, ...base};",
            "unknown struct update field",
        ),
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(
            mir.diagnostics.iter().any(|d| d.message.contains(message)),
            "{source}\n{}",
            mir.dump()
        );
    }
}

#[test]
fn record_operations_reject_invalid_shapes_without_runtime_inference() {
    for (source, message) in [
        (
            "type Foo = struct {x: Int}; def source: Foo = {x: 1}; pub def bad = source.{x};",
            "field projection requires a named struct target context",
        ),
        (
            "type Foo = struct {x: Int}; def source: Dict(Int) = {x: 1}; pub def bad: Foo = source.{x};",
            "field projection requires a named struct source",
        ),
        (
            "type Foo = struct {x: Int}; def source: Foo = {x: 1}; pub def bad: Foo = source.{missing as x};",
            "unknown projection source field",
        ),
        (
            "type Foo = struct {x: Int}; def source: Foo = {x: 1}; pub def bad: Foo = source.{x, x};",
            "duplicate projection destination",
        ),
        (
            "type Foo = struct {x: Int}; def source: Foo = {x: 1}; pub def bad = source <~ {missing: 1};",
            "unknown struct update field",
        ),
        (
            "type Foo = struct {x: Int}; def source: Foo = {x: 1}; pub def bad = source <~ {x: \"wrong\"};",
            "type mismatch",
        ),
        (
            "def source: Dict(Int) = {x: 1}; pub def bad = source <~ {x: 2};",
            "struct update requires a named struct operand",
        ),
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(
            mir.diagnostics.iter().any(|d| d.message.contains(message)),
            "{source}\n{}",
            mir.dump()
        );
    }
}

#[test]
fn positional_projection_requires_a_tuple_or_the_single_newtype_payload() {
    for source in [
        "type Count = struct(Int); pub def invalid = Count(42).1;",
        "type Record = struct {value: Int}; def value: Record = {value: 42}; pub def invalid = value.0;",
        "type Choice = enum {Item(Int)}; pub def invalid = Choice.Item(42).0;",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.contains("has no item at index")),
            "{source}\n{}",
            mir.dump()
        );
        assert!(mir.seal().is_err());
    }
}

#[test]
fn tuple_completion_normalizes_literal_slots_without_erasing_source_identity() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        type Point = struct {x: Int};
        def candidate: Unchecked(Point) = {x: 42};
        pub def pair: (Point, Int) = (candidate, 0);
        pub def checked: Bool = do { let inferred = (1, "ok"); True };
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    let TypeState::Known(candidate) = symbol_type(&mir, "candidate") else {
        panic!("candidate");
    };
    let TypeState::Known(pair) = symbol_type(&mir, "pair") else {
        panic!("pair");
    };
    assert_eq!(mir.types[pair.index()].constructor, TypeConstructor::Tuple);
    assert_eq!(
        mir.types[candidate.index()].constructor,
        TypeConstructor::Unchecked
    );
    assert_eq!(
        mir.types[candidate.index()].arguments,
        [mir.types[pair.index()].arguments[0]]
    );
    assert_eq!(mir.value_adjustments.iter().flatten().count(), 1);
    assert!(mir.types.iter().all(|ty| !matches!(
        ty.constructor,
        TypeConstructor::TupleLiteral | TypeConstructor::ArrayLiteral
    )));
}

#[test]
fn function_tuple_and_unit_type_syntax_are_static_ir_operations() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        def pair: Fn(Int, String) -> (Int, String) = fn(x, y) { (x, y) };
        pub def answer: (Int, String) = pair(1, "ok"); pub def unit: () = ();
    "#,
    )]);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let TypeState::Known(answer) = symbol_type(&mir, "answer") else {
        panic!("{}", mir.dump());
    };
    let tuple = &mir.types[answer.index()];
    assert_eq!(tuple.constructor, TypeConstructor::Tuple);
    assert_eq!(
        mir.types[tuple.arguments[0].index()].constructor,
        TypeConstructor::Int
    );
    assert_eq!(
        mir.types[tuple.arguments[1].index()].constructor,
        TypeConstructor::String
    );
    assert!(matches!(symbol_type(&mir, "unit"), TypeState::Known(_)));
}
