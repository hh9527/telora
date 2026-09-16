use super::super::{
    JsonKind, parse,
    structure::RawKind,
    text::{ParseCtx, TextSpan},
    validate,
};
use crate::{DataLimits, SourceDatabase};
extern crate std;

#[test]
fn collects_independent_diagnostics_but_resource_failure_stops_immediately() {
    let input = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/json/multiple-errors.json"),
    )
    .unwrap();
    let mut sources = SourceDatabase::default();
    let source = sources.add("multiple-errors.json", &input);
    let errors = super::super::parse_structure(source, &input, DataLimits::default())
        .unwrap()
        .validate()
        .unwrap_err();
    assert_eq!(errors.len(), 6, "{errors:?}");
    for message in [
        "outside the i64 range",
        "extra JSON comma",
        "duplicate JSON object key",
        "must be finite",
        "trailing JSON comma",
    ] {
        assert!(
            errors.iter().any(|error| error.message.contains(message)),
            "{message}"
        );
    }
    for error in errors
        .iter()
        .filter(|error| error.message.contains("comma"))
    {
        assert_eq!(&input[error.labels[0].location.range()], ",");
    }
    let errors = super::super::parse_structure(
        source,
        &input,
        DataLimits {
            nodes: 2,
            ..DataLimits::default()
        },
    )
    .unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("nodes limit"));
}

#[test]
fn structural_phase_defers_duplicate_keys_and_numeric_ranges() {
    for input in [r#"{"a":1,"\u0061":2}"#, "9223372036854775808", "1e999"] {
        let mut sources = SourceDatabase::default();
        let source = sources.add("phase.json", input);
        let raw = parse::Parser::new(source, core::iter::once(input), DataLimits::default())
            .parse(input.len())
            .expect("structurally valid");
        let mut ctx = ParseCtx::new(input);
        assert_eq!(ctx.decoded_bytes(), 0);
        assert!(validate::validate(raw, &mut ctx).is_err());
    }
}

#[test]
fn plain_text_stays_in_source_and_escaped_text_uses_one_buffer() {
    let input = r#"["plain", "\uD83D\uDE00", "more", "\n"]"#;
    let mut sources = SourceDatabase::default();
    let source = sources.add("phase.json", input);
    let raw = parse::Parser::new(source, core::iter::once(input), DataLimits::default())
        .parse(input.len())
        .unwrap();
    assert!(
        matches!(&raw.nodes[1].kind, RawKind::String(s) if &input[s.range.clone()] == "\\uD83D\\uDE00")
    );
    let mut ctx = ParseCtx::new(input);
    let plan = validate::validate(raw, &mut ctx).unwrap();
    assert_eq!(ctx.decoded_bytes(), 5);
    for (index, expected) in ["plain", "😀", "more", "\n"].into_iter().enumerate() {
        let JsonKind::String(span) = &plan.nodes[index].kind else {
            panic!("string")
        };
        assert_eq!(ctx.text(span), expected);
        assert_eq!(matches!(span, TextSpan::Source(_)), index % 2 == 0);
    }
    // Offsets, unlike references, remain valid across decoded-buffer growth.
    ctx.decoded.push_str(&"x".repeat(100_000));
    let JsonKind::String(span) = &plan.nodes[1].kind else {
        panic!("string")
    };
    assert_eq!(ctx.text(span), "😀");
    assert_eq!(&input[plan.nodes[1].location.range()], r#""\uD83D\uDE00""#);
}

#[test]
fn keys_are_sorted_by_contents_and_failed_validation_does_not_publish_values() {
    let input = r#"{"z":0,"\u0061":1,"b":2}"#;
    let mut sources = SourceDatabase::default();
    let source = sources.add("phase.json", input);
    let (plan, ctx) = super::super::parse_structure(source, input, DataLimits::default())
        .unwrap()
        .validate()
        .unwrap();
    let JsonKind::Object(fields) = &plan.nodes[plan.root.index()].kind else {
        panic!("object")
    };
    let names: alloc::vec::Vec<_> = fields.iter().map(|(key, _)| ctx.text(key)).collect();
    assert_eq!(names, ["a", "b", "z"]);
    let bad = r#"{"a":1,"\u0061":2}"#;
    for _ in 0..2 {
        assert!(
            super::super::parse_structure(source, bad, DataLimits::default())
                .unwrap()
                .validate()
                .is_err()
        );
    }
}

#[test]
fn recovered_slots_cannot_bypass_resource_admission() {
    let mut sources = SourceDatabase::default();
    let source = sources.add("commas.json", "[,,,,]");
    let errors = super::super::parse_structure(
        source,
        "[,,,,]",
        DataLimits {
            container_size: 2,
            ..DataLimits::default()
        },
    )
    .unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("container_size limit"));
}

#[test]
fn validation_reuses_the_flat_node_arena() {
    let input = "[0,1,2,3]";
    let mut sources = SourceDatabase::default();
    let source = sources.add("arena.json", input);
    let raw = parse::Parser::new(source, core::iter::once(input), DataLimits::default())
        .parse(input.len())
        .unwrap();
    let allocation = raw.nodes.as_ptr().cast::<u8>();
    let mut ctx = ParseCtx::new(input);
    let plan = validate::validate(raw, &mut ctx).unwrap();
    assert_eq!(allocation, plan.nodes.as_ptr().cast::<u8>());
}
