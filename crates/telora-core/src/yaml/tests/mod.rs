use super::*;

fn parse(source: &str) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let mut sources = SourceDatabase::default();
    let id = sources.add("test.yaml", source);
    validate_yaml_registered(&sources, id)
}

#[test]
fn lowers_core_schema_collections_aliases_and_block_scalars() {
    let parsed = parse(
        "name: Telora\nenabled: true\nlegacy: yes\nwhen: 2026-08-04\nbase: &pair [1, 2]\ncopy: *pair\nitems:\n  - one\n  - {name: two, ok: false}\nliteral: |-\n  a\n  b\nfolded: >\n  hello\n  world\n",
    );
    assert!(parsed.is_ok(), "{parsed:?}");
    assert_eq!(
        crate::data_plan_test::render(&parsed.unwrap()),
        "{base: [1, 2], copy: [1, 2], enabled: 'True, folded: \"hello world\\n\", items: [\"one\", {name: \"two\", ok: 'False}], legacy: \"yes\", literal: \"a\\nb\", name: \"Telora\", when: \"2026-08-04\"}"
    );
}
#[test]
fn rejects_ambiguous_yaml_features() {
    for source in [
        "a: 1\na: 2\n",
        "value: !thing x\n",
        "base: &x [1]\nvalue: {<<: *x}\n",
        "---\na: 1\n---\nb: 2\n",
        "value: *later\nlater: &later 1\n",
    ] {
        let parsed = parse(source);
        assert!(parsed.is_err(), "accepted {source}");
    }
}

#[test]
fn expands_mapping_merges_and_decodes_the_standard_binary_tag() {
    let parsed = parse(
        "defaults: &defaults {a: 1, b: 2}\nitem:\n  <<: *defaults\n  b: 3\n  bytes: !!binary SGk=\nflow: {<<: *defaults, b: 4}\n",
    );
    assert!(parsed.is_ok(), "{parsed:?}");
    let value = crate::data_plan_test::render(&parsed.unwrap());
    assert!(
        value.contains("item: {a: 1, b: 3, bytes: b\"\\x48\\x69\"}"),
        "{value}"
    );
    assert!(value.contains("flow: {a: 1, b: 4}"), "{value}");

    for source in [
        "value: !!binary SGk\n",
        "value: !!binary SG==\n",
        "value: {tagged: !custom x}\n",
        "value: {1: text}\n",
        "base: &base {a: 1}\nvalue: {<<: [*base, 1]}\n",
        "a: &a {x: 1}\nb: &b {x: 2}\nvalue: {<<: [*a, *b]}\n",
        "root: &root {self: *root}\n",
    ] {
        let parsed = parse(source);
        assert!(parsed.is_err(), "accepted invalid YAML: {source}");
        assert!(!parsed.as_ref().unwrap_err().is_empty(), "{source}");
    }

    let mut sources = SourceDatabase::default();
    let source = "base: &base [0, 1]\nitems: [*base, *base]\n";
    let source_id = sources.add("aliases.yaml", source);
    let plan = validate_yaml_registered(&sources, source_id).unwrap();
    let stats = plan
        .enforce_limits(crate::DataLimits::default(), source.len())
        .unwrap();
    // Root Object + base Array and its two children + items Array, two
    // alias Arrays, and both pairs of aliased children.
    assert_eq!(stats.nodes, 11);
    assert!(
        plan.enforce_limits(
            crate::DataLimits {
                nodes: 10,
                ..crate::DataLimits::default()
            },
            source.len(),
        )
        .unwrap_err()
        .to_string()
        .contains("nodes")
    );
}

#[test]
fn rejects_non_finite_float_values() {
    for source in [
        "value: .inf\n",
        "value: -.inf\n",
        "value: .nan\n",
        "value: 1.0e9999\n",
    ] {
        let parsed = parse(source);
        assert!(parsed.is_err(), "accepted {source}");
        assert!(
            parsed.as_ref().unwrap_err()[0]
                .message
                .contains("must be finite")
        );
    }
}
