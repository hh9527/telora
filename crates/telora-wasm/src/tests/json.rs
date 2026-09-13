use super::*;

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
