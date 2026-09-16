use super::*;

#[test]
fn interpreter_adapters_capture_operand_at_construction_without_memoization() {
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
    assert_eq!(session.diagnostics().unwrap().len(), 1);
    // Requests must never write memoized work pointers into initialized captures.
    let environments = |session: &crate::session::Session| {
        use crate::abi::{ENVIRONMENTS, table_address};
        let output = session.output();
        let table = table_address(ENVIRONMENTS) as u64;
        let buffer = output.word(table).unwrap() as u64;
        let frozen = output.word(table + 12).unwrap();
        (0..frozen)
            .map(|index| {
                let slot = buffer + u64::from(index) * 8;
                let pointer = output.word(slot).unwrap() as u64;
                let bytes = output.word(slot + 4).unwrap() as u64;
                output.bytes(pointer, bytes).unwrap().to_vec()
            })
            .collect::<Vec<_>>()
    };
    let baseline = environments(&session);
    assert!(!baseline.is_empty());
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 12])
    );
    assert_eq!(session.diagnostics().unwrap().len(), 1);
    assert_eq!(environments(&session), baseline);
    session.collect_work(&[]).unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 12])
    );
    assert_eq!(session.diagnostics().unwrap().len(), 1);
    assert_eq!(environments(&session), baseline);
}
