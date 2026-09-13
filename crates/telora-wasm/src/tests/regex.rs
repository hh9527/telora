use super::*;

#[test]
fn string_parse_uses_closed_scalar_and_option_targets() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/string-parse.telora"),
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
    let source = include_str!("../../tests/fixtures/string-parse-origins.telora");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let report = session.call(&[]).unwrap();
    assert_eq!(
        report["labels"][1]["location"]["start"],
        source.find("\"12345\"").unwrap()
    );
}

#[test]
fn regex_errors_are_language_diagnostics_and_leave_the_session_usable() {
    let source = include_str!("../../tests/fixtures/regex-errors.telora");
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
        result[0]["labels"][1]["location"]["start"],
        source.find("\"[\"").unwrap()
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn regex_compiles_and_matches_in_the_linked_rust_runtime() {
    let bytes =
        compile_export(include_str!("../../tests/fixtures/regex.telora"), "checks").unwrap();
    let mut session = crate::session::Session::load(&bytes, 50_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 10])
    );
    assert!(session.diagnostics().unwrap().is_empty());
}
