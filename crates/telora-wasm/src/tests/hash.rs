use super::*;

#[test]
fn hash_states_remain_immutable_across_initialization_and_calls() {
    let bytes = compile_export(include_str!("../../tests/fixtures/hash.telora"), "checks").unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    for _ in 0..3 {
        assert_eq!(
            session.call(&[]).unwrap(),
            serde_json::json!(vec![true; 12])
        );
    }
    assert!(session.diagnostics().unwrap().is_empty());
}
