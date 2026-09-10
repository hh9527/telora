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
    let mir = module_resolve::resolve(inventory, &[sources[0].0.into()], |_, name| {
        Ok(sources
            .iter()
            .find(|(key, _)| *key == name)
            .unwrap()
            .1
            .into())
    });
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    mir
}

#[test]
fn closes_aliases_and_lexical_references_without_changing_syntax_or_types() {
    let mut mir = graph(&[
        (
            "@src/main",
            r#"import "./bridge" { renamed }; import "./base" as ns;
          export def result = fn(x: Int) { let before = x; let x = x; (x, before, renamed, ns.original) };"#,
        ),
        (
            "@src/bridge",
            r#"import "./base" { original }; export { original as renamed };"#,
        ),
        // A type error is irrelevant to symbol closure.
        ("@src/base", r#"export def original: Int = "wrong";"#),
    ]);
    let allocation = mir.hir.as_ptr();
    let slots = mir.ty_slots.len();
    resolve(&mut mir, &["Int"]);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert_eq!(allocation, mir.hir.as_ptr());
    assert_eq!(slots, mir.ty_slots.len());
    assert!(
        mir.ty_slots
            .iter()
            .all(|state| *state == TypeState::Unknown)
    );
    assert!(
        mir.resolve_slots
            .iter()
            .all(|state| matches!(state, ResolveState::Bound(_)))
    );
    let original = mir
        .symbols
        .iter()
        .position(|symbol| {
            symbol.name == "original" && symbol.kind == SymbolKind::Declaration(BindingKind::Def)
        })
        .unwrap();
    for node in &mir.hir {
        if matches!(&node.kind, HirKind::Variable(name) if name == "renamed")
            || matches!(node.kind, HirKind::Field)
        {
            assert_eq!(
                mir.resolve_slots[node.resolution.unwrap().index()],
                ResolveState::Bound(SymbolId(original as u32))
            );
        }
    }
    let x_targets = mir
        .hir
        .iter()
        .filter(|node| matches!(&node.kind, HirKind::Variable(name) if name == "x"))
        .map(|node| mir.resolve_slots[node.resolution.unwrap().index()].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        x_targets[0], x_targets[1],
        "let initializer sees preceding x"
    );
    assert_ne!(x_targets[1], x_targets[2], "later reference sees new x");
    assert!(mir.symbols_closed);
}

#[test]
fn records_unused_duplicates_but_only_used_wildcard_ambiguity() {
    let mut mir = graph(&[
        (
            "@src/main",
            r#"import "./a" *; import "./b" *;
          def duplicate = 1; def duplicate = 2;
          export def bad = shared;
          export def safe = do { let shared = 3; shared };"#,
        ),
        ("@src/a", "export def shared = 1; export def unused = 1;"),
        ("@src/b", "export def shared = 2; export def unused = 2;"),
    ]);
    resolve(&mut mir, &[]);
    assert_eq!(
        mir.resolve_conflicts.len(),
        2,
        "{:?}",
        mir.resolve_conflicts
    );
    assert!(
        matches!(&mir.resolve_conflicts[0], ResolveConflict::DuplicateDefinition { name, definitions }
        if name == "duplicate" && definitions.len() == 2 && definitions[0] != definitions[1])
    );
    assert!(
        matches!(&mir.resolve_conflicts[1], ResolveConflict::AmbiguousImport { name, candidates }
        if name == "shared" && candidates.len() == 2)
    );
    assert!(!mir.resolve_slots.contains(&ResolveState::Pending));
    assert!(mir.symbols_closed);
}

#[test]
fn keeps_unresolved_results_and_explicit_member_constraints() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        export def record = { a: 1 };
        export def field = record.a;
        export def broken = absent;
        export def pattern = fn(x) { match x { captured => captured } };
    "#,
    )]);
    resolve(&mut mir, &[]);
    assert!(!mir.resolve_slots.contains(&ResolveState::Pending));
    assert!(mir.resolve_slots.contains(&ResolveState::Unresolved));
    assert!(
        mir.resolve_slots
            .iter()
            .any(|state| matches!(state, ResolveState::Member { .. }))
    );
    assert!(
        mir.symbols
            .iter()
            .any(|symbol| symbol.name == "captured" && symbol.kind == SymbolKind::Pattern)
    );
    assert!(
        mir.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("absent"))
    );
    assert!(mir.dump().contains("symbol 0"));
}

#[test]
fn constructor_patterns_use_source_declarations_without_type_evaluation() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        type Wrap = struct(Int);
        export def unwrap = fn(value) { match value { Wrap(payload) => payload } };
    "#,
    )]);
    resolve(&mut mir, &["Int", "Struct"]);
    let wrap = mir
        .symbols
        .iter()
        .position(|symbol| {
            symbol.name == "Wrap" && symbol.kind == SymbolKind::Declaration(BindingKind::Type)
        })
        .unwrap();
    let reference = mir
        .hir
        .iter()
        .find(|node| matches!(&node.kind, HirKind::Variable(name) if name == "Wrap"))
        .unwrap();
    assert_eq!(
        mir.resolve_slots[reference.resolution.unwrap().index()],
        ResolveState::Bound(SymbolId(wrap as u32))
    );
    assert!(
        mir.hir
            .iter()
            .any(|node| matches!(&node.kind, HirKind::PatternName(name) if name == "payload"))
    );
    assert!(!mir.resolve_slots.contains(&ResolveState::Pending));
}
