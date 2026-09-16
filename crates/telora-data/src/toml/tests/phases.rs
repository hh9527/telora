use super::*;
use crate::json::text::TextSpan;

#[test]
fn syntax_keeps_independent_table_blocks_and_defers_resolution() {
    let text =
        "[workspace.package]\nname='foo'\nversion='1.0.0'\n[workspace.package]\nname='bar'\n";
    let mut sources = SourceDatabase::default();
    let id = sources.try_add_data("test", text.into()).unwrap();
    let syntax = parse_structure(id, text, DataLimits::default()).unwrap();
    assert_eq!(syntax.raw.tables.len(), 3);
    assert!(syntax.raw.tables[0].header.is_none());
    assert_eq!(syntax.raw.tables[1].items.len(), 2);
    assert_eq!(syntax.raw.tables[2].items.len(), 1);
    assert_eq!(syntax.raw.tables[1].header.as_ref().unwrap().path.len(), 2);
    let errors = syntax.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("already defined"));
}
#[test]
fn plain_text_borrows_and_transforms_share_a_buffer() {
    let text = "plain='hello'\nescaped=\"a\\nb\"\ncanonical=1979-05-27T07:32:00Z\nwhen=1979-05-27 07:32:00+00:00\n";
    let mut sources = SourceDatabase::default();
    let id = sources.try_add_data("test", text.into()).unwrap();
    let (plan, ctx) = parse_structure(id, text, DataLimits::default())
        .unwrap()
        .validate()
        .unwrap();
    let TomlKind::Object(fields) = &plan.nodes[plan.root.index()].kind else {
        panic!()
    };
    let get = |name| {
        &plan.nodes[fields
            .iter()
            .find(|(k, _)| ctx.text(k) == name)
            .unwrap()
            .1
            .value
            .index()]
        .kind
    };
    let TomlKind::String(span @ TextSpan::Source(range)) = get("plain") else {
        panic!("borrowed plain string")
    };
    assert_eq!(ctx.text(span).as_ptr(), text[range.clone()].as_ptr());
    let TomlKind::Temporal {
        value: TextSpan::Source(_),
        ..
    } = get("canonical")
    else {
        panic!("borrowed canonical time")
    };
    let TomlKind::Temporal {
        value: span @ TextSpan::Decoded(_),
        ..
    } = get("when")
    else {
        panic!("normalized time")
    };
    assert_eq!(ctx.text(span), "1979-05-27T07:32:00Z");
    assert_eq!(ctx.decoded_bytes(), 23);
}
#[test]
fn final_graph_limits_are_checked_during_second_phase() {
    let text = "[a.b]\nx=1";
    let mut sources = SourceDatabase::default();
    let id = sources.try_add_data("test", text.into()).unwrap();
    let syntax = parse_structure(
        id,
        text,
        DataLimits {
            depth: 3,
            ..DataLimits::default()
        },
    )
    .unwrap();
    assert!(syntax.validate().unwrap_err()[0].message.contains("depth"));
    let syntax = parse_structure(
        id,
        text,
        DataLimits {
            nodes: 3,
            ..DataLimits::default()
        },
    )
    .unwrap();
    assert!(syntax.validate().unwrap_err()[0].message.contains("nodes"));
}

#[test]
fn independent_errors_accumulate_and_syntax_exhaustion_stops() {
    let text = fixture("multiple-errors.toml");
    let mut sources = SourceDatabase::default();
    let id = sources.try_add_data("test", text.clone()).unwrap();
    let syntax = parse_structure(id, &text, DataLimits::default()).unwrap();
    let errors = syntax.validate().unwrap_err();
    assert_eq!(errors.len(), 6, "{errors:?}");
    assert_eq!(
        errors
            .iter()
            .filter(|d| d.message.contains("duplicate TOML key"))
            .count(),
        2
    );
    assert!(
        errors
            .windows(2)
            .all(|p| p[0].labels[0].location.start <= p[1].labels[0].location.start)
    );
    let errors = parse_structure(
        id,
        "a=[,,,]",
        DataLimits {
            nodes: 2,
            ..DataLimits::default()
        },
    )
    .unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("nodes"));
}
