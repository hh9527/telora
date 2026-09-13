use super::*;

#[test]
fn codec_scalar_encoding_uses_closed_payload_identities() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/codec-scalars.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 14])
    );
}

#[test]
fn json_parse_error_blames_the_original_text() {
    let source = include_str!("../../tests/fixtures/json-parse-origins.telora");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(
        result["labels"][1]["location"]["start"],
        source.find("\"[1,]\"").unwrap()
    );
    assert_eq!(result["message"], "expected value at line 1 column 4");
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn json_parse_materializes_postorder_plan_with_closed_types() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/json-parse.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        assert_eq!(
            session.call(&[]).unwrap(),
            serde_json::json!([
                "{\"a\":[1,1,true,null,\"中\",{\"k\":false}],\"z\":[]}",
                true,
                true,
                true,
                true,
                true
            ])
        );
    }
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn stringify_rejections_are_captured_once_and_preserve_subjects() {
    let source = include_str!("../../tests/fixtures/json-errors.telora");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    for (index, message) in [
        "JSON cannot encode Bytes",
        "JSON cannot encode temporal values; use a codec first",
        "std/json.stringify_pretty indent must be between 0 and 16",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(result[index]["message"], *message);
    }
    assert_eq!(
        result[0]["labels"][1]["location"]["start"],
        source.find("Value.Bytes(b").unwrap()
    );
    assert_eq!(result[3], "true");
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn stringify_traverses_closed_value_layouts() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/json-stringify.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        let result = session.call(&[]).unwrap();
        assert_eq!(
            result[0],
            "{\"a\":[null,true,false,-7,1,\"中\\n\\\"\"],\"z\":{}}"
        );
        let expected: serde_json::Value =
            serde_json::from_str(result[0].as_str().unwrap()).unwrap();
        assert_eq!(result[1], serde_json::to_string_pretty(&expected).unwrap());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result[2].as_str().unwrap()).unwrap(),
            expected
        );
        assert_eq!(result[3], "[]");
    }
}
