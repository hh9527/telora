use super::*;

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
    mir.seal().unwrap();
    let image = crate::type_image::TypeImage::from_mir(&mir).unwrap();
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
