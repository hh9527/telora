use super::*;
use alloc::{string::ToString, vec::Vec};

fn parse(source: &str) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let mut sources = SourceDatabase::default();
    let id = sources.add("test.json", source);
    validate_json_registered(&sources, id)
}

#[test]
fn lowers_all_json_categories_directly_from_cst() {
    let value = parse(r#"{"a":null,"b":true,"c":false,"d":-2,"e":1.5,"f":["x"]}"#).unwrap();
    assert_eq!(
        crate::data_plan_test::render(&value),
        "{a: 'None, b: 'True, c: 'False, d: -2, e: 1.5, f: [\"x\"]}"
    );
}

#[test]
fn decodes_unicode_surrogate_pairs() {
    assert_eq!(
        crate::data_plan_test::render(&parse(r#""\uD83D\uDE00""#).unwrap()),
        "\"😀\""
    );
}

#[test]
fn reports_precise_duplicate_and_number_ranges() {
    let mut sources = SourceDatabase::default();
    let duplicate = sources.add("duplicate.json", r#"{"a":1,"a":2}"#);
    let parsed = validate_json_registered(&sources, duplicate).unwrap_err();
    assert_eq!(parsed[0].labels[0].location.range(), 7..10);
    assert_eq!(parsed[0].labels[1].location.range(), 1..4);

    let large = sources.add("large.json", "9223372036854775808");
    let parsed = validate_json_registered(&sources, large).unwrap_err();
    assert!(parsed[0].message.contains("outside the i64 range"));
    assert_eq!(parsed[0].labels[0].location.range(), 0..19);

    let non_finite = sources.add("non-finite.json", "1e9999");
    let parsed = validate_json_registered(&sources, non_finite).unwrap_err();
    assert!(parsed[0].message.contains("must be finite"));
}

#[test]
fn validated_plan_accounts_the_complete_logical_graph_before_materialization() {
    let source = r#"{"a":"xyz","b":[1,true]}"#;
    let mut sources = SourceDatabase::default();
    let source_id = sources.add("quota.json", source);
    let plan = validate_json_registered(&sources, source_id).unwrap();

    assert_eq!(
        plan.enforce_limits(crate::DataLimits::default(), source.len())
            .unwrap(),
        DataStats {
            file_size: source.len(),
            nodes: 5,
            depth: 3,
            container_size: 2,
            bytes_len: 0,
            string_len: 3,
            payloads_bytes: 5,
        }
    );
}

#[test]
fn validated_plan_enforces_each_structural_limit() {
    let source = r#"{"a":"xyz","b":[1,true]}"#;
    let mut sources = SourceDatabase::default();
    let source_id = sources.add("limits.json", source);
    let plan = validate_json_registered(&sources, source_id).unwrap();
    let defaults = crate::DataLimits::default();

    for (limits, name) in [
        (
            crate::DataLimits {
                file_size: source.len() - 1,
                ..defaults
            },
            "file_size",
        ),
        (
            crate::DataLimits {
                nodes: 4,
                ..defaults
            },
            "nodes",
        ),
        (
            crate::DataLimits {
                depth: 2,
                ..defaults
            },
            "depth",
        ),
        (
            crate::DataLimits {
                container_size: 1,
                ..defaults
            },
            "container_size",
        ),
        (
            crate::DataLimits {
                string_len: 2,
                ..defaults
            },
            "string_len",
        ),
        (
            crate::DataLimits {
                payloads_bytes: 4,
                ..defaults
            },
            "payloads_bytes",
        ),
    ] {
        assert!(
            plan.enforce_limits(limits, source.len())
                .unwrap_err()
                .to_string()
                .contains(name),
            "{name}"
        );
    }

    let location = Location::from_usize(source_id, 0..source.len()).unwrap();
    let mut bytes = ValidatedDataPlan::default();
    let root = bytes.scalar(DataScalar::Bytes(vec![0; 3]), location);
    bytes.set_root(root);
    assert!(
        bytes
            .enforce_limits(
                crate::DataLimits {
                    bytes_len: 2,
                    ..defaults
                },
                source.len(),
            )
            .unwrap_err()
            .to_string()
            .contains("bytes_len")
    );
}

#[test]
fn records_shared_database_provenance() {
    let mut sources = SourceDatabase::default();
    let first = sources.add("first.json", r#"{"name":"Ada"}"#);
    let second = sources.add("second.json", r#"{"name":"Lin"}"#);
    let first = validate_json_registered(&sources, first).unwrap();
    let second = validate_json_registered(&sources, second).unwrap();
    fn field_location(plan: &ValidatedDataPlan) -> Location {
        let DataPlanNodeKind::Object(fields) = &plan.node(plan.root()).kind else {
            panic!("expected object");
        };
        plan.node(fields["name"].value).location
    }
    assert_ne!(
        field_location(&first).source,
        field_location(&second).source
    );
    assert_eq!(field_location(&first).range(), 8..13);
}
