extern crate std;
use super::*;
use crate::{
    SourceDatabase,
    json::{DataPlanNodeKind, DataScalar, ValidatedDataPlan},
};
use alloc::string::String;
mod machine;
mod phases;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/toml")
            .join(name),
    )
    .unwrap()
}
fn parse(text: &str, limits: DataLimits) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let mut sources = SourceDatabase::default();
    let source = sources.try_add_data("test.toml", text.into()).unwrap();
    let crate::data_plan::ParsedData::Toml { plan, decoded } =
        crate::data_plan::parse_registered_with_limits(
            &sources,
            source,
            crate::data_plan::Format::Toml,
            limits,
        )?
    else {
        panic!("TOML span plan")
    };
    let mut plan = owned(plan, text, &decoded);
    plan.source_index = Some((source, sources.get(source).line_index().clone()));
    Ok(plan)
}
fn owned(parsed: TomlPlan, source: &str, decoded: &str) -> ValidatedDataPlan {
    let mut plan = ValidatedDataPlan::default();
    for node in parsed.nodes {
        let scalar = match node.kind {
            TomlKind::Int(n) => DataScalar::Int(n),
            TomlKind::Float(n) => DataScalar::Float(n),
            TomlKind::Bool(b) => DataScalar::Bool(b),
            TomlKind::String(s) => DataScalar::String(s.resolve(source, decoded).into()),
            TomlKind::Temporal { kind, value } => DataScalar::Temporal {
                kind,
                value: value.resolve(source, decoded).into(),
            },
            TomlKind::Array(items) => {
                plan.array(items, node.location);
                continue;
            }
            TomlKind::Object(fields) => {
                plan.object(
                    fields
                        .into_iter()
                        .map(|(k, v)| (k.resolve(source, decoded).into(), v))
                        .collect(),
                    node.location,
                );
                continue;
            }
        };
        plan.scalar(scalar, node.location);
    }
    plan.set_root(parsed.root);
    plan.postordered = true;
    plan
}

#[test]
fn eol_and_lexical_windows_preserve_values_and_positions() {
    let mut snapshots = Vec::new();
    for eol in ["\n", "\r\n", "\r"] {
        let text = fixture("core.toml").replace('\n', eol);
        let plan = parse(&text, DataLimits::default()).unwrap();
        snapshots.push((
            crate::data_plan_test::render(&plan),
            plan.nodes()
                .iter()
                .map(|n| plan.compact(n.location).0)
                .collect::<Vec<_>>(),
        ));
    }
    assert_eq!(snapshots[0], snapshots[1]);
    assert_eq!(snapshots[0], snapshots[2]);
    for len in [4094, 4095, 4096, 8191] {
        let text = format!("x=\"{}\\u4e2d\"", "a".repeat(len));
        let plan = parse(&text, DataLimits::default()).unwrap();
        assert!(plan.nodes().iter().any(|n| matches!(&n.kind, DataPlanNodeKind::Scalar(DataScalar::String(s)) if s == &format!("{}中", "a".repeat(len)))));
    }
}
#[test]
fn duplicate_and_unicode_spans_are_preserved() {
    let text = fixture("locations.toml");
    let errors = parse(&text, DataLimits::default()).unwrap_err();
    let first = text.find("name").unwrap();
    let second = text.rfind("name").unwrap();
    assert_eq!(errors[0].labels[0].location.range(), second..second + 4);
    assert_eq!(errors[0].labels[1].location.range(), first..first + 4);
    let plan = parse(&text[..second], DataLimits::default()).unwrap();
    let DataPlanNodeKind::Object(fields) = &plan.node(plan.root()).kind else {
        panic!()
    };
    assert_eq!(fields["é"].key_location.range(), 0..4);
    assert_eq!(plan.node(fields["é"].value).location.range(), 7..13);
}
