use super::*;
use crate::module_resolve::{self, ModuleSpec};

#[test]
fn explicit_type_application_reports_contract_arity_and_inherits_resolution_failures() {
    for (declaration, application, expected) in [
        ("def identity: Fn(Int) -> Int = fn(value) {value};", "identity@[Int](1)", "monomorphic binding"),
        ("def identity: for(T) Fn(T) -> T = fn(value) {value};", "identity@[Int, String](1)", "expects 1 arguments, found 2"),
        ("def pair: for(A, B) Fn(A, B) -> A = fn(left, right) {left};", "pair@[Int](1, 2)", "expects 2 arguments, found 1"),
    ] {
        let source = format!("{declaration} export def bad = {application}; export def independent = 42;");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|d| d.message.contains(expected)), "{}", mir.dump());
        assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
        assert!(mir.seal().is_err());
    }
    let mut mir = graph(&[("@src/main", "export def bad = missing@[Int](1); export def independent = 42;")]);
    resolve(&mut mir);
    assert_eq!(mir.diagnostics.len(), 1, "{}", mir.dump());
    assert!(mir.diagnostics[0].message.contains("unknown binding"));
    let application = mir.hir.iter().position(|node| matches!(node.kind, HirKind::TypeApply)).unwrap();
    let TypeState::Conflicted(conflict) = mir.ty_slots[application] else { panic!("{}", mir.dump()) };
    assert!(mir.type_conflicts[conflict.index()].resolve_origin.is_some());
    assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
    assert!(mir.seal().is_err());
}

#[test]
fn native_value_shapes_are_diagnosed_before_codegen_and_enforced_by_seal() {
    let mut mir = graph(&[("@src/main", "native value: Int; export { value }; export def independent = 42;")]);
    resolve(&mut mir);
    assert!(mir.diagnostics.iter().any(|d| d.message == "native declaration requires a function signature"), "{}", mir.dump());
    assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
    mir.diagnostics.clear();
    assert!(mir.seal().is_err());
}

#[test]
fn seal_rejects_record_nodes_with_non_record_skeletons() {
    let mut mir = graph(&[("@src/main", "export def value = { item: 42 }; export def scalar = 42;")]);
    resolve(&mut mir);
    mir.seal().unwrap();
    let TypeState::Known(scalar) = symbol_type(&mir, "scalar") else { panic!("scalar type") };
    let record = mir.hir.iter().position(|node| matches!(node.kind, HirKind::Dict)).unwrap();
    mir.ty_slots[record] = TypeState::Known(scalar);
    assert!(mir.seal().is_err());
}

#[test]
fn patterns_report_missing_coverage_unreachable_arms_and_refutable_lets() {
    for (body, message) in [
        ("match value { Some(x) => x }", "missing None"),
        ("match value { None => 0, Some(1) => 1 }", "missing Some(_)"),
        ("match value { _ => 0, None => 1 }", "prior arms cover every value"),
        ("match value { None => 0, Some(_) => 1, _ => 2 }", "prior arms cover every value"),
        ("match value { Some(_) => 0, Some(1) => 1, None => 2 }", "prior arms cover Some"),
        ("match value { None => 0, None => 1, Some(_) => 2 }", "prior arms cover None"),
        ("match value { Some(x) if True => x, None if True => 0 }", "non-exhaustive match"),
        ("do { let (x, 1) = (1, 2); x }", "refutable let pattern"),
        ("do { let {x} = {x: 1} else { fail!(\"never\") }; x }", "let else pattern is irrefutable"),
        ("match 1 { {} => 0, _ => 1 }", "Struct pattern cannot match Int"),
        ("do { let dict: Dict(Int) = {x: 1}; match dict { {x} => x, _ => 0 } }", "Struct pattern cannot match Dict"),
    ] {
        let source = format!("export def read: Fn(Option(Int)) -> Int = fn(value) {{ {body} }};");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|diagnostic| diagnostic.message.contains(message)), "{source}\n{}", mir.dump());
        assert!(mir.seal().is_err(), "{source}");
    }
}

#[test]
fn static_type_uses_reject_value_metadata_and_ordinary_function_results() {
    for source in [
        "def metadata: TypeOf(Int) = Int.type; type Invalid = metadata; export def independent = 42;",
        "def choose: for(A) Fn(A) -> A = fn(value) {value}; type Invalid = choose(Int); export def independent = 42;",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|d| d.message.contains("metadata data cannot become a type")), "{}", mir.dump());
        assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
        assert!(mir.seal().is_err());
    }
    let mut mir = graph(&[
        ("@src/main", "import \"@src/types\" {metadata}; type Invalid = metadata; export def independent = 42;"),
        ("@src/types", "export def metadata: TypeOf(Int) = Int.type;"),
    ]);
    resolve(&mut mir);
    assert!(mir.diagnostics.iter().any(|d| d.message.contains("metadata data cannot become a type")));
    assert!(mir.seal().is_err());
    let mut mir = graph(&[
        ("@src/main", "import \"@src/types\" {Family as Renamed}; type Alias(T) = Renamed(T); export def answer: Alias(Int) = {value: 42};"),
        ("@src/types", "export type Family(T) = struct {value: T};"),
    ]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
}

#[test]
fn seal_requires_a_record_for_every_construction_check() {
    let mut mir = graph(&[("@src/main", "@check(fn(value) {Ok(())}) type Checked = struct(Int); export {Checked};")]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    mir.construction_checks.clear();
    assert!(mir.seal().is_err(), "a solved checker must not disappear before codegen");

    let mut mir = graph(&[("@src/main", r#"
        def verify: Fn(Int) -> Result((), Never) = fn(value) {Ok(())};
        type Choice = enum { @check(verify) Empty };
        export {Choice};
    "#)]);
    resolve(&mut mir);
    assert!(!mir.diagnostics.is_empty());
    assert!(mir.type_conflicts.is_empty(), "the invalid placement has a well-typed checker");
    assert!(mir.type_unknowns.is_empty(), "checker types are fully determined");
    mir.diagnostics.clear();
    assert!(mir.seal().is_err(), "unsupported check sites cannot bypass seal by clearing diagnostics");
}

#[test]
fn invalid_check_signatures_keep_the_original_conflict_and_contract_context() {
    for expression in ["fn(value) {True}", "fn(value) {Ok(value)}", "fn(value) {Err(\"bad\")}", "fn(value) {None}"] {
        let source = format!("@check({expression}) type Checked = struct(Int); export def independent = 42;");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        let diagnostics = mir.diagnostics.iter().filter(|d| d.message.starts_with("invalid @check function:")).collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 1, "{}", mir.dump());
        assert!(diagnostics[0].message.contains("Result((), BlameError)"));
        assert!(diagnostics[0].message.contains("cannot unify"));
        assert!(!diagnostics[0].labels.is_empty());
        assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
        assert!(mir.seal().is_err());
    }
    let mut mir = graph(&[("@src/main", "@check(missing) type Checked = struct(Int); export def independent = 42;")]);
    let count = mir.diagnostics.len();
    resolve(&mut mir);
    assert_eq!(mir.diagnostics.len(), count);
    assert!(!mir.diagnostics.iter().any(|d| d.message.starts_with("invalid @check")));
    assert!(mir.seal().is_err());
}

#[test]
fn conflicts_render_both_type_shapes_before_poisoning_the_slots() {
    let mut mir = graph(&[("@src/main", r#"
        def values: Array(Int) = [1];
        export def bad: Bool = values;
        export def independent = 42;
    "#)]);
    resolve(&mut mir);
    let conflict = mir.type_conflicts.iter().find(|conflict| conflict.message.contains("cannot unify")).unwrap();
    assert!(conflict.message.contains("Array<Int>"), "{}", conflict.message);
    assert!(conflict.message.contains("Bool"), "{}", conflict.message);
    assert!(!conflict.message.contains("SymbolId"));
    assert!(mir.diagnostics.iter().any(|d| d.message == conflict.message && d.labels.iter().any(|label| Some(label.location) == conflict.location)));
    assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
    assert!(mir.seal().is_err());
}

#[test]
fn diagnostic_type_rendering_is_bounded_and_read_only() {
    let mut mir = graph(&[("@src/main", "export def answer = 42;")]);
    let mut solver = Solver::new(&mut mir);
    let integer = solver.structure(TypeConstructor::Int, vec![]);
    let wide = solver.structure(TypeConstructor::Tuple, vec![integer; 1000]);
    let mut deep = wide;
    for _ in 0..31 { deep = solver.structure(TypeConstructor::Array, vec![deep]); }
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
fn non_callable_diagnostics_render_existing_type_evidence() {
    for (value, expected) in [
        ("1", "Int"), ("\"text\"", "String"), ("[1]", "Array<Int>"),
        ("{item: 1}", "{item: Int}"), ("Int", "TypeOf(Int)"),
    ] {
        let source = format!("export def bad = {{ let value = {value}; value(2) }}; export def independent = 42;");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        let message = format!("cannot call value of type {expected}");
        assert!(mir.diagnostics.iter().any(|d| d.message == message), "{}", mir.dump());
        assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
        assert!(mir.seal().is_err());
    }
}

#[test]
fn call_arity_diagnostics_use_the_solved_signature() {
    for (source, expected) in [
        ("def f = fn(a) {a}; export def bad = f(1, 2);", "call expects 1 arguments, found 2"),
        ("def f: Fn(Int, Int) -> Int = fn(a, b) {a + b}; export def bad = 1 |> f;", "call expects 2 arguments, found 1"),
        ("type Id = struct(Int); export def bad = Id();", "call expects 1 arguments, found 0"),
        ("def f: Fn() -> Int = fn() {42}; export def bad = f(1);", "call expects 0 arguments, found 1"),
    ] {
        let source = format!("{source} export def independent = 42;");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|d| d.message == expected), "{}", mir.dump());
        assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
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
        assert!(mir.diagnostics.iter().any(|d| d.message.contains(message)), "{expression}\n{}", mir.dump());
        assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
        assert!(mir.seal().is_err());
    }
    let mut mir = graph(&[("@src/main", "import \"std/prelude\" {Tuple as Product}; type Bad = Product(Int, String); export {Bad};")]);
    resolve(&mut mir);
    assert!(mir.diagnostics.iter().any(|d| d.message.contains("expected 1 arguments, got 2")));
    assert!(mir.seal().is_err());
    let mut mir = graph(&[("@src/main", "type Pair = Tuple([Int, String]); type Empty = Tuple([]); type Function = Func([Int], String); export {Pair, Empty, Function};")]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
}

#[test]
fn bottom_tails_wait_for_explicit_return_evidence_from_generalized_constructors() {
    let mut mir = graph(&[("@src/main", r#"
        export def choose = fn(flag) {
            if flag { return Ok("hi"); } else { return Err(2); }
        };
        export def answer = choose(True) == Ok("hi") && choose(False) == Err(2);
    "#)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let TypeState::Known(signature) = symbol_type(&mir, "choose") else { panic!("closed signature") };
    let result = *mir.types[signature.index()].arguments.last().unwrap();
    assert_eq!(mir.types[result.index()].constructor, TypeConstructor::Result);
    let arguments = &mir.types[result.index()].arguments;
    assert_eq!(mir.types[arguments[0].index()].constructor, TypeConstructor::String);
    assert_eq!(mir.types[arguments[1].index()].constructor, TypeConstructor::Int);
}

#[test]
fn function_value_aliases_do_not_become_enum_pattern_constructors() {
    for setup in [
        "def make: Fn(Int) -> Event = Event.Progress;",
        "import Event.{Progress}; def make: Fn(Int) -> Event = Progress;",
        "def first = Event.Progress; def make = first;",
    ] {
        let source = format!("type Event = enum {{Progress(Int), Finished}}; {setup} export def invalid = match Event.Progress(1) {{make(value) => value, _ => 0}}; export def independent = 42;");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|d| d.message.contains("constructor pattern requires a type declaration")), "{}", mir.dump());
        assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
        assert!(mir.seal().is_err());
    }
    let mut mir = graph(&[("@src/main", "type Event = enum {Progress(Int)}; import Event.{Progress as Advance}; export def read = match Advance(42) {Advance(value) => value};")]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
}

#[test]
fn patterns_close_alias_selections_and_preserve_nested_irrefutability() {
    let mut mir = graph(&[("@src/main", r#"
        import Bool.{True as Yes, False as No};
        type Wrapped(T) = struct(T);
        type Event(T) = enum {Empty, Value(T)};
        export def read: Fn(Event((Int, String))) -> Int = fn(value) {
            match value { Event.Empty => 0, Event.Value((x, _)) => x }
        };
        export def boolean: Fn(Bool) -> Int = fn(value) { match value { Yes => 1, No => 0 } };
        export def unwrap: Fn(Wrapped(Int)) -> Int = fn(value) { let Wrapped(x) = value; x };
    "#)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let pattern = mir.hir.iter().enumerate().find(|(index, node)| matches!(node.kind, HirKind::ConstructorPattern)
        && matches!(mir.member_selections[*index], Some(MemberSelection::EnumVariant { .. }))).unwrap().0;
    mir.member_selections[pattern] = Some(MemberSelection::EnumVariant { index: 999 });
    assert!(mir.seal().is_err());
}

#[test]
fn let_else_checks_divergence_without_overwriting_the_inferred_branch_type() {
    for branch in ["0", "()", "if flag { 0 } else { fail!(\"stop\") }"] {
        let source = format!("export def read: Fn(Option(Int), Bool) -> Int = fn(value, flag) {{ let Some(item) = value else {{ {branch} }}; item }}; export def independent = 42;");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|diagnostic| diagnostic.message == "let else branch must have type Never"), "{}", mir.dump());
        let node = mir.hir.iter().find(|node| matches!(node.kind, HirKind::LetElse)).unwrap();
        let branch = node.children.iter().find(|edge| edge.role == Role::Else).unwrap().node;
        let TypeState::Known(ty) = mir.ty_slots[branch.index()] else { panic!("branch retains solved evidence") };
        assert_ne!(mir.types[ty.index()].constructor, TypeConstructor::Never);
        let TypeState::Known(ty) = symbol_type(&mir, "independent") else { panic!("independent type") };
        assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::Int);
        mir.diagnostics.clear();
        assert!(mir.seal().is_err(), "seal must enforce divergence itself");
    }
    for branch in ["return 42;", "fail!(\"stop\")", "if flag { return 1; } else { return 2; }"] {
        let source = format!("export def read: Fn(Option(Int), Bool) -> Int = fn(value, flag) {{ let Some(item) = value else {{ {branch} }}; item }};");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    }
}

#[test]
fn decorators_on_aliases_are_diagnosed_and_cannot_be_silently_dropped() {
    for declaration in ["type Prop = Int;", "type Base = struct {value: Int}; @property(PropertyTarget.Type) type Prop = Base;"] {
        let source = if declaration.starts_with("type Prop") {
            format!("@property(PropertyTarget.Type) {declaration} export def independent = 42;")
        } else {
            format!("{declaration} export def independent = 42;")
        };
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|d| d.message.contains("aliases cannot own properties")), "{}", mir.dump());
        assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
        mir.diagnostics.clear();
        assert!(mir.seal().is_err(), "seal must reject unrecorded decorators independently of diagnostics");
    }
    let mut mir = graph(&[("@src/main", "@property(PropertyTarget.Type) type Mark = struct {value: Int}; export {Mark};")]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    mir.properties.clear();
    assert!(mir.seal().is_err(), "removing all property records must not erase the obligations");
}

#[test]
fn property_admission_links_capabilities_without_evaluating_targets() {
    let mut mir = graph(&[("@src/main", r#"
        import "std/prelude" {property as marker};
        def attach = marker;
        def choose: Fn() -> PropertyTarget = fn() { fail!("must not execute in type solving") };
        @attach(choose()) type Mark = struct { value: Int };
        def property: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: 42} };
        @property type Item = struct { value: Int };
        export def answer = Item.type;
    "#)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let index = mir.properties.iter().position(|record| matches!(record.admission, Some(PropertyAdmission::Require { .. }))).unwrap();
    let Some(PropertyAdmission::Require { capability, targets }) = mir.properties[index].admission else { unreachable!() };
    assert_eq!(targets, 3);
    assert_eq!(mir.properties[capability.index()].owner, mir.properties[index].property);
    assert_eq!(mir.properties[capability.index()].admission, Some(PropertyAdmission::Capability));
    mir.properties[index].admission = Some(PropertyAdmission::Capability);
    assert!(mir.seal().is_err());
}

#[test]
fn property_admission_rejects_missing_and_forged_capability_records() {
    for (source, expected) in [
        ("type Mark = struct {value: Int}; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: 42} }; @mark type Item = struct {value: Int}; export def answer = Item.type;", "no @property capability declaration"),
        ("def forged: Fn(Type, Option(PropertyAttr)) -> PropertyAttr = fn(owner, previous) { {bits: 63} }; @forged type Mark = struct {value: Int}; export def answer = Mark.type;", "reserved for @property capability records"),
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|diagnostic| diagnostic.message.contains(expected)), "{}", mir.dump());
        assert!(mir.seal().is_err());
    }
}

#[test]
fn generic_properties_close_provider_instances_before_sealing() {
    let mut mir = graph(&[("@src/main", r#"
        @property(PropertyTarget.Type) type Mark(T) = struct { witness: TypeOf(T) };
        def mark: for(T) Fn(TypeOf(T)) -> Fn(Type, Option(Mark(T))) -> Mark(T) = fn(witness) {
            fn(owner, previous) { {witness: witness} }
        };
        @mark(T.type) type Box(T) = struct { value: T };
        type Outer(T) = struct { value: Box(Array(T)) };
        export def answer = (Box(Int).type, Box(String).type, Outer(Int).type);
    "#)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let box_symbol = mir.symbols.iter().position(|symbol| symbol.name == "Box" && symbol.kind == SymbolKind::Declaration(BindingKind::Type)).unwrap();
    let records = mir.properties.iter().enumerate().filter(|(_, record)| record.concrete && mir.types[record.owner.index()].constructor == TypeConstructor::Nominal(SymbolId(box_symbol as u32))).map(|(index, _)| index).collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    for &index in &records {
        let record = &mir.properties[index];
        let instance = &mir.generic_instances[record.instance.unwrap().index()];
        assert!(instance.concrete);
        assert_eq!(instance.ty(record.providers[0]), Some(record.property));
        assert_eq!(mir.types[record.owner.index()].arguments, mir.types[record.property.index()].arguments);
    }
    let instance = mir.properties[records[0]].instance.take();
    assert!(mir.seal().is_err());
    mir.properties[records[0]].instance = instance;
    mir.properties.remove(records[0]);
    assert!(mir.seal().is_err());
}

#[test]
fn principal_schemes_preserve_quantified_bounds() {
    let mut mir = graph(&[("@src/main", r#"
        trait Named { name: Fn(Self) -> String };
        export def plain: for(T) Fn(T) -> T = fn(value) { value };
        export def first: for(T: Named) Fn(T) -> T = fn(value) { value };
        export def second: for(U: Named) Fn(U) -> U = fn(value) { value };
    "#)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let scheme = |name| {
        let index = mir.symbols.iter().position(|symbol| symbol.name == name && matches!(symbol.kind, SymbolKind::Declaration(_))).unwrap();
        mir.symbol_schemes[index].unwrap()
    };
    let first = scheme("first");
    assert_eq!(first, scheme("second"));
    assert_ne!(first, scheme("plain"));
    assert_eq!(mir.type_schemes[first.index()].bounds.len(), 1);
    mir.type_schemes[first.index()].bounds.clear();
    assert!(mir.seal().is_err());
}

#[test]
fn recursive_family_growth_is_a_static_conflict_but_permutations_and_resets_close() {
    for source in [
        "type Grow(A) = struct {next: Grow(Array(A))}; export {Grow}; export def independent = 42;",
        "type Left(A) = struct {next: Right(Array(A))}; type Right(B) = struct {next: Left(B)}; export {Left}; export def independent = 42;",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|d| d.message.contains("recursive type family expands")), "{}", mir.dump());
        assert!(!mir.type_conflicts.is_empty());
        assert!(mir.types.len() < 1000, "{} types: {:?}", mir.types.len(), mir.diagnostics);
        let symbol = mir.symbols.iter().position(|s| s.name == "independent" && matches!(s.kind, SymbolKind::Declaration(_))).unwrap();
        let TypeState::Known(ty) = mir.ty_slots[mir.symbol_types[symbol].index()] else { panic!("independent result stays solved") };
        assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::Int);
        assert!(mir.seal().is_err());
    }
    for source in [
        "type Swap(A, B) = struct {next: Swap(B, A)}; export {Swap};",
        "type Left(A) = struct {next: Right(Array(A))}; type Right(B) = struct {next: Left(Int)}; export {Left};",
        "type Tree(A) = struct {children: Array(Tree(A))}; export {Tree};",
        "type Wrapped(A) = struct {next: Wrapped(Unchecked(A))}; export {Wrapped};",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        assert!(mir.types.len() < 1000, "{} types", mir.types.len());
    }
}

#[test]
fn generic_references_distinguish_exported_schemes_and_call_instances() {
    let mut mir = graph(&[("@src/main", r#"
        export def identity: for(T) Fn(T) -> T = fn(value) { value };
        export def answer = identity(42);
        export def same = identity@[Int] == identity@[Int];
    "#)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let (scheme_node, scheme) = mir.generic_references.iter().enumerate()
        .find_map(|(node, reference)| match reference {
            Some(reference @ GenericReference::Scheme { .. }) => Some((node, *reference)),
            _ => None,
        }).expect("export preserves its quantified contract");
    let (instance_node, instance) = mir.generic_references.iter().enumerate()
        .find_map(|(node, reference)| match reference {
            Some(reference @ GenericReference::Instance(_)) => Some((node, *reference)),
            _ => None,
        }).expect("call selects its concrete instance");
    let GenericReference::Instance(id) = instance else { unreachable!() };
    assert!(mir.generic_instances[id.index()].concrete);
    mir.generic_references[scheme_node] = None;
    assert!(mir.seal().is_err(), "generic references must carry the type pass outcome");
    mir.generic_references[scheme_node] = Some(instance);
    assert!(mir.seal().is_err(), "a scheme export is not a call instance");
    mir.generic_references[scheme_node] = Some(scheme);
    mir.generic_references[instance_node] = Some(scheme);
    assert!(mir.seal().is_err(), "a concrete use cannot discard its arguments");
    mir.generic_references[instance_node] = Some(GenericReference::Instance(GenericInstanceId(u32::MAX)));
    assert!(mir.seal().is_err(), "references must close over admitted instances");
}

#[test]
fn principal_schemes_share_alpha_equivalent_contracts_but_not_function_symbols() {
    let mut mir = graph(&[("@src/main", r#"
        export def first: for(T) Fn(T) -> T = fn(value) { value };
        export def second: for(U) Fn(U) -> U = fn(value) { value };
        export def closed: for(T) Fn(T) -> Int = fn(value) { 42 };
        export def unused: for(T, U) Fn(T) -> T = fn(value) { value };
    "#)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let symbol = |name| mir.symbols.iter().position(|symbol| symbol.name == name && matches!(symbol.kind, SymbolKind::Declaration(_))).unwrap();
    assert_ne!(symbol("first"), symbol("second"));
    assert_eq!(mir.symbol_schemes[symbol("first")], mir.symbol_schemes[symbol("second")]);
    assert_ne!(mir.symbol_schemes[symbol("first")], mir.symbol_schemes[symbol("closed")]);
    assert_ne!(mir.symbol_schemes[symbol("first")], mir.symbol_schemes[symbol("unused")]);
    let scheme = &mir.type_schemes[mir.symbol_schemes[symbol("closed")].unwrap().index()];
    let SchemeNode::Apply { constructor: TypeConstructor::Function, arguments } = &mir.scheme_nodes[scheme.body.index()] else { panic!("function scheme") };
    assert_eq!(mir.scheme_nodes[arguments[0].index()], SchemeNode::Bound(0));
    let SchemeNode::Known(result) = mir.scheme_nodes[arguments[1].index()] else { panic!("closed result reuses TypeId") };
    assert_eq!(mir.types[result.index()].constructor, TypeConstructor::Int);
    let original = (mir.type_schemes.clone(), mir.scheme_nodes.clone(), mir.symbol_schemes.clone());
    mir.build_type_schemes();
    assert_eq!(original, (mir.type_schemes.clone(), mir.scheme_nodes.clone(), mir.symbol_schemes.clone()));
    let bound = mir.scheme_nodes.iter().position(|node| matches!(node, SchemeNode::Bound(0))).unwrap();
    mir.scheme_nodes[bound] = SchemeNode::Bound(999);
    assert!(mir.seal().is_err());
}

#[test]
fn interpreter_contracts_reject_invalid_witnesses_and_nested_parameters() {
    for (source, message) in [
        ("export def bad = interpreter!(fn(x) { True });", "directly annotated generic def"),
        ("export def bad: for(T) Fn(TypeOf(T), TypeOf(T)) -> Fn(T) -> Bool = interpreter!(fn(x) { True });", "unique witness"),
        ("export def bad: for(T, U) Fn(TypeOf(T)) -> Fn(T) -> Bool = interpreter!(fn(x) { True });", "missing a type parameter witness"),
        ("export def bad: for(T) Fn(Int) -> Fn(T) -> Bool = interpreter!(fn(x) { True });", "TypeOf witnesses"),
        ("export def bad: for(T) Fn(TypeOf(Int)) -> Fn(T) -> Bool = interpreter!(fn(x) { True });", "quantified type parameter"),
        ("export def bad: for(T) Fn(TypeOf(T)) -> Fn(Array(T)) -> Bool = interpreter!(fn(x) { True });", "cannot nest"),
        ("export def bad: for(T) Fn(TypeOf(T)) -> Fn(T) -> Array(T) = interpreter!(fn(x) { [] });", "result cannot contain"),
        ("def erased: Fn(Int) -> Bool = fn(x) { True }; export def bad: for(T) Fn(TypeOf(T)) -> Fn(T) -> Bool = interpreter!(erased);", "cannot unify"),
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(mir.diagnostics.iter().any(|diagnostic| diagnostic.message.contains(message)), "{source}\n{}", mir.dump());
    }
    for source in [
        "export def adapt: for(T) Fn(TypeOf(T)) -> Fn(Int) -> Bool = interpreter!(fn(x) { True });",
        "def erased: Fn(Dyn) -> Never = fn(x) { fail!(\"stop\") }; export def adapt: for(T) Fn(TypeOf(T)) -> Fn(T) -> Bool = interpreter!(erased);",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
    }
}

#[test]
fn interpreter_plans_derive_from_resolved_signatures_without_hidden_references() {
    let mut mir = graph(&[("@src/main", r#"
        type Witness(T) = TypeOf(T);
        def erased: Fn(String, Dyn, Bool, Dyn, Dyn) -> Bool = fn(text, a, flag, b, again) { flag };
        export def adapt: for(A, B) Fn(Witness(B), Witness(A)) -> Fn(String, A, Bool, B, A) -> Bool = interpreter!(erased);
        export def answer = adapt(Int.type, String.type)("text", "a", True, 42, "b");
    "#)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    assert!(!mir.hir.iter().any(|node| matches!(&node.kind, HirKind::Variable(name) if name.starts_with('\0'))));
    let index = mir.interpreter_plans.iter().position(Option::is_some).unwrap();
    assert_eq!(mir.interpreter_plans[index], Some(InterpreterPlan { witness_count: 2, parameters: vec![None, Some(1), None, Some(0), Some(1)] }));
    mir.interpreter_plans[index].as_mut().unwrap().parameters[1] = Some(0);
    assert!(mir.seal().is_err());
}

#[test]
fn empty_option_bottom_evidence_does_not_default_arbitrary_generic_results() {
    let mut mir = graph(&[("@src/main", "def inspect: for(T) Fn(Option(T)) -> Bool = fn(value) { match value { None => False, Some(_) => True } }; export def empty = Option.None; export def constrained: Option(Int) = None; export def observed = inspect(empty);")]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let TypeState::Known(empty) = symbol_type(&mir, "empty") else { panic!("empty") };
    let TypeState::Known(constrained) = symbol_type(&mir, "constrained") else { panic!("constrained") };
    assert_eq!(mir.types[empty.index()].constructor, TypeConstructor::Option);
    assert!(matches!(mir.types[mir.types[empty.index()].arguments[0].index()].constructor, TypeConstructor::Parameter(_)));
    assert_eq!(mir.types[mir.types[constrained.index()].arguments[0].index()].constructor, TypeConstructor::Int);
    assert!(mir.hir.iter().enumerate().any(|(index, node)| {
        matches!(&node.kind, HirKind::Variable(name) if name == "empty")
            && matches!(mir.ty_slots[index], TypeState::Known(instance)
                if mir.types[instance.index()].constructor == TypeConstructor::Option
                    && mir.types[mir.types[instance.index()].arguments[0].index()].constructor == TypeConstructor::Never)
    }), "{}", mir.dump());

    let mut mir = graph(&[("@src/main", "native unknown: for(T) Fn() -> Option(T); export def answer = unknown();")]);
    resolve(&mut mir);
    assert!(mir.seal().is_err());
    assert!(mir.diagnostics.iter().any(|d| d.message.starts_with("unknown generic argument")), "{}", mir.dump());
}

#[test]
fn newtype_reference_facets_preserve_declarations_and_reject_function_patterns() {
    let mut mir = graph(&[("@src/main", "type Id = struct(Int); def make: Fn(Int) -> Id = Id; export def value = make(42);")]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let TypeState::Known(declaration) = symbol_type(&mir, "Id") else { panic!("type declaration") };
    let TypeState::Known(constructor) = symbol_type(&mir, "make") else { panic!("constructor value") };
    assert_eq!(mir.types[declaration.index()].constructor, TypeConstructor::Meta);
    assert_eq!(mir.types[constructor.index()].constructor, TypeConstructor::Function);
    assert_eq!(mir.types[declaration.index()].arguments[0], mir.types[constructor.index()].arguments[1]);
    for constructor in ["def Make: Fn(Int) -> Id = fn(value) { Id(value) };", "def Make = Id;"] {
        let source = format!("type Id = struct(Int); {constructor} export def value = do {{ let Make(payload) = Id(42); payload }};");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err());
        assert!(mir.type_conflicts.iter().any(|conflict| conflict.message.contains("constructor pattern requires a type declaration")), "{}", mir.dump());
    }
}

#[test]
fn property_target_members_use_native_identity_and_ordinary_resolution() {
    let mut mir = graph(&[("@src/main", "import \"std/prelude\" {PropertyTarget as Target}; import Target.{Member as Both}; def target: Fn(Bool) -> Target = fn(enabled) { if enabled { Target.StructType } else { Target.EnumType } }; @property(target(True)) type Mark = struct {value: Int}; export def answer = (Target.Type, Target.StructType, Target.EnumType, Both, Target.Field, Target.Variant);")]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let TypeState::Known(tuple) = symbol_type(&mir, "answer") else { panic!("closed targets") };
    assert_eq!(mir.types[tuple.index()].arguments.len(), 6);
    for &target in &mir.types[tuple.index()].arguments {
        assert_eq!(mir.types[target.index()].constructor, TypeConstructor::PropertyTarget);
    }
    for source in [
        "type PropertyTarget = enum {Type}; @property(PropertyTarget.Type) type Mark = struct {value: Int}; export def answer = 42;",
        "import \"./other\" as property; @property(PropertyTarget.Type) type Mark = struct {value: Int}; export def answer = 42;",
    ] {
        let mut mir = graph(&[("@src/main", source), ("@src/other", "export def value = 42;")]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(!mir.type_conflicts.is_empty(), "{source}");
    }
}

#[test]
fn completed_record_construction_keeps_its_static_identity_deterministically() {
    let sources = [("@src/main", "type Item = struct {value: Int}; def item: Item = {value: 42}; def raw = {value: 42}; export def answer = [item] == [{value: 42}] && [raw] != [item];")];
    let mut first = graph(&sources);
    resolve(&mut first);
    first.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", first.dump()));
    let TypeState::Known(raw) = symbol_type(&first, "raw") else { panic!("closed raw value") };
    let TypeState::Known(item) = symbol_type(&first, "item") else { panic!("closed nominal value") };
    assert!(matches!(first.types[raw.index()].constructor, TypeConstructor::Record(_)));
    assert!(matches!(first.types[item.index()].constructor, TypeConstructor::Nominal(_)));
    let mut second = graph(&sources);
    resolve(&mut second);
    assert_eq!(first.dump(), second.dump());
}

#[test]
fn metadata_equality_does_not_unify_represented_types_or_narrow_a_join() {
    for source in [
        "export def answer = Int.type != String.type;",
        "def chosen = if True { Int.type } else { String.type }; export def answer = chosen == Int.type;",
        "type Choice = enum { Selected(Type), Empty }; def chosen = match Choice.Selected(Int.type) { Choice.Selected(value) => value, Choice.Empty => String.type }; export def answer = chosen == Int.type;",
        "def matches = fn(value) { value == Int.type }; export def answer = matches(String.type);",
        "def chosen = if True { Int.type } else { Int.type }; export def answer = chosen == String.type;",
        "def same = fn(left, right) { left == right }; export def answer = same(1, 1);",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let TypeState::Known(ty) = symbol_type(&mir, "answer") else { panic!("closed comparison") };
        assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::Bool);
        if source.contains("def chosen") {
            let TypeState::Known(ty) = symbol_type(&mir, "chosen") else { panic!("closed join") };
            let expected = if source.contains("else { Int.type }") { TypeConstructor::TypeOf } else { TypeConstructor::Type };
            assert_eq!(mir.types[ty.index()].constructor, expected, "{source}");
        }
        if source.contains("def matches") {
            let TypeState::Known(ty) = symbol_type(&mir, "matches") else { panic!("closed predicate") };
            let parameter = mir.types[ty.index()].arguments[0];
            assert_eq!(mir.types[parameter.index()].constructor, TypeConstructor::Type);
        }
    }
    for source in [
        "export def answer = Int.type == 1;",
        "export def answer = 1 == \"1\";",
        "type A = struct { x: Int }; type B = struct { x: Int }; def a: A = { x: 1 }; def b: B = { x: 1 }; export def answer = a == b;",
        "export def answer: TypeOf(Int) = if True { Int.type } else { String.type };",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(!mir.type_conflicts.is_empty(), "{source}");
    }
}

#[test]
fn imported_generic_constructor_uses_have_independent_type_arguments() {
    for source in [
        "type Message(T) = enum { Data(T), Empty }; import Message.{Data}; export def answer = (Data(1), Data@[String](\"text\"));",
        "import Option.{Some as Make}; export def answer = (Make(1), Make@[String](\"text\"));",
        "type Message(T) = enum { Data(T), Empty }; export def answer = (Message.Data(1), Message.Data@[String](\"text\"));",
        "import \"std/prelude\" { Option as Family }; export def answer = (Family.Some@[Int](1), Family.Some@[String](\"text\"));",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let TypeState::Known(ty) = symbol_type(&mir, "answer") else { panic!("closed tuple") };
        let items = &mir.types[ty.index()].arguments;
        assert_eq!(items.len(), 2);
        for (&item, expected) in items.iter().zip([TypeConstructor::Int, TypeConstructor::String]) {
            let argument = mir.types[item.index()].arguments[0];
            assert_eq!(mir.types[argument.index()].constructor, expected);
        }
    }
}

#[test]
fn constructor_alias_arguments_follow_family_order_and_reject_wrong_arity() {
    for source in [
        "import Result.{Err as Reject, Ok as Accept}; export def answer: (Result(Int, String), Result(Int, String)) = (Reject@[Int, String](\"text\"), Accept@[Int, String](1));",
        "type Outcome(T, E) = enum { Accept(T), Reject(E) }; import Outcome.{Reject}; export def answer: Outcome(Int, String) = Reject@[Int, String](\"text\");",
        "def flip: for(T, E) Fn(E, T) -> (T, E) = fn(e, t) { (t, e) }; def alias = flip; export def answer = alias@[Int, String](\"text\", 1);",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
    }
    for source in [
        "export def answer = Option.Some@[Int, String](1);",
        "type Message(T) = enum { Data(T) }; export def answer = Message.Data@[Int, String](1);",
        "export def answer = Option.Some@[String](1);",
        "type Message(T) = enum { Data(T) }; export def answer = Message(Int).Data@[String](\"text\");",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(!mir.type_conflicts.is_empty(), "{source}");
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
        let source = format!("def stop: Fn() -> Never = fn() {{ fail!(\"stop\") }}; export def answer = {expression};");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let TypeState::Known(ty) = symbol_type(&mir, "answer") else { panic!("closed array") };
        assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::Array);
        assert_eq!(mir.types[mir.types[ty.index()].arguments[0].index()].constructor, expected, "{source}");
        for node in mir.hir.iter().enumerate().filter_map(|(index, node)| (node.module == ModuleId(0) && matches!(node.kind, HirKind::Call)).then_some(HirId(index as u32))) {
            let TypeState::Known(ty) = mir.ty_slots[node.index()] else { continue; };
            assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::Never);
        }
    }
}

#[test]
fn metadata_joins_preserve_witnesses_or_widen_without_equating_represented_types() {
    for (expression, constructor) in [
        ("if True { Int.type } else { String.type }", TypeConstructor::Type),
        ("if True { Array(Int).type } else { Array(Int).type }", TypeConstructor::TypeOf),
        ("if True { Int.type } else { fail!(\"stop\") }", TypeConstructor::TypeOf),
        ("match 1 { 0 => Int.type, 1 => String.type, _ => Bool.type }", TypeConstructor::Type),
        ("if True { if False { Int.type } else { String.type } } else { Bool.type }", TypeConstructor::Type),
    ] {
        let source = format!("export def answer = {expression};");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let TypeState::Known(ty) = symbol_type(&mir, "answer") else { panic!("closed metadata") };
        assert_eq!(mir.types[ty.index()].constructor, constructor);
    }
    for source in [
        "export def bad: TypeOf(Int) = if True { Int.type } else { String.type };",
        "def broad: Type = Int.type; export def bad: TypeOf(Int) = broad;",
        "export def bad = if True { Int.type } else { 42 };",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(!mir.type_conflicts.is_empty(), "{source}");
    }
}

#[test]
fn callable_value_evidence_closes_nested_results_and_aliases_without_call_sites() {
    let mut mir = graph(&[("@src/main", "export def invoke = fn(factory) { factory()() }; export def alias = fn(callback, value) { let saved = callback; saved(value) }; export def compose = fn(outer, inner, value) { outer(inner(value)) };")]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let query = crate::mir_query::MirQuery::new(&mir);
    for (name, expected) in [
        ("invoke", "for(A) Fn(Fn() -> Fn() -> A) -> A"),
        ("alias", "for(A, B) Fn(Fn(A) -> B, A) -> B"),
        ("compose", "for(A, B, C) Fn(Fn(A) -> B, Fn(C) -> A, C) -> B"),
    ] {
        let symbol = mir.symbols.iter().position(|symbol| symbol.name == name && symbol.kind == SymbolKind::Declaration(BindingKind::Def)).unwrap();
        assert_eq!(query.symbol_signature(SymbolId(symbol as u32)).as_deref(), Some(expected));
    }
}

#[test]
fn implicit_schemes_fill_independent_reference_arguments_in_dependency_order() {
    for source in [
        "def identity = fn(value) { value }; def alias = identity; export def answer = (alias(1), alias(\"text\"));",
        "def identity = fn(value) { value }; export def answer = (identity(1), identity(\"text\"), identity@[Int](3));",
        "export def answer = { let identity = fn(value) { value }; (identity(1), identity(\"text\"), identity@[Int](3)) };",
        "def first = fn(value) { second(value) }; def second = fn(value) { value }; export def answer = (first(1), first(\"text\"));",
        "def second = fn(value) { value }; def first = fn(value) { second(value) }; export def answer = (first(1), first(\"text\"));",
        "def keep = fn(left: Int, right) { left }; export def answer = (keep(1, True), keep(2, \"text\"));",
        "def pair = fn(value) { (value, value) }; export def answer = (pair(1), pair(\"text\"));",
        "def outer = fn(value) { let keep = fn(other) { value }; (keep(True), keep(\"text\")) }; export def answer = (outer(1), outer(\"text\"));",
        "export def apply = fn(callback, value) { callback(value) };",
        "export def select = fn(condition, value) { if condition { value } else { value } };",
        "export def wrap = fn(value) { [value] };",
        "def mixed = fn(value, unused) { value + value }; export def answer = mixed(21, True);",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
    }
}

#[test]
fn implicit_schemes_publish_deterministic_signatures_without_renumbering_resolved_symbols() {
    let sources = [("@src/main", "export def apply = fn(callback, value) { callback(value) }; export def identity = fn(value) { value };")];
    let mut first = graph(&sources);
    let identities = first.symbols.iter().map(|symbol| (symbol.name.clone(), symbol.declarations.clone())).collect::<Vec<_>>();
    resolve(&mut first);
    first.seal().unwrap();
    for (symbol, identity) in first.symbols.iter().zip(identities) {
        assert_eq!((&symbol.name, &symbol.declarations), (&identity.0, &identity.1));
    }
    let query = crate::mir_query::MirQuery::new(&first);
    for (name, expected) in [("apply", "for(A, B) Fn(Fn(A) -> B, A) -> B"), ("identity", "for(A) Fn(A) -> A")] {
        let symbol = first.symbols.iter().position(|symbol| symbol.name == name && symbol.kind == SymbolKind::Declaration(BindingKind::Def)).unwrap();
        assert_eq!(query.symbol_signature(SymbolId(symbol as u32)).as_deref(), Some(expected));
    }
    let mut second = graph(&sources);
    resolve(&mut second);
    assert_eq!(first.dump(), second.dump());
}

#[test]
fn implicit_schemes_do_not_generalize_recursive_captured_or_constrained_slots() {
    for source in [
        "def recur = fn(value) { if True { value } else { recur(value) } }; export def answer = (recur(1), recur(\"text\"));",
        "def left = fn(value) { right(value) }; def right = fn(value) { left(value) }; export def answer = (left(1), left(\"text\"));",
        "def add = fn(left, right) { left + right }; export def answer = (add(1, 2), add(1.0, 2.0));",
        "def mixed = fn(value, unused) { value + value }; export def answer = (mixed(21, True), mixed(21, \"text\"));",
        "def before = fn(left, right) { left < right }; export def answer = (before(1, 2), before(\"a\", \"b\"));",
        "def before = fn(left, right) { left < right }; export def answer = before(True, False);",
        "def factory = fn() { { make: fn(value) { value } } }; def alias = factory().make; export def answer = (alias(1), alias(\"text\"));",
        "export def answer = { let values = []; let keep = fn(value) { [...values, value] }; (keep(1), keep(\"text\")) };",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}\n{}", mir.dump());
        assert!(!mir.type_conflicts.is_empty(), "{source}\n{}", mir.dump());
    }
}

#[test]
fn propagation_keeps_never_tail_error_evidence_and_infers_operand_from_return_context() {
    let mut mir = graph(&[("@src/main", "export def stopped = fn(value: Result(Int, String)) { value?; fail!(\"tail\") }; export def contextual = fn(value) -> Option(Int) { Some(value? + 1) };")]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let TypeState::Known(stopped) = symbol_type(&mir, "stopped") else { panic!("closed function") };
    let result = *mir.types[stopped.index()].arguments.last().unwrap();
    assert_eq!(mir.types[result.index()].constructor, TypeConstructor::Result);
    assert_eq!(mir.types[mir.types[result.index()].arguments[0].index()].constructor, TypeConstructor::Never);
    let TypeState::Known(contextual) = symbol_type(&mir, "contextual") else { panic!("closed function") };
    let input = mir.types[contextual.index()].arguments[0];
    assert_eq!(mir.types[input.index()].constructor, TypeConstructor::Option);
    assert_eq!(mir.types[mir.types[input.index()].arguments[0].index()].constructor, TypeConstructor::Int);
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
fn sequence_spreads_reject_wrong_containers_and_conflicting_element_evidence() {
    for source in [
        "export def bad = [...(1, 2)];",
        "export def bad = (...[1, 2], 3);",
        "type Wrapped = struct((Int, String)); export def bad = (...Wrapped((1, \"x\")), 3);",
        "export def bad = (...(1,), Int);",
        "def empty = []; def numbers = [...empty, 1]; export def bad = [...empty, \"x\"];",
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
        ("type Item = struct {x: Int}; def base: Item = {x: 1}; export def bad = base <~ {x: 2, ...base, x: 3};", "duplicate update field"),
        ("type Item = struct {x: Int}; def base: Item = {x: 1}; def dict: Dict(Int) = {x: 2}; export def bad: Item = {...base, ...dict};", "cannot mix Dict and named struct spreads"),
        ("type Item = struct {x: Int}; def base: Item = {x: 1}; export def bad = {...base};", "record spread requires a named struct target context"),
        ("def base: Dict(Int) = {x: 1}; export def bad = {...base, y: \"wrong\"};", "cannot unify"),
        ("type Item = struct {x: Int}; def base: Item = {x: 1}; export def bad = base <~ {extra: 1, ...base};", "unknown struct update field"),
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(mir.diagnostics.iter().any(|d| d.message.contains(message)), "{source}\n{}", mir.dump());
    }
}

#[test]
fn record_operations_reject_invalid_shapes_without_runtime_inference() {
    for (source, message) in [
        ("type Foo = struct {x: Int}; def source: Foo = {x: 1}; export def bad = source.{x};", "field projection requires a named struct target context"),
        ("type Foo = struct {x: Int}; def source: Dict(Int) = {x: 1}; export def bad: Foo = source.{x};", "field projection requires a named struct source"),
        ("type Foo = struct {x: Int}; def source: Foo = {x: 1}; export def bad: Foo = source.{missing as x};", "unknown projection source field"),
        ("type Foo = struct {x: Int}; def source: Foo = {x: 1}; export def bad: Foo = source.{x, x};", "duplicate projection destination"),
        ("type Foo = struct {x: Int}; def source: Foo = {x: 1}; export def bad = source <~ {missing: 1};", "unknown struct update field"),
        ("type Foo = struct {x: Int}; def source: Foo = {x: 1}; export def bad = source <~ {x: \"wrong\"};", "cannot unify"),
        ("def source: Dict(Int) = {x: 1}; export def bad = source <~ {x: 2};", "struct update requires a named struct operand"),
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(mir.diagnostics.iter().any(|d| d.message.contains(message)), "{source}\n{}", mir.dump());
    }
}

#[test]
fn syntax_recovery_keeps_independent_type_conflicts_without_a_fake_result_obligation() {
    let mut mir = module_resolve::resolve(vec![ModuleSpec {
        native: None, name: "main".into(), kind: ModuleKind::Source, implicit_imports: vec![],
    }], &["main".into()], |_, _| Ok(
        "export def broken = match A { A 1, _ => 2 }; export def healthy = 42; export def bad = 1 + \"x\";".into()
    ));
    assert!(mir.diagnostics.iter().any(|d| d.message == "missing FatArrow"));
    crate::symbol_resolve::resolve(&mut mir);
    resolve(&mut mir);
    assert!(matches!(symbol_type(&mir, "healthy"), TypeState::Known(_)));
    assert!(matches!(symbol_type(&mir, "bad"), TypeState::Conflicted(_)));
    assert!(mir.diagnostics.iter().any(|d| d.message.contains("cannot unify")));
    assert!(!mir.diagnostics.iter().any(|d| d.message == "unknown type"), "{}", mir.dump());
    assert!(mir.seal().is_err());
}

#[test]
fn positional_projection_requires_a_tuple_or_the_single_newtype_payload() {
    for source in [
        "type Count = struct(Int); export def invalid = Count(42).1;",
        "type Record = struct {value: Int}; def value: Record = {value: 42}; export def invalid = value.0;",
        "type Choice = enum {Item(Int)}; export def invalid = Choice.Item(42).0;",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|d| d.message.contains("has no item at index")), "{source}\n{}", mir.dump());
        assert!(mir.seal().is_err());
    }
}

#[test]
fn alias_cycles_are_conflicted_before_generic_expansion_without_blocking_other_types() {
    let mut mir = graph(&[("@src/main", r#"
        type Family(A) = (Concrete, A);
        type Concrete = Family(Int);
        type Direct(A) = Direct(A);
        type Node = struct { next: Option(Link) };
        type Link = Node;
        export def healthy = 42;
        export { Family, Concrete, Direct, Link };
    "#)]);
    resolve(&mut mir);
    assert!(mir.types_solved);
    for name in ["Family", "Concrete", "Direct"] {
        assert!(matches!(symbol_type(&mir, name), TypeState::Conflicted(_)), "{name}: {:?}", symbol_type(&mir, name));
    }
    assert!(matches!(symbol_type(&mir, "Link"), TypeState::Known(_)));
    assert!(matches!(symbol_type(&mir, "healthy"), TypeState::Known(_)));
    assert_eq!(mir.diagnostics.iter().filter(|d| d.message == "recursive type alias component").count(), 3);
    assert!(mir.ty_slots.len() < 10_000, "alias rejection must precede unbounded slot expansion");
}

#[test]
fn tuple_completion_normalizes_literal_slots_without_erasing_source_identity() {
    let mut mir = graph(&[("@src/main", r#"
        type Point = struct {x: Int};
        def candidate: Unchecked(Point) = {x: 42};
        export def pair: (Point, Int) = (candidate, 0);
        export def inferred = (1, "ok");
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    let TypeState::Known(candidate) = symbol_type(&mir, "candidate") else { panic!("candidate"); };
    let TypeState::Known(pair) = symbol_type(&mir, "pair") else { panic!("pair"); };
    assert_eq!(mir.types[pair.index()].constructor, TypeConstructor::Tuple);
    assert_eq!(mir.types[candidate.index()].constructor, TypeConstructor::Unchecked);
    assert_eq!(mir.types[candidate.index()].arguments, [mir.types[pair.index()].arguments[0]]);
    assert_eq!(mir.value_adjustments.iter().flatten().count(), 1);
    assert!(mir.types.iter().all(|ty| !matches!(ty.constructor, TypeConstructor::TupleLiteral | TypeConstructor::ArrayLiteral)));
}

#[test]
fn branch_completion_does_not_unify_candidate_and_checked_identity() {
    for expression in ["if True { candidate } else { good }", "match True { True => good, False => candidate }"] {
        let source = format!("type Point = struct {{x: Int}}; def candidate: Unchecked(Point) = {{x: 0}}; def good: Point = {{x: 42}}; export def answer = {expression};");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
        mir.seal().unwrap();
        let TypeState::Known(candidate) = symbol_type(&mir, "candidate") else { panic!("candidate"); };
        let TypeState::Known(answer) = symbol_type(&mir, "answer") else { panic!("answer"); };
        assert_eq!(mir.types[candidate.index()].constructor, TypeConstructor::Unchecked);
        assert_eq!(mir.types[candidate.index()].arguments, [answer]);
        assert_eq!(mir.value_adjustments.iter().flatten().count(), 1, "{}", mir.dump());
    }
}

#[test]
fn checked_return_closes_the_callable_signature_before_instantiation() {
    let mut mir = graph(&[("@src/main", "type Box(T) = struct {value: T}; def finish: for(T) Fn(Unchecked(Box(T))) -> Box(T) = fn(value) { value }; def candidate: Unchecked(Box(Int)) = {value: 42}; export def answer = finish(candidate);")]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    let closure = HirId(mir.hir.iter().position(|node| matches!(node.kind, HirKind::Closure)).unwrap() as u32);
    let boundary = mir.hir[closure.index()].children.iter().find(|edge| edge.role == Role::ReturnType).unwrap().node;
    assert!(mir.value_adjustments[closure.index()].is_some());
    let instance = mir.generic_instances.iter().find(|instance| mir.symbols[instance.symbol.index()].name == "finish" && instance.concrete).unwrap();
    let callable = instance.adjustment(closure).unwrap();
    assert_eq!(callable, instance.signature);
    let signature = &mir.types[callable.index()];
    assert_eq!(signature.constructor, TypeConstructor::Function);
    assert_eq!(signature.arguments.last().copied(), instance.adjustment(boundary));
    mir.value_adjustments[boundary.index()] = None;
    assert!(mir.seal().is_err(), "a checked signature without its return check must not seal");
}

#[test]
fn unchecked_identity_and_conversion_evidence_are_separate() {
    let mut mir = graph(&[("@src/main", r#"
        type Point = struct {x: Int};
        def candidate: Unchecked(Unchecked(Point)) = {x: 42};
        export def checked: Point = candidate;
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    let TypeState::Known(candidate) = symbol_type(&mir, "candidate") else { panic!("candidate"); };
    let TypeState::Known(checked) = symbol_type(&mir, "checked") else { panic!("checked"); };
    assert_ne!(candidate, checked);
    assert_eq!(mir.types[candidate.index()].constructor, TypeConstructor::Unchecked);
    assert_eq!(mir.types[candidate.index()].arguments, [checked]);
    assert_eq!(mir.value_adjustments.iter().flatten().count(), 1);
    let (_, image) = mir.seal().unwrap().into_parts();
    drop(mir);
    assert!(std::ptr::eq(image.layout(candidate).unwrap(), image.layout(checked).unwrap()));
    for source in [
        "export type Bad = Unchecked(Int);",
        "type Item = struct(Int); export type Bad = Unchecked(Item);",
        "type Item = enum {One}; export type Bad = Unchecked(Item);",
        "type A = struct {x: Int}; type B = struct {x: Int}; def candidate: Unchecked(A) = {x: 1}; export def wrong: B = candidate;",
        "type Wrap(T) = Unchecked(T); export type Bad = Wrap(Int);",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(!mir.diagnostics.is_empty(), "{source}\n{}", mir.dump());
        assert!(mir.seal().is_err());
    }
}

#[test]
fn generic_construction_checks_close_bodies_and_member_discovered_owners() {
    let mut mir = graph(&[("@src/main", r#"
        def identity: for(T) Fn(T) -> T = fn(value) { value };
        @check(fn(value) { let copied = identity(value.item); Ok(()) })
        type Item(T) = struct { item: T };
        type Envelope(T) = struct { child: Item(T) };
        export def first = Envelope(Int).type;
        export def second = Envelope(String).type;
    "#)]);
    let hir = mir.hir.as_ptr();
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    assert_eq!(hir, mir.hir.as_ptr());
    let checks = mir.construction_checks.iter().filter(|check| check.concrete).collect::<Vec<_>>();
    assert_eq!(checks.len(), 2, "{}", mir.dump());
    for check in checks {
        let instance = &mir.generic_instances[check.instance.unwrap().index()];
        assert!(instance.concrete);
        assert_eq!(instance.ty(check.checker), Some(check.signature));
        assert!(instance.references.iter().any(|(_, reference)| {
            let target = &mir.generic_instances[reference.index()];
            mir.symbols[target.symbol.index()].name == "identity" && target.concrete
        }));
        let input = mir.types[check.signature.index()].arguments[0];
        assert_eq!(mir.types[input.index()].arguments, [check.owner]);
    }
}

#[test]
fn construction_checks_are_separate_closed_contracts_without_execution() {
    let mut mir = graph(&[("@src/main", r#"
        @check(fn(value) { if value.port > 0 { Ok(()) } else { Err(blame!("positive port", value.port)) } })
        type Endpoint = struct { port: Int };
        @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive count", value)) } })
        type Count = struct(Int);
        type Event = enum { @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive payload", value)) } }) Item(Int), Empty };
        @check(fn(value) { fail!("must not execute during static solving") })
        type Deferred = struct { value: Int };
        export def answer = Endpoint.type;
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    assert_eq!(mir.construction_checks.len(), 4);
    assert!(mir.properties.iter().all(|property| !mir.construction_checks.iter().any(|check| check.owner == property.owner)));
    for check in &mir.construction_checks {
        let signature = &mir.types[check.signature.index()];
        assert_eq!(signature.constructor, TypeConstructor::Function);
        assert_eq!(signature.arguments.len(), 2);
        let result = &mir.types[signature.arguments[1].index()];
        assert_eq!(result.constructor, TypeConstructor::Result);
        assert_eq!(mir.types[result.arguments[0].index()].constructor, TypeConstructor::Tuple);
        assert!(mir.types[result.arguments[0].index()].arguments.is_empty());
        assert_eq!(mir.types[result.arguments[1].index()].constructor, TypeConstructor::Native(NativeTypeId::BLAME_ERROR));
        let input = &mir.types[signature.arguments[0].index()];
        let TypeConstructor::Nominal(symbol) = mir.types[check.owner.index()].constructor else { panic!("owner") };
        if ["Endpoint", "Deferred"].contains(&mir.symbols[symbol.index()].name.as_str()) {
            assert_eq!(input.constructor, TypeConstructor::Unchecked);
            assert_eq!(input.arguments, [check.owner]);
        } else { assert_eq!(input.constructor, TypeConstructor::Int); }
    }
}

#[test]
fn construction_checks_reject_wrong_boundaries_and_signatures() {
    for source in [
        "@check type Item = struct(Int);",
        "@check(fn(x) { Ok(()) }, fn(x) { Ok(()) }) type Item = struct(Int);",
        "@check(fn(x) { Ok(()) }) @check(fn(x) { Ok(()) }) type Item = struct(Int);",
        "@check(fn(x) { Ok(()) }) type Item = enum { One(Int) };",
        "type Item = enum { @check(fn(x) { Ok(()) }) Empty };",
        "type Item = struct { @check(fn(x) { Ok(()) }) value: Int };",
        "@check(fn(x) { 42 }) type Item = struct(Int);",
        "@check(fn(x) { Err(\"wrong error type\") }) type Item = struct(Int);",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(!mir.diagnostics.is_empty(), "{source}");
        assert!(mir.seal().is_err(), "{source}");
    }
}

#[test]
fn never_returning_provider_preserves_its_declared_nominal_result() {
    let mut mir = graph(&[("@src/main", r#"
        @property(PropertyTarget.Type) type Tag = struct { value: Int };
        def provider: Fn(Type, Option(Tag)) -> Tag = fn(owner, previous) { fail!("deferred") };
        @provider type Item = struct { value: Int };
        export def answer = Item.type;
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    mir.seal().unwrap();
    let TypeState::Known(signature) = symbol_type(&mir, "provider") else { panic!("provider signature"); };
    let result = *mir.types[signature.index()].arguments.last().unwrap();
    let TypeConstructor::Nominal(symbol) = mir.types[result.index()].constructor else { panic!("declared result lost"); };
    assert_eq!(mir.symbols[symbol.index()].name, "Tag");
    assert!(mir.properties.iter().any(|property| property.property == result));
}

#[test]
fn nominal_member_layouts_close_generic_and_recursive_type_references() {
    let mut mir = graph(&[("@src/main", r#"
        type Tree(T) = enum { Leaf(T), Branch(Array(Tree(T))), Empty };
        type Box(T) = struct { value: T, children: Array(Box(T)) };
        export def tree: Tree(Int) = Tree(Int).Leaf(42);
        export def boxed: Box(String) = { value: "ok", children: [] };
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    let TypeState::Known(tree) = symbol_type(&mir, "tree") else { panic!("tree"); };
    let TypeState::Known(boxed) = symbol_type(&mir, "boxed") else { panic!("boxed"); };
    let (_, image) = mir.seal().unwrap().into_parts();
    drop(mir);
    let layout = image.layout(tree).unwrap();
    assert_eq!(layout.members.len(), 3);
    assert_eq!(image.types[layout.members[2].unwrap().index()].constructor, TypeConstructor::Int);
    let branch = &image.types[layout.members[0].unwrap().index()];
    assert_eq!(branch.constructor, TypeConstructor::Array);
    assert_eq!(branch.arguments, [tree]);
    assert_eq!(layout.members[1], None);
    let body = &image.types[layout.body.index()];
    assert_eq!(body.constructor, TypeConstructor::Enum(vec![("Branch".into(), true), ("Empty".into(), false), ("Leaf".into(), true)]));
    assert_eq!(body.arguments, layout.members.iter().flatten().copied().collect::<Vec<_>>());
    let layout = image.layout(boxed).unwrap();
    assert_eq!(image.types[layout.body.index()].constructor, TypeConstructor::Record(vec!["children".into(), "value".into()]));
    assert_eq!(image.types[layout.body.index()].arguments, layout.members.iter().flatten().copied().collect::<Vec<_>>());
    assert_eq!(image.types[layout.members[1].unwrap().index()].constructor, TypeConstructor::String);
    let children = &image.types[layout.members[0].unwrap().index()];
    assert_eq!(children.constructor, TypeConstructor::Array);
    assert_eq!(children.arguments, [boxed]);
}

#[test]
fn generic_instances_close_body_types_and_transitive_references() {
    let mut mir = graph(&[("@src/main", r#"
        def metadata: for(T) Fn(T) -> TypeOf(Array(T)) = fn(value) { Array(T).type };
        def forward: for(U) Fn(U) -> TypeOf(Array(U)) = fn(value) { metadata(value) };
        export def number = forward(1);
        export def text = forward("ok");
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().expect("all instance slots close before codegen");
    for expected in [TypeConstructor::Int, TypeConstructor::String] {
        let outer = mir.generic_instances.iter().find(|instance| {
            mir.symbols[instance.symbol.index()].name == "forward"
                && mir.types[instance.arguments[0].1.index()].constructor == expected
        }).expect("concrete forward instance");
        let inner = outer.references.iter().find_map(|(_, id)| {
            let inner = &mir.generic_instances[id.index()];
            (mir.symbols[inner.symbol.index()].name == "metadata").then_some(inner)
        }).expect("reference to instantiated metadata body");
        assert_eq!(mir.types[inner.arguments[0].1.index()].constructor, expected);
        let represented = inner.types.iter().find_map(|(node, ty)| {
            matches!(mir.hir[node.index()].kind, HirKind::TypeMetadata)
                .then(|| mir.types[ty.index()].arguments[0])
        }).expect("metadata expression has an instance-specific TypeId");
        let array = &mir.types[represented.index()];
        assert_eq!(array.constructor, TypeConstructor::Array);
        assert_eq!(mir.types[array.arguments[0].index()].constructor, expected);
    }
}

#[test]
fn generic_instance_closure_reports_all_unsolved_arguments() {
    for (declaration, call, expected) in [
        ("def phantom: for(A, B, C) Fn() -> Int = fn() {42};", "phantom@[_ , _, _]()", vec!["A", "B", "C"]),
        ("def phantom: for(A, B, C) Fn() -> Int = fn() {42};", "phantom@[Int, _, _]()", vec!["B", "C"]),
        ("def accept: for(A, B) Fn(A) -> Int = fn(value) {42};", "accept@[String, _](1)", vec!["B"]),
    ] {
        let source = format!("{declaration} export def bad = {call}; export def independent = 42;");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        let messages = mir.diagnostics.iter().filter(|d| d.message.starts_with("unknown generic argument"))
            .map(|d| d.message.clone()).collect::<Vec<_>>();
        assert_eq!(messages, expected.iter().map(|name| format!("unknown generic argument for parameter {name:?}")).collect::<Vec<_>>(), "{}", mir.dump());
        for arguments in &mir.type_instances {
            for &(_, slot) in arguments {
                if mir.ty_slots[slot.index()] == TypeState::Unknown {
                    assert!(mir.type_unknowns.contains(&slot), "{}", mir.dump());
                }
            }
        }
        assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
        assert!(mir.seal().is_err());
    }
}

#[test]
fn an_unfilled_implicit_generic_argument_prevents_sealing() {
    let mut mir = graph(&[("@src/main", r#"
        def phantom: for(T) Fn() -> Int = fn() { 42 };
        export def answer = phantom();
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.iter().any(|d| d.message.starts_with("unknown generic argument")));
    assert!(!mir.type_unknowns.is_empty());
    assert!(mir.seal().is_err());
}

#[test]
fn generic_templates_are_static_and_value_uses_require_concrete_instances() {
    for expression in ["identity == identity", "[identity]", "(identity, 1)"] {
        let source = format!("export def identity: for(T) Fn(T) -> T = fn(value) {{ value }}; export def bad = {expression};");
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{expression}\n{}", mir.dump());
        assert!(!mir.type_unknowns.is_empty(), "{}", mir.dump());
    }
    let mut mir = graph(&[("@src/main", r#"
        export def unused: for(T) Fn(T) -> T = fn(value) { value };
        def identity: for(T) Fn(T) -> T = fn(value) { value };
        export def concrete: Fn(Int) -> Int = identity;
        export def answer = concrete(42);
    "#)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let unused = mir.symbols.iter().position(|symbol| symbol.name == "unused"
        && matches!(symbol.kind, SymbolKind::Declaration(_))).unwrap();
    assert!(!mir.generic_instances.iter().any(|instance| instance.symbol.index() == unused && instance.concrete));
    let sealed = mir.seal().unwrap();
    let template = ExecutionRoot { node: *mir.symbols[unused].declarations.last().unwrap(), instance: None };
    assert!(sealed.validate_execution_roots(&[template]).is_err(), "a static template cannot be an execution root");
    let answer = mir.symbols.iter().find(|symbol| symbol.name == "answer"
        && matches!(symbol.kind, SymbolKind::Declaration(_))).unwrap();
    let root = ExecutionRoot { node: *answer.declarations.last().unwrap(), instance: None };
    sealed.validate_execution_roots(&[root]).unwrap();
    let symbol = mir.hir_symbols[root.node.index()].unwrap();
    let executable = mir.seal_export(symbol).unwrap();
    assert_eq!(executable.root(), root.node);
    assert!(!executable.globals().iter().any(|symbol| symbol.index() == unused));
    assert!(!executable.instances().is_empty());
    assert!(executable.closure().nodes().windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(executable.closure().nodes(), mir.seal_export(symbol).unwrap().closure().nodes());
}

#[test]
fn recursive_generic_references_close_to_the_same_instance() {
    let mut mir = graph(&[("@src/main", r#"
        def repeat: for(T) Fn(T, Int) -> T = fn(value, n) {
            if n > 0 { repeat(value, n - 1) } else { value }
        };
        export def answer = repeat(42, 3);
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    let (index, instance) = mir.generic_instances.iter().enumerate().find(|(_, instance)| {
        mir.symbols[instance.symbol.index()].name == "repeat"
            && mir.types[instance.arguments[0].1.index()].constructor == TypeConstructor::Int
    }).unwrap();
    assert!(instance.references.iter().any(|(_, id)| id.index() == index));
}

#[test]
fn retains_per_reference_generic_arguments_for_codegen() {
    let source = [("@src/main", r#"
        def identity: for(T) Fn(T) -> T = fn(value) { value };
        def forward: for(U) Fn(U) -> U = fn(value) { identity(value) };
        export def number = identity(1);
        export def text = identity("ok");
        export def forwarded = forward(True);
    "#)];
    let mut mir = graph(&source);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    let identity = mir.symbols.iter().position(|s| s.name == "identity").unwrap();
    let parameter = mir.symbol_generics[identity][0];
    let arguments = mir.type_instances.iter().filter_map(|instance| {
        let &(p, slot) = instance.first()?;
        if p != parameter { return None; }
        assert_eq!(instance.len(), 1);
        let TypeState::Known(ty) = mir.ty_slots[slot.index()] else {
            panic!("generic argument was not normalized: {}", mir.dump());
        };
        Some(mir.types[ty.index()].constructor.clone())
    }).collect::<Vec<_>>();
    assert_eq!(arguments.len(), 3);
    assert!(arguments.contains(&TypeConstructor::Int));
    assert!(arguments.contains(&TypeConstructor::String));
    assert!(arguments.iter().any(|ty| matches!(ty, TypeConstructor::Parameter(_))));
    mir.seal().expect("closed generic argument graph");

    let mut repeated = graph(&source);
    resolve(&mut repeated);
    assert_eq!(mir.type_instances, repeated.type_instances);
    assert_eq!(mir.dump(), repeated.dump());
}

#[test]
fn phantom_generic_results_keep_their_argument_evidence_across_calls() {
    let mut mir = graph(&[("@src/main", r#"
        import "./lib" as lib;
        import "./app" {main as selected};
        def consume: for(T) Fn(lib.Phantom(T)) -> Int = fn(x) { x.n };
        export def answer = consume(selected);
    "#), ("@src/lib", r#"
        export type Phantom(T) = struct { n: Int };
        export def make: for(T) Fn(TypeOf(T), Int) -> Phantom(T) = fn(target, n) { {n} };
    "#), ("@src/app", r#"
        import "./lib" as lib;
        export def main = lib.make(Int.type, 42);
    "#)]);
    resolve(&mut mir);
    assert!(mir.type_unknowns.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
}

fn graph(sources: &[(&str, &str)]) -> Mir {
    let mut sources = sources.to_vec();
    if !sources.iter().any(|(name, _)| *name == "std/prelude") {
        sources.push((
            "std/prelude",
            include_str!("../../modules/std/prelude.telora"),
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

#[test]
fn generic_alias_application_uses_declared_parameters_including_unused_parameters() {
    let mut mir = graph(&[("@src/main", r#"
        type Pair(A, B) = Tuple([B, A]);
        type Keep(A, B) = Array(A);
        def pair: Pair(Int, String) = ("text", 42);
        export def number = pair.1;
        export def values: Keep(Int, String) = [1, 2];
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_unknowns.is_empty());
    let TypeState::Known(number) = symbol_type(&mir, "number") else { panic!("known number"); };
    assert_eq!(mir.types[number.index()].constructor, TypeConstructor::Int);
    let TypeState::Known(values) = symbol_type(&mir, "values") else { panic!("known values"); };
    assert_eq!(mir.types[values.index()].constructor, TypeConstructor::Array);
    assert_eq!(mir.types[mir.types[values.index()].arguments[0].index()].constructor, TypeConstructor::Int);
}

#[test]
fn solves_function_calls_across_modules_in_one_arena() {
    let mut mir = graph(&[
        (
            "@src/main",
            "import \"./math\" { inc }; export def answer = inc(41); export def pair = (answer, 2);",
        ),
        ("@src/math", "export def inc = fn(x) { x + 1 };"),
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
        ("@src/main", "import \"@src/other\" {missing}; export def bad = missing; export def good = 42;"),
        ("@src/other", "export def present = 1;"),
    ]);
    let references = mir.resolve_slots.clone();
    let diagnostics = mir.diagnostics.len();
    resolve(&mut mir);
    assert_eq!(references, mir.resolve_slots);
    assert!(matches!(symbol_type(&mir, "bad"), TypeState::Conflicted(_)));
    assert!(mir.type_conflicts.iter().any(|failure| matches!(failure.resolve_origin, Some(ResolveFailure::Symbol(_)))));
    assert!(matches!(symbol_type(&mir, "good"), TypeState::Known(_)));
    assert_eq!(mir.diagnostics.len(), diagnostics, "{}", mir.dump());
    assert!(mir.seal().is_err());
}

#[test]
fn unresolved_symbols_remain_authoritative_while_other_slots_are_solved() {
    let mut mir = graph(&[("@src/main", "def missing = absent; def dependent = [missing.item]; export def good = 1;")]);
    let references = mir.resolve_slots.clone();
    let diagnostics = mir.diagnostics.len();
    resolve(&mut mir);
    assert_eq!(references, mir.resolve_slots);
    let TypeState::Conflicted(failure) = symbol_type(&mir, "missing") else { panic!("{}", mir.dump()); };
    assert!(matches!(mir.type_conflicts[failure.index()].resolve_origin, Some(ResolveFailure::Reference(_))));
    assert_eq!(symbol_type(&mir, "dependent"), TypeState::Conflicted(failure));
    assert!(matches!(symbol_type(&mir, "good"), TypeState::Known(_)));
    assert_eq!(mir.diagnostics.len(), diagnostics, "the type pass must not repeat the resolve failure: {}", mir.dump());
    assert!(mir.seal().is_err());
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
fn function_tuple_and_unit_type_syntax_are_static_ir_operations() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        def pair: Fn(Int, String) -> (Int, String) = fn(x, y) { (x, y) };
        export def answer = pair(1, "ok"); export def unit: () = ();
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

#[test]
fn native_type_identity_survives_aliases_and_ordinary_names_can_be_shadowed() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        import "std/prelude" { Int as Number };
        type Int = String;
        type Array = String;
        export def number: Number = 42;
        export def text: Int = "ok";
        export def other: Array = "also text";
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    for (name, expected) in [
        ("number", TypeConstructor::Int),
        ("text", TypeConstructor::String),
        ("other", TypeConstructor::String),
    ] {
        let TypeState::Known(id) = symbol_type(&mir, name) else {
            panic!("{}", mir.dump());
        };
        assert_eq!(mir.types[id.index()].constructor, expected);
    }
}

#[test]
fn instantiates_generics_and_solves_recursive_nominal_skeletons() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        def id: for(T) Fn(T) -> T = fn(value) { value };
        type Pair(T) = struct { first: T, second: T };
        type Tree = enum { Leaf(Int), Branch((Tree, Tree)) };
        def pair: Pair(Int) = { first: id(1), second: id(2) };
        export def text = id("ok");
        export def number = pair.first;
        export def tree: Tree = Tree.Branch((Tree.Leaf(1), Tree.Leaf(2)));
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
    assert!(matches!(symbol_type(&mir, "tree"), TypeState::Known(_)));
    let TypeState::Known(text) = symbol_type(&mir, "text") else {
        panic!("{}", mir.dump());
    };
    assert_eq!(mir.types[text.index()].constructor, TypeConstructor::String);
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
        export def answer = read(Choice.Number(3));
        export def projection = (1, "ok").0;
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
}

#[test]
fn native_slot_identity_does_not_depend_on_the_declared_name() {
    let mut mir = graph(&[
        ("@src/main", "export def answer: Quantity = 42;"),
        (
            "std/prelude",
            "native type Quantity @4; export { Quantity };",
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
fn higher_order_native_calls_use_only_their_declared_generic_signature() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        native unrelated_name: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);
        native find: for(A) Fn(Array(A), Fn(A) -> Bool) -> Option(A);
        def ordinary: for(A, B) Fn(A, Fn(A) -> B) -> B = fn(value, callback) { callback(value) };
        def read: Fn(Array(Tuple([Int, String]))) -> Option(String) = fn(items) {
            match find(items, fn(item) { True }) {
                Some(pair) => Some(`value=\{pair.0}`),
                None => None,
            }
        };
        export def mapped = unrelated_name([1, 2, 3], fn(x) { x > 1 });
        export def explicit = unrelated_name@[Int, _]([1], fn(x) { "ok" });
        export def text = ordinary(1, fn(x) { "ok" });
        export def result = read([(1, "one")]);
    "#,
    ), ("std/fmt", r#"
        type Fmt = struct(String);
        trait Display { display: Fn(Self) -> Fmt };
        impl Display for Int { display: fn(value) { Fmt("int") } };
        export { Display };
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
    assert!(mir.type_conflicts.is_empty(), "{:?}", mir.type_conflicts);
    let TypeState::Known(mapped) = symbol_type(&mir, "mapped") else {
        panic!("{}", mir.dump());
    };
    let array = &mir.types[mapped.index()];
    assert_eq!(array.constructor, TypeConstructor::Array);
    assert_eq!(
        mir.types[array.arguments[0].index()].constructor,
        TypeConstructor::Bool
    );
}

#[test]
fn generic_call_conflicts_keep_the_use_site_and_other_instances_stay_independent() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);
        export def bad: Array(String) = map([1], fn(x) { x > 0 });
        export def good: Array(Int) = map(["ok"], fn(x) { 42 });
    "#,
    )]);
    resolve(&mut mir);
    assert!(!mir.type_conflicts.is_empty());
    assert!(mir.type_conflicts.iter().all(|c| c.location.is_some()));
    assert!(
        mir.diagnostics
            .iter()
            .any(|d| !d.labels.is_empty() && d.message.contains("cannot unify"))
    );
    let TypeState::Known(good) = symbol_type(&mir, "good") else {
        panic!("{}", mir.dump());
    };
    let array = &mir.types[good.index()];
    assert_eq!(array.constructor, TypeConstructor::Array);
    assert_eq!(
        mir.types[array.arguments[0].index()].constructor,
        TypeConstructor::Int
    );
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
        ("std/blame", include_str!("../../modules/std/blame.telora")),
    ]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_conflicts.is_empty(), "{:?}", mir.type_conflicts);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
}

#[test]
fn unregistered_native_slots_cannot_create_intrinsic_types() {
    for source in [
        "native type Forged @4; export def value: Forged = 1;",
        "native type Int @999; export def value: Int = 1;",
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
                            .is_some_and(|id| mir.modules[id.index()].name == "@src/main")
                )
                .all(|s| s.native_type.is_none())
        );
    }
}

#[test]
fn configured_decorators_use_factory_and_provider_signatures() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type)
        type Label = struct { value: String };
        def make_label: Fn(String) -> Fn(Type, Option(Label)) -> Label = fn(text) {
            fn(owner, previous) { { value: text } }
        };
        @make_label("name")
        type Item = struct { value: Int };
        export def item: Item = { value: 1 };
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_conflicts.is_empty(), "{:?}", mir.type_conflicts);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
}

#[test]
fn property_presence_proves_signature_bounds_without_running_providers() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type)
        type Label = struct { text: String };
        def label: Fn(Type, Option(Label)) -> Label = fn(owner, previous) { fail!("must not run") };
        @label @label
        type Item = struct { value: Int };
        native inspect: for(P, T: Property(P)) Fn(TypeOf(T), TypeOf(P)) -> P;
        def read: for(T: Property(Label)) Fn(TypeOf(T)) -> Label = fn(target) { inspect(target, Label.type) };
        export def answer = read(Item.type);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| matches!(b.state, BoundState::Assumed(_)))
    );
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| matches!(b.state, BoundState::Property(_)))
    );
    assert_eq!(
        mir.properties
            .iter()
            .filter(|p| p.providers.len() == 2)
            .count(),
        1
    );
}

#[test]
fn missing_property_bound_is_rejected_with_all_type_slots_known() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type)
        type Label = struct { text: String };
        native requires: for(T: Property(Label)) Fn(TypeOf(T)) -> Bool;
        export def answer = requires(Int.type);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| b.state == BoundState::Rejected)
    );
    assert!(
        mir.diagnostics
            .iter()
            .any(|d| d.message.contains("no static evidence") && !d.labels.is_empty())
    );
}

#[test]
fn trait_implementations_consume_property_evidence_and_lexical_bounds() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type)
        type Label = struct { text: String };
        def label: Fn(Type, Option(Label)) -> Label = fn(owner, previous) { fail!("not executed") };
        @label type Item = struct { value: Int };
        trait Named { name: Fn(Self) -> String };
        impl(T: Property(Label)) Named for T { name: fn(value) { "named" } };
        def name: for(T: Named) Fn(T) -> String = fn(value) { Named.name(value) };
        def item: Item = { value: 1 };
        export def answer = name(item);
    "#,
    )]);
    resolve(&mut mir);
    assert!(
        mir.diagnostics.is_empty(),
        "{:?}\n{}",
        mir.diagnostics,
        mir.dump()
    );
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| matches!(b.state, BoundState::Implementation(_)))
    );
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| matches!(b.state, BoundState::Assumed(_)))
    );
}

#[test]
fn trait_evidence_rejects_missing_cycles_overlap_and_wrong_member_signatures() {
    for (source, message) in [
        (
            "trait Show { show: Fn(Self) -> String }; export def answer = Show.show(1);",
            "no static evidence",
        ),
        (
            "trait Show { show: Fn(Self) -> String }; impl(T: Show) Show for T { show: fn(x) { \"cycle\" } }; export def answer = Show.show(1);",
            "no static evidence",
        ),
        (
            "trait Show { show: Fn(Self) -> String }; impl(T) Show for T { show: fn(x) { \"all\" } }; impl Show for Int { show: fn(x) { \"int\" } }; export { Show };",
            "overlapping trait implementations",
        ),
        (
            "trait Show { show: Fn(Self) -> String }; impl Show for Int { show: fn(x) { 42 } }; export { Show };",
            "cannot unify",
        ),
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics.iter().any(|d| d.message.contains(message)),
            "{message}: {:?}",
            mir.diagnostics
        );
    }
}

#[test]
fn member_properties_keep_separate_presence_records_and_structural_contexts() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Field) type Mark = struct { value: Int };
        type Ctx = struct { owner: Type, index: Int, name: String, ty: Type };
        def mark: Fn(Ctx, Option(Mark)) -> Mark = fn(ctx, previous) { { value: ctx.index } };
        type Item = struct { @mark first: Int, @mark second: String };
        export def item: Item = { first: 1, second: "ok" };
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(
        mir.properties
            .iter()
            .any(|p| p.site == PropertySite::Field(0))
    );
    assert!(
        mir.properties
            .iter()
            .any(|p| p.site == PropertySite::Field(1))
    );
}

#[test]
fn exact_impl_wins_over_property_blanket_without_specializing_function_names() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type) type Tag = struct { value: Int };
        def tag: Fn(Type, Option(Tag)) -> Tag = fn(owner, previous) { { value: 1 } };
        @tag type Item = struct { value: Int };
        trait Label { label: Fn(Self) -> String };
        impl(T: Property(Tag)) Label for T { label: fn(value) { "generic" } };
        impl Label for Item { label: fn(value) { "exact" } };
        impl Label for Int { label: fn(value) { "primitive" } };
        def item: Item = { value: 1 };
        export def answer = Label.label(item);
        export def number = Label.label(1);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    for requirement in &mir.bound_requirements {
        let BoundState::Implementation(symbol) = requirement.state else {
            continue;
        };
        assert!(mir.symbol_generics[symbol.index()].is_empty());
    }
}

#[test]
fn data_contract_resolves_the_exported_value_type_without_reading_data() {
    let mut mir = graph(&[
        (
            "@src/main",
            "import \"./payload.json\" { data }; export def answer = data;",
        ),
        ("@src/payload.json", "THIS IS NOT JSON OR TELORA"),
        ("std/value", "export type Value = Int;"),
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
