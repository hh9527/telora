use super::*;
use crate::module_resolve::{self, ModuleSpec};

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
    assert_eq!(image.types[layout.members[0].unwrap().index()].constructor, TypeConstructor::Int);
    let branch = &image.types[layout.members[1].unwrap().index()];
    assert_eq!(branch.constructor, TypeConstructor::Array);
    assert_eq!(branch.arguments, [tree]);
    assert_eq!(layout.members[2], None);
    let body = &image.types[layout.body.index()];
    assert_eq!(body.constructor, TypeConstructor::Enum(vec![("Leaf".into(), true), ("Branch".into(), true), ("Empty".into(), false)]));
    assert_eq!(body.arguments, layout.members.iter().flatten().copied().collect::<Vec<_>>());
    let layout = image.layout(boxed).unwrap();
    assert_eq!(image.types[layout.body.index()].constructor, TypeConstructor::Record(vec!["value".into(), "children".into()]));
    assert_eq!(image.types[layout.body.index()].arguments, layout.members.iter().flatten().copied().collect::<Vec<_>>());
    assert_eq!(image.types[layout.members[0].unwrap().index()].constructor, TypeConstructor::String);
    let children = &image.types[layout.members[1].unwrap().index()];
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
fn an_unfilled_implicit_generic_argument_prevents_sealing() {
    let mut mir = graph(&[("@src/main", r#"
        def phantom: for(T) Fn() -> Int = fn() { 42 };
        export def answer = phantom();
    "#)]);
    resolve(&mut mir);
    assert!(mir.diagnostics.iter().any(|d| d.message == "unknown generic argument"));
    assert!(!mir.type_unknowns.is_empty());
    assert!(mir.seal().is_err());
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
fn unresolved_symbols_remain_authoritative_while_other_slots_are_solved() {
    let mut mir = graph(&[("@src/main", "def missing = absent; export def good = 1;")]);
    let references = mir.resolve_slots.clone();
    resolve(&mut mir);
    assert_eq!(references, mir.resolve_slots);
    assert_eq!(symbol_type(&mir, "missing"), TypeState::Unknown);
    assert!(matches!(symbol_type(&mir, "good"), TypeState::Known(_)));
    assert!(
        mir.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == "unknown type")
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
    )]);
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
            .any(|d| !d.labels.is_empty() && d.message.contains("incompatible types"))
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
            "incompatible types",
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
