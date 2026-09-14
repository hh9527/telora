use super::*;

#[test]
fn equality_uses_closed_recursive_layouts_and_function_instance_identity() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/equality.telora"
    ))
    .expect("read test source");
    let bytes = compile_export(source, "checks").unwrap();
    let mut session = crate::session::Session::load(&bytes, 20_000_000).unwrap();
    session.initialize().unwrap();
    let checks = session.call(&[]).unwrap();
    for (index, check) in checks.as_array().unwrap().iter().enumerate() {
        assert_eq!(check, true, "equality check {index}");
    }
    assert_eq!(checks.as_array().unwrap().len(), 41);
    assert!(session.diagnostics().unwrap().is_empty());

    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/equality-effects.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 5_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result[0], false);
    assert_eq!(result[1][0]["message"], "left failed");
    assert_eq!(result[1].as_array().unwrap().len(), 1);
    assert_eq!(result[2][0]["message"], "evaluated left");
    assert_eq!(result[2][1]["message"], "right failed");
    assert_eq!(result[2].as_array().unwrap().len(), 2);
    assert_eq!(result[3], "NonFiniteFloat");
    assert_eq!(result[4], "NonFiniteFloat");
    assert_eq!(
        session
            .diagnostics()
            .unwrap()
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>(),
        ["left", "right"]
    );
}
