use super::*;

fn source(case: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/language/src/check")
            .join(case)
            .join("testee.telora"),
    )
    .unwrap()
}

#[test]
fn finite_instance_cycles_close_and_growth_does_not_cascade() {
    for case in [
        "diag-instance-growth",
        "diag-instance-mutual-growth",
        "diag-instance-trait-growth",
    ] {
        let source = source(case);
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.contains("type expansion depth limit exceeded")),
            "{case}: {:?}",
            mir.diagnostics
        );
        assert!(
            !mir.diagnostics
                .iter()
                .any(|d| d.message.contains("no closed implementation evidence")),
            "{case}: {:?}",
            mir.diagnostics
        );
        assert!(mir.seal().is_err());
        assert!(
            mir.generic_instances.len() < 10000,
            "growth stopped at the type depth guard"
        );
        if case == "diag-instance-growth" {
            assert!(matches!(
                symbol_type(&mir, "independent"),
                TypeState::Known(_)
            ));
            assert!(
                mir.diagnostics
                    .iter()
                    .any(|d| d.message.contains("type mismatch"))
            );
            assert!(
                mir.diagnostics
                    .iter()
                    .any(|d| d.message.contains("does not implement Missing"))
            );
        }
    }
    let source = source("instance-convergence");
    let mut mir = graph(&[("@src/main", &source)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}"));
    let count = mir.generic_instances.len();
    assert_eq!(
        mir.generic_instances
            .iter()
            .filter(
                |instance| instance.concrete && mir.symbols[instance.symbol.index()].name == "same"
            )
            .count(),
        1
    );
    let mut again = graph(&[("@src/main", &source)]);
    resolve(&mut again);
    assert_eq!(count, again.generic_instances.len());
    assert_eq!(
        mir.generic_instances
            .iter()
            .map(|i| (&i.symbol, &i.arguments))
            .collect::<Vec<_>>(),
        again
            .generic_instances
            .iter()
            .map(|i| (&i.symbol, &i.arguments))
            .collect::<Vec<_>>()
    );
}

#[test]
fn module_inventory_order_preserves_shared_instance_identity() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/language/src/check/instance-order");
    let sources = ["testee", "first", "second", "shared"].map(|name| {
        (
            format!("@src/check/instance-order/{name}"),
            std::fs::read_to_string(directory.join(format!("{name}.telora"))).unwrap(),
        )
    });
    let mut inventory = sources
        .iter()
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let mut first = graph(&inventory);
    resolve(&mut first);
    first
        .seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", first.dump()));
    inventory[1..].reverse();
    let mut second = graph(&inventory);
    resolve(&mut second);
    second
        .seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", second.dump()));
    let keys = |mir: &Mir| {
        mir.generic_instances
            .iter()
            .map(|instance| (instance.symbol, instance.arguments.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&first), keys(&second));
    assert_eq!(
        first
            .generic_instances
            .iter()
            .filter(|instance| instance.concrete
                && first.symbols[instance.symbol.index()].name == "identity")
            .count(),
        1
    );
}

#[test]
fn finite_graph_larger_than_the_old_instance_cap_seals() {
    use std::fmt::Write;
    let mut source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/language/src/check/instance-convergence/large-template.telora"),
    )
    .unwrap();
    // Deterministic source generation avoids checking in thousands of copies.
    for index in 0..4100 {
        writeln!(source, "type Item{index} = struct {{}}; def instance{index}: Fn(Item{index}) -> Item{index} = identity@[Item{index}];").unwrap();
    }
    let mut mir = graph(&[("@src/main", &source)]);
    resolve(&mut mir);
    assert!(mir.generic_instances.len() > 4096);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}"));
}

#[test]
fn explicit_type_depth_and_tuple_width_are_guarded() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/type-expansion-limits");
    let depth = std::fs::read_to_string(fixtures.join("depth.telora")).unwrap();
    let tuple = std::fs::read_to_string(fixtures.join("tuple.telora")).unwrap();
    for (source, expected) in [
        (
            depth.replace(
                "{{TYPE}}",
                &format!("{}Int{}", "Array(".repeat(240), ")".repeat(240)),
            ),
            None,
        ),
        (
            depth.replace(
                "{{TYPE}}",
                &format!("{}Int{}", "Array(".repeat(257), ")".repeat(257)),
            ),
            Some("type expansion depth limit exceeded"),
        ),
        (
            tuple.replace("{{ITEMS}}", &vec!["Int"; 1024].join(", ")),
            None,
        ),
        (
            tuple.replace("{{ITEMS}}", &vec!["Int"; 1025].join(", ")),
            Some("tuple item limit exceeded"),
        ),
    ] {
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        if let Some(expected) = expected {
            let diagnostics = mir
                .diagnostics
                .iter()
                .filter(|d| d.message.contains(expected))
                .collect::<Vec<_>>();
            assert_eq!(diagnostics.len(), 1, "{:?}", mir.diagnostics);
            assert!(!diagnostics[0].labels.is_empty());
            assert!(mir.seal().is_err());
            assert!(mir.generic_instances.is_empty());
        } else {
            mir.seal().unwrap_or_else(|d| panic!("{d:?}"));
        }
    }
}

#[test]
fn finite_substitution_is_checked_before_more_expansion() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/type-expansion-limits");
    let nested =
        |leaf: &str, depth| format!("{}{leaf}{}", "Array(".repeat(depth), ")".repeat(depth));
    for case in ["layout", "instance"] {
        let template = std::fs::read_to_string(fixtures.join(format!("{case}.telora"))).unwrap();
        for (depth, accepted) in [(100, true), (200, false)] {
            let source = template
                .replace("{{TYPE}}", &nested("T", 80))
                .replace("{{INPUT}}", &nested("Int", depth));
            let mut mir = graph(&[("@src/main", &source)]);
            resolve(&mut mir);
            if accepted {
                mir.seal().unwrap_or_else(|d| panic!("{case}: {d:?}"));
            } else {
                assert!(
                    mir.diagnostics
                        .iter()
                        .any(|d| d.message.contains("type expansion depth limit exceeded")),
                    "{case}: {:?}",
                    mir.diagnostics
                );
                assert!(mir.seal().is_err());
            }
        }
    }
}

#[test]
fn concrete_instance_obligations_share_one_closed_evidence_graph() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests");
    let entry =
        std::fs::read_to_string(root.join("fixtures/type-expansion-limits/evidence.telora"))
            .unwrap();
    let provider =
        std::fs::read_to_string(root.join("language/src/test/instance-evidence/provider.telora"))
            .unwrap();
    let mut mir = graph(&[("@src/main", &entry), ("@src/provider", &provider)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}"));
    let keys = mir
        .evidence
        .iter()
        .map(|node| (node.subject, node.bound))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        keys.len(),
        mir.evidence.len(),
        "one stable node per obligation"
    );
    assert!(mir.evidence.iter().all(|node| node.state.is_proven()));
    assert!(
        mir.evidence
            .iter()
            .any(|node| !node.dependencies.is_empty())
    );
    for instance in mir
        .generic_instances
        .iter()
        .filter(|instance| instance.concrete)
    {
        for &(node, implementation) in &instance.implementations {
            assert!(matches!(
                mir.member_selections[node.index()],
                Some(MemberSelection::TraitMember { .. })
            ));
            assert!(mir.generic_instances[implementation.index()].concrete);
        }
    }
    assert!(
        mir.generic_instances
            .iter()
            .filter(|instance| instance.concrete)
            .any(|instance| !instance.implementations.is_empty())
    );
}
