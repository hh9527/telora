use super::*;

#[test]
fn tail_calls_bound_stack_and_preserve_pending_work_and_failures() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/tail-calls.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    // This verifies stack behavior, not an exact instruction budget.
    let mut session = crate::session::Session::load(&bytes, 200_000_000).unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([42, 42, 42, 42, 42, 42, 42, true, 40])
    );
    let usage = session.usage();
    eprintln!("tail-call fixture fuel: {}", usage.fuel_budget - usage.fuel_remaining);
    let events = session.debug_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message.as_deref(), Some("after tail recursion"));
    assert_eq!(events[0].repr, "40");
    let bytes = compile_export(source, "failing").unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert!(session.call(&[]).unwrap_err().contains("tail failure"));
    let diagnostics = session.diagnostics().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        telora_core::source::SourceCoordinates(diagnostics[0].origin).start(),
        point(source, source.find("fail!(").unwrap())
    );
}
