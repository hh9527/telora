use super::*;
use crate::json::text::TextSpan;

#[test]
fn spans_share_original_input_and_only_transformed_text_is_decoded() {
    let text = "plain: hello\nquoted: 'world'\nescaped: \"a\\nb\"\nblock: |-\n  one\n  two\nbytes: !!binary SGk=\n";
    let mut sources = SourceDatabase::default();
    let id = sources.try_add_data("spans.yaml", text.into()).unwrap();
    let raw = parse_structure(id, text, DataLimits::default()).unwrap();
    let pointer = raw.raw.nodes.as_ptr() as usize;
    let (plan, ctx) = raw.validate().unwrap();
    assert_eq!(plan.nodes.as_ptr() as usize, pointer, "reuse node arena");
    let YamlKind::Object(fields) = &plan.nodes[plan.root.index()].kind else {
        panic!("object")
    };
    let value = |name| {
        &plan.nodes[fields
            .iter()
            .find(|(key, _)| ctx.text(key) == name)
            .unwrap()
            .1
            .value
            .index()]
        .kind
    };
    for (name, expected) in [("plain", "hello"), ("quoted", "world")] {
        let YamlKind::String(span @ TextSpan::Source(range)) = value(name) else {
            panic!("source span")
        };
        assert_eq!(ctx.text(span), expected);
        assert_eq!(ctx.text(span).as_ptr(), text[range.clone()].as_ptr());
    }
    for (name, expected) in [("escaped", "a\nb"), ("block", "one\ntwo")] {
        let YamlKind::String(span @ TextSpan::Decoded(_)) = value(name) else {
            panic!("decoded span")
        };
        assert_eq!(ctx.text(span), expected);
    }
    let YamlKind::Bytes(span) = value("bytes") else {
        panic!("bytes")
    };
    assert_eq!(ctx.bytes(span), b"Hi");
    assert_eq!(ctx.decoded_bytes(), 12);
}

#[test]
fn semantic_errors_accumulate_after_structure_and_quota_is_fail_fast() {
    let text = fixture("multiple-errors.yaml");
    let mut sources = SourceDatabase::default();
    let id = sources.try_add_data("errors.yaml", text.clone()).unwrap();
    let structure = parse_structure(id, &text, DataLimits::default()).unwrap();
    let errors = structure.validate().unwrap_err();
    assert_eq!(errors.len(), 6, "{errors:?}");
    assert_eq!(
        errors
            .iter()
            .filter(|e| e.message.contains("duplicate YAML key"))
            .count(),
        2
    );
    assert!(
        errors
            .windows(2)
            .all(|p| p[0].labels[0].location.start <= p[1].labels[0].location.start)
    );
    let limits = DataLimits {
        container_size: 1,
        ..DataLimits::default()
    };
    let errors = parse_structure(id, &text, limits).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("container_size"));
}

#[test]
fn flow_scalar_spans_exclude_delimiter_whitespace() {
    let plan = parse(
        "{a : [ 1 , 2.5 , hello , 'world' , !!binary SGk= ]}",
        DataLimits::default(),
    )
    .unwrap();
    assert_eq!(
        crate::data_plan_test::render(&plan),
        "{a: [1, 2.5, \"hello\", \"world\", b\"\\x48\\x69\"]}"
    );
}

#[test]
fn recovered_commas_and_duplicates_cannot_bypass_limits() {
    let mut sources = SourceDatabase::default();
    let text = "[,1e999,,9223372036854775808]";
    let id = sources.try_add_data("commas.yaml", text.into()).unwrap();
    let errors = parse_structure(id, text, DataLimits::default())
        .unwrap()
        .validate()
        .unwrap_err();
    assert_eq!(errors.len(), 4, "{errors:?}");
    for text in ["[,,,]", "{a: 0, a: 1, a: 2}"] {
        let limits = DataLimits {
            container_size: 2,
            ..DataLimits::default()
        };
        let errors = parse_structure(id, text, limits).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("container_size"));
    }
    let limits = DataLimits {
        nodes: 2,
        ..DataLimits::default()
    };
    assert!(
        parse_structure(id, "[,,]", limits).unwrap_err()[0]
            .message
            .contains("nodes")
    );
}
