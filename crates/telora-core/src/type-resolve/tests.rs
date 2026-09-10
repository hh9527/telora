use super::*;
use crate::module_resolve::{self, ModuleSpec};

fn graph(sources: &[(&str, &str)]) -> Mir {
    let inventory = sources
        .iter()
        .map(|(name, _)| ModuleSpec {
            name: (*name).into(),
            kind: ModuleKind::Source,
            implicit_imports: vec![],
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
    crate::symbol_resolve::resolve(&mut mir, &["Int", "String"]);
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
    let mut solver = Solver {
        mir: &mut mir,
        revision: 0,
        tasks: vec![],
    };
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
