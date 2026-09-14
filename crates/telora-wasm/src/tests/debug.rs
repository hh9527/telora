use super::*;

#[test]
fn debug_is_observational_and_separate_from_diagnostics() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/debug.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    let initial = session.take_debug_events().unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].repr, "41");
    assert_eq!(initial[0].message.as_deref(), Some("initialize"));
    assert_eq!(initial[0].line, 5);
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!(vec![true; 5]));
    let events = session.take_debug_events().unwrap();
    assert_eq!(
        events.iter().map(|e| e.repr.as_str()).collect::<Vec<_>>(),
        [
            "42",
            "{name: \"é🦀\", value: 42}",
            "<fn>",
            "('True, (), b\"\\x61\\x62\", [1, 2], {a: 1, b: 2}, 'Some(3), -0.0, <dyn>)",
            "42",
            "{name: \"é🦀\", value: 42}",
        ]
    );
    assert!(session.take_debug_events().unwrap().is_empty());
    assert!(session.diagnostics().unwrap().is_empty());
    session.set_debug_enabled(false).unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!(vec![true; 5]));
    assert!(session.take_debug_events().unwrap().is_empty());

    let bytes = compile_export(source, "long").unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    session.call(&["🦀".repeat(10_000).into()]).unwrap();
    let events = session.take_debug_events().unwrap();
    assert!(events[0].repr.len() <= 4096);
    assert!(events[0].repr.ends_with("..."));

    let bytes = compile_export(source, "stopped").unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    assert!(session.call(&[]).unwrap_err().contains("stopped"));
    assert!(session.debug_events().unwrap().is_empty());
}
