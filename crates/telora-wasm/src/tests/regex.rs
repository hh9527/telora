use super::*;

#[test]
fn string_parse_constructs_nested_and_recursive_sealed_records() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/language/src/test/runtime-text/string-parse-record.telora"
        ))
        .expect("read test source"),
        "checks",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!(vec![true; 8]));
    assert!(session.diagnostics().unwrap().is_empty());
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/string-parse-record-effects.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result[0].as_array().unwrap().len(), 1);
    assert_eq!(result[0][0]["message"], "positive required");
    assert_eq!(
        diagnostic_point(&result[0][0]["labels"][1]["location"]["start"]),
        point(source, source.find("\"-7\"").unwrap())
    );
    assert_eq!(result[1], 42);
    assert_eq!(
        diagnostic_point(&result[2]["labels"][1]["location"]["start"]),
        point(source, source.find("\"\"").unwrap())
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn regex_property_initialization_validates_sealed_capture_contracts() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/language/src/test/runtime-text/regex-prepare.telora"
        ))
        .expect("read test source"),
        "checks",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([true, true, true, true, true])
    );
    for (fixture, message) in [
        (
            "missing",
            "regex captures must match struct fields; missing captures [\"x\"], extra captures [\"y\"]",
        ),
        (
            "optional",
            "regex capture \"x\" is optional, but its field is required",
        ),
        (
            "required",
            "regex capture \"x\" is required, but its field is optional",
        ),
        ("scalar", "std/regex.parse_by requires a struct type"),
    ] {
        let source = std::fs::read_to_string(format!(
            "{}/tests/fixtures/regex-property-{fixture}.telora",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("read regex property fixture");
        let bytes = compile(&source).unwrap();
        let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
        let error = session.initialize().unwrap_err().to_string();
        assert!(error.contains(message), "{fixture}: {error}");
    }
}

#[test]
fn string_parse_uses_closed_scalar_and_option_targets() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/language/src/test/runtime-text/string-parse.telora"
        ))
        .expect("read test source"),
        "checks",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 13])
    );
    assert!(session.diagnostics().unwrap().is_empty());
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/string-parse-origins.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let report = session.call(&[]).unwrap();
    assert_eq!(
        diagnostic_point(&report["labels"][1]["location"]["start"]),
        point(source, source.find("\"12345\"").unwrap())
    );
}

#[test]
fn regex_errors_are_language_diagnostics_and_leave_the_session_usable() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/regex-errors.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 50_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert!(
        result[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("invalid regular expression:")
    );
    assert_eq!(result[1]["message"], "capture group 1 must have a name");
    assert!(
        result[2]["message"]
            .as_str()
            .unwrap()
            .contains("look-around")
    );
    assert_eq!(result[3], true);
    assert_eq!(
        diagnostic_point(&result[0]["labels"][1]["location"]["start"]),
        point(source, source.find("\"[\"").unwrap())
    );
    assert!(session.diagnostics().unwrap().is_empty());
}
