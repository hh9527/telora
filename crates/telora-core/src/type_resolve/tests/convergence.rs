use super::*;

fn source(case: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/language/src/check").join(case).join("testee.telora")).unwrap()
}

#[test]
fn finite_instance_cycles_close_and_growth_does_not_cascade() {
    for case in ["diag-instance-growth", "diag-instance-mutual-growth", "diag-instance-trait-growth"] {
        let source = source(case);
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(mir.diagnostics.iter().any(|d| d.message.contains("finite generic instance expansion")),
            "{case}: {:?}", mir.diagnostics);
        assert!(!mir.diagnostics.iter().any(|d| d.message.contains("no closed implementation evidence")),
            "{case}: {:?}", mir.diagnostics);
        assert!(mir.seal().is_err());
        assert!(mir.generic_instances.len() < 100, "growth stopped before enumeration");
        if case == "diag-instance-growth" {
            assert!(matches!(symbol_type(&mir, "independent"), TypeState::Known(_)));
            assert!(mir.diagnostics.iter().any(|d| d.message.contains("type mismatch")));
            assert!(mir.diagnostics.iter().any(|d| d.message.contains("does not implement Missing")));
        }
    }
    let source = source("instance-convergence");
    let mut mir = graph(&[("@src/main", &source)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}"));
    let count = mir.generic_instances.len();
    assert_eq!(mir.generic_instances.iter().filter(|instance|
        instance.concrete && mir.symbols[instance.symbol.index()].name == "same").count(), 1);
    let mut again = graph(&[("@src/main", &source)]);
    resolve(&mut again);
    assert_eq!(count, again.generic_instances.len());
    assert_eq!(mir.generic_instances.iter().map(|i| (&i.symbol, &i.arguments)).collect::<Vec<_>>(),
        again.generic_instances.iter().map(|i| (&i.symbol, &i.arguments)).collect::<Vec<_>>());
}

#[test]
fn module_inventory_order_preserves_shared_instance_identity() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/language/src/check/instance-order");
    let sources = ["testee", "first", "second", "shared"].map(|name| (
        format!("@src/check/instance-order/{name}"),
        std::fs::read_to_string(directory.join(format!("{name}.telora"))).unwrap(),
    ));
    let mut inventory = sources.iter().map(|(name, source)| (name.as_str(), source.as_str())).collect::<Vec<_>>();
    let mut first = graph(&inventory);
    resolve(&mut first);
    first.seal().unwrap_or_else(|d| panic!("{d:?}"));
    inventory[1..].reverse();
    let mut second = graph(&inventory);
    resolve(&mut second);
    second.seal().unwrap_or_else(|d| panic!("{d:?}"));
    let keys = |mir: &Mir| mir.generic_instances.iter().map(|instance|
        (instance.symbol, instance.arguments.clone())).collect::<Vec<_>>();
    assert_eq!(keys(&first), keys(&second));
    assert_eq!(first.generic_instances.iter().filter(|instance|
        instance.concrete && first.symbols[instance.symbol.index()].name == "identity").count(), 1);
}

#[test]
fn finite_graph_larger_than_the_old_instance_cap_seals() {
    use std::fmt::Write;
    let mut source = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/language/src/check/instance-convergence/large-template.telora")).unwrap();
    // Deterministic source generation avoids checking in thousands of copies.
    for index in 0..4100 {
        writeln!(source, "type Item{index} = struct {{}}; def instance{index} = identity@[Item{index}];").unwrap();
    }
    let mut mir = graph(&[("@src/main", &source)]);
    resolve(&mut mir);
    assert!(mir.generic_instances.len() > 4096);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}"));
}
