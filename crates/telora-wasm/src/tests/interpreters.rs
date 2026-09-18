use super::*;

#[test]
fn stack_array_elements_preserve_captures_branches_and_failure_order() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/stack-array-elements.telora"
    ))
    .unwrap();
    let bytes = compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        assert_eq!(
            session.call(&[]).unwrap(),
            serde_json::json!([
                18, 19, 7, [[3, 4], [5, 6]], [11, 14], [[8, 9], [10, 11]], 2,
                [[1, 2, 3], [4, 5]], 10, 17, [42], [2, 3]
            ])
        );
        assert!(session.diagnostics().unwrap().is_empty());
        session.collect_work(&[]).unwrap();
    }
}

#[test]
fn large_captured_interpreter_operands_use_bounded_scratch_locals() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/interpreter-large-operands.telora"
    ))
    .unwrap();
    let bytes = compile(&source).unwrap();
    let maximum = wasmparser::Parser::new(0)
        .parse_all(&bytes)
        .filter_map(|payload| match payload.unwrap() {
            wasmparser::Payload::CodeSectionEntry(body) => Some(
                body.get_locals_reader()
                    .unwrap()
                    .into_iter()
                    .map(|local| local.unwrap().0)
                    .sum::<u32>(),
            ),
            _ => None,
        })
        .max()
        .unwrap();
    assert!(maximum < 1000, "largest function has {maximum} locals");
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        assert_eq!(session.call(&[]).unwrap(), serde_json::json!(7800));
        assert!(session.diagnostics().unwrap().is_empty());
        session.collect_work(&[]).unwrap();
    }
}

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
