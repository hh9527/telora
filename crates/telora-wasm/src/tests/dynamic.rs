use super::*;

#[test]
fn dynamic_projection_uses_exact_type_ids_and_shared_box_identity() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/dynamic.telora"),
        "checks",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 5_000_000).unwrap();
    session.initialize().unwrap();
    let expected = serde_json::json!(vec![true; 18]);
    assert_eq!(session.call(&[]).unwrap(), expected);
    assert_eq!(session.call(&[]).unwrap(), expected);
    assert!(session.diagnostics().unwrap().is_empty());
}
