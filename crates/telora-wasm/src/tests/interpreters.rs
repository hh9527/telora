use super::*;

#[test]
fn interpreter_adapters_preserve_identity_captures_and_deferred_operand() {
    let bytes = compile(include_str!("../../tests/fixtures/interpreter.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert!(session.diagnostics().unwrap().is_empty());
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([true, true, true, true, true, true, true, true])
    );
    assert_eq!(session.diagnostics().unwrap().len(), 2);
}
