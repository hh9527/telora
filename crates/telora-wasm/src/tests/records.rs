use super::*;

#[test]
fn records_project_and_update_closed_fields_while_dicts_merge_sorted_columns() {
    let source = include_str!("../../tests/fixtures/records.telora");
    let bytes = compile_export(source, "inspect").unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([
            {"x":1,"Y":"source"}, {"first":1,"second":1}, {},
            {"x":3,"y":"source","extra":[1,2]}, {"x":1,"y":"source","extra":[1,2]},
            {"x":2,"y":"source","extra":[1,2]}, {"x":4,"y":"source","extra":[1,2]},
            {"x":2,"y":"source","extra":[1,2]}, {"x":1,"Y":"source"},
            {"item":"generic"}, {"value":"changed"},
            {"child":{"x":2},"values":[]}, {"child":{"x":1},"values":[1]},
            {"a":1,"b":3,"c":5}, {"a":1,"b":2}, {"a":1,"b":2}, {"a":1,"b":2},
            {"a":[1,2],"b":[3],"z":[9]}, ["a","b","z"]
        ])
    );
    assert!(session.diagnostics().unwrap().is_empty());

    let source = include_str!("../../tests/fixtures/record-origins.telora");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result[0], serde_json::json!({"x":21,"y":"original"}));
    assert_eq!(result[1], serde_json::json!({"first":11,"second":11}));
    assert_eq!(result[2], serde_json::json!({"a":1,"kept":2}));
    for (index, text) in [
        (3, "\"original\""),
        (4, "21"),
        (5, "notice(\"base\", source) <~"),
        (6, "11"),
        (7, "{...original_dict}"),
        (8, "31"),
    ] {
        assert_eq!(
            result[index]["labels"][1]["location"]["start"],
            source.find(text).unwrap(),
            "{text}"
        );
    }
    assert_eq!(
        result[9]
            .as_array()
            .unwrap()
            .iter()
            .map(|report| report["message"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["checked", "checked", "positive required"]
    );
    assert_eq!(result[10], "discarded still fails");
    assert_eq!(result[11], "spread still fails");
    let diagnostics = session.diagnostics().unwrap();
    assert_eq!(
        diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>(),
        [
            "base",
            "discarded",
            "patch",
            "projection receiver",
            "a",
            "dict",
            "last"
        ]
    );
    assert!(diagnostics.iter().all(|d| d.warning));
}
