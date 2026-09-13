use super::*;

#[test]
fn tail_calls_bound_stack_and_preserve_pending_work_and_failures() {
    let source = include_str!("../../tests/fixtures/tail-calls.telora");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([42, 42, 42, 42, 42, 42, 42, true, 40])
    );
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
        diagnostics[0].origin[1] as usize,
        source.find("fail!(").unwrap()
    );
}
