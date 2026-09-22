use super::*;

fn small_stack(mut mir: Mir) -> Mir {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            resolve(&mut mir);
            assert!(mir.symbols_closed);
            assert!(mir.resolution_facts.waiting.is_empty());
            mir
        })
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn long_reexport_chain_closes_without_recursive_calls() {
    let count = 1_500;
    let sources = (0..count)
        .map(|i| {
            (
                format!("@src/m{i}"),
                if i + 1 == count {
                    "pub def value: Int = 1;".into()
                } else {
                    format!("pub use crate::m{}::value;", i + 1)
                },
            )
        })
        .collect::<Vec<_>>();
    let sources = sources
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect::<Vec<_>>();
    let mir = small_stack(graph(&sources));
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let targets = mir
        .symbols
        .iter()
        .filter(|s| s.name == "value")
        .map(|s| format!("{:?}", s.resolution))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(targets.len(), 1);
}

#[test]
fn long_namespace_alias_chain_uses_mir_query_results() {
    let mut source = String::new();
    for i in 0..4_000 {
        source.push_str(&format!("def a{i} = a{};\n", i + 1));
    }
    source.push_str("use self::base as a4000; pub def result: Int = a0.value;");
    let mir = small_stack(graph(&[
        ("@src/main", &source),
        ("@src/base", "pub def value: Int = 1;"),
    ]));
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(
        mir.resolution_facts
            .namespaces
            .iter()
            .filter(|v| matches!(v, Some(Some(_))))
            .count()
            >= 4_001
    );
}

#[test]
fn long_constructor_alias_chain_uses_mir_query_results() {
    let mut source = String::new();
    for i in 0..4_000 {
        source.push_str(&format!("def C{i} = C{};\n", i + 1));
    }
    source.push_str("type End = enum { Tag }; def C4000 = End.Tag; pub def classify = fn(value) { match value { C0 => 1 } };");
    let mir = small_stack(graph(&[("@src/main", &source)]));
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(
        mir.resolution_facts
            .constructors
            .iter()
            .filter(|value| **value == Some(true))
            .count()
            >= 4_001
    );
}

fn fixtures(names: &[&str]) -> Mir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/mir-scheduling");
    let sources = names
        .iter()
        .map(|name| {
            (
                format!("@src/{name}"),
                std::fs::read_to_string(root.join(format!("{name}.telora"))).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    graph(
        &sources
            .iter()
            .map(|(n, s)| (n.as_str(), s.as_str()))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn ungrounded_cycle_finishes_without_discarding_independent_evidence() {
    let mut mir = small_stack(fixtures(&["cycle", "bridge"]));
    assert!(!mir.diagnostics.is_empty());
    assert!(
        mir.symbols
            .iter()
            .filter(|s| s.name == "lost")
            .all(|s| s.resolution == ResolveState::Unresolved)
    );
    crate::type_resolve::resolve(&mut mir);
    let independent = mir
        .symbols
        .iter()
        .position(|s| s.name == "independent" && matches!(s.kind, SymbolKind::Declaration(_)))
        .unwrap();
    assert!(matches!(
        mir.ty_slots[mir.symbol_types[independent].index()],
        TypeState::Known(_)
    ));
    assert!(mir.seal().is_err());
}

#[test]
fn diamond_reexports_do_not_create_a_spurious_ambiguity() {
    let mir = small_stack(fixtures(&["diamond", "left", "right", "base"]));
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
}

#[test]
fn ready_task_order_preserves_sealed_ids_and_diagnostics() {
    for names in [
        vec!["diamond", "left", "right", "base"],
        vec!["cycle", "bridge"],
    ] {
        let mut first = fixtures(&names);
        let mut second = fixtures(&names);
        resolve(&mut first);
        let mut scheduler = Scheduler::default();
        scheduler.newest_first = true;
        resolve_with_scheduler(&mut second, scheduler);
        assert_eq!(first.diagnostics, second.diagnostics);
        assert_eq!(first.resolve_slots, second.resolve_slots);
        crate::type_resolve::resolve(&mut first);
        crate::type_resolve::resolve(&mut second);
        assert_eq!(first.diagnostics, second.diagnostics);
        if names[0] == "diamond" {
            assert_eq!(
                format!("{:?}", first.seal().unwrap().types()),
                format!("{:?}", second.seal().unwrap().types())
            );
            assert_eq!(first.dump(), second.dump());
        }
    }
}
