use wasm_encoder::Encode;
use wasmparser::{Operator, Parser, Payload};

#[test]
fn linked_calls_use_minimal_indices_and_still_execute() {
    let bytes = super::compile("pub def answer: Fn() -> Int = fn() { 42 };").unwrap();
    let mut calls = 0;
    for payload in Parser::new(0).parse_all(&bytes) {
        if let Payload::CodeSectionEntry(body) = payload.unwrap() {
            let mut operators = body.get_operators_reader().unwrap();
            while !operators.eof() {
                let start = operators.original_position();
                if let Operator::Call { function_index } = operators.read().unwrap() {
                    let mut encoded = Vec::new();
                    function_index.encode(&mut encoded);
                    assert_eq!(
                        operators.original_position() - start,
                        1 + encoded.len() as u64
                    );
                    calls += 1;
                }
            }
        }
    }
    assert!(calls > 0);
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!(42));
}
