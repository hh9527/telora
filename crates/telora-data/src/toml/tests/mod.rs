extern crate std;
use super::*;
use crate::json::{DataPlanNodeKind, DataScalar};
use alloc::string::String;

mod machine;

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
    let id = sources.add("test.toml", text);
    crate::data_plan::parse_registered_with_limits(
        &sources,
        id,
        crate::data_plan::Format::Toml,
        limits,
    ).map(crate::data_plan::ParsedData::owned)
}

#[test]
fn chunks_eol_and_positions_are_stable() {
    let mut snapshots = Vec::new();
    for eol in ["\n", "\r\n", "\r"] {
        let text = fixture("core.toml").replace('\n', eol);
        let mut sources = SourceDatabase::default();
        let source = sources.add("core.toml", &text);
        let expected = parse(&text, DataLimits::default()).unwrap();
        snapshots.push((
            crate::data_plan_test::render(&expected),
            expected
                .nodes()
                .iter()
                .map(|n| expected.compact(n.location).0)
                .collect::<Vec<_>>(),
        ));
        for at in text
            .char_indices()
            .map(|(i, _)| i)
            .chain(core::iter::once(text.len()))
        {
            let plan = super::parse::parse_chunks(
                source,
                [&text[..at], "", &text[at..]].into_iter(),
                text.len(),
                DataLimits::default(),
            )
            .unwrap();
            assert_eq!(
                format!("{:?}", plan.nodes()),
                format!("{:?}", expected.nodes()),
                "split {at}"
            );
        }
        let plan = super::parse::parse_chunks(
            source,
            text.char_indices()
                .map(|(i, ch)| &text[i..i + ch.len_utf8()]),
            text.len(),
            DataLimits::default(),
        )
        .unwrap();
        assert_eq!(
            format!("{:?}", plan.nodes()),
            format!("{:?}", expected.nodes())
        );
    }
    assert_eq!(snapshots[0], snapshots[1]);
    assert_eq!(snapshots[0], snapshots[2]);
}

#[test]
fn duplicate_and_unicode_spans_and_chunked_errors() {
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
    let field = &fields["é"];
    assert_eq!(field.key_location.range(), 0..4);
    assert_eq!(plan.node(field.value).location.range(), 7..13);
    for text in [
        text.as_str(),
        "a = \"unclosed",
        "a = \"\\uD800\"",
        "a = [1,,2]",
        "a={b=1,}",
    ] {
        let mut sources = SourceDatabase::default();
        let source = sources.add("error.toml", text);
        let expected = super::parse::parse_chunks(
            source,
            core::iter::once(text),
            text.len(),
            DataLimits::default(),
        )
        .unwrap_err();
        for at in text
            .char_indices()
            .map(|(i, _)| i)
            .chain(core::iter::once(text.len()))
        {
            let error = super::parse::parse_chunks(
                source,
                [&text[..at], &text[at..]].into_iter(),
                text.len(),
                DataLimits::default(),
            )
            .unwrap_err();
            assert_eq!(error, expected, "split {at}: {text}");
        }
    }
}
