use super::*;

#[test]
fn interpreter_adapters_preserve_identity_captures_and_deferred_operand() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/interpreter.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert!(session.diagnostics().unwrap().is_empty());
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([true, true, true, true, true, true, true, true])
    );
    assert_eq!(session.diagnostics().unwrap().len(), 2);
    session.collect_work(&[]).unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([true, true, true, true, true, true, true, true])
    );
}
