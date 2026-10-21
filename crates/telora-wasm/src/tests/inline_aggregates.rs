use crate::{abi::*, session::Session, transport::Value};

#[test]
fn caller_output_owns_forwarded_values_across_tail_and_early_returns() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-output-ownership.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 500_000_000).unwrap();
    session.initialize().unwrap();
    let callable = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let signature = session.manifest.types[callable.ty as usize]
        .arguments
        .clone();
    let expected = serde_json::json!({"x": 7, "y": [8, 9]});
    let input = session.input_value(signature[0], &expected).unwrap();
    // These Host borrows are reused after Guest execution, so own their roots.
    let callable_pin = session.pin_value(callable).unwrap();
    let input_pin = session.pin_value(input).unwrap();
    let count = session.input_value(signature[1], &10000.into()).unwrap();
    let heap_bytes = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_heap_bytes")
        .unwrap();
    let before = heap_bytes.call(&mut session.store, ()).unwrap();
    let result = session.invoke_values(callable, &[input, count]).unwrap();
    let after = heap_bytes.call(&mut session.store, ()).unwrap();
    assert!(
        after - before < 256,
        "pure tail computation grew managed storage by {} bytes",
        after - before
    );
    let depth = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_frame_depth")
        .unwrap();
    assert_eq!(depth.call(&mut session.store, ()).unwrap(), 0);
    assert_ne!(result.pointer, input.pointer);
    assert_eq!(session.output_value(result).unwrap(), expected);
    assert_eq!(
        session.output().bytes(result.pointer as u64, 8).unwrap(),
        session.output().bytes(input.pointer as u64, 8).unwrap(),
    );
    let field = INLINE_DATA as usize + DATA as usize;
    session
        .write(input.pointer as usize + field, &99i64.to_le_bytes())
        .unwrap();
    assert_eq!(session.output_value(result).unwrap(), expected);

    let early = session.input_value(signature[1], &(-1).into()).unwrap();
    let early_pin = session.pin_value(early).unwrap();
    let first = session.invoke_values(callable, &[input, early]).unwrap();
    let first_pin = session.pin_value(first).unwrap();
    let saved = serde_json::json!({"x": 11, "y": [12, 13]});
    assert_eq!(session.output_value(first).unwrap(), saved);
    session
        .write(first.pointer as usize + field, &88i64.to_le_bytes())
        .unwrap();
    let second = session.invoke_values(callable, &[input, early]).unwrap();
    assert_ne!(first.pointer, second.pointer);
    assert_eq!(session.output_value(second).unwrap(), saved);
    for pin in [callable_pin, input_pin, early_pin, first_pin] {
        session.unpin_value(pin).unwrap();
    }
}

#[test]
fn nested_plain_aggregates_copy_owned_fields_and_survive_collection() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-inline-aggregates.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 20_000_000).unwrap();
    session.initialize().unwrap();
    let callable = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let point = session.manifest.types[callable.ty as usize].arguments[0];
    let descriptor = session.manifest.types[point as usize].clone();
    assert!(descriptor.inline_fields);
    assert!(descriptor.bytes > SCALAR_BYTES);
    let tuple = &session.manifest.types[descriptor.fields[1].ty as usize];
    assert!(tuple.inline_fields);
    assert!(session.manifest.types[tuple.fields[1].ty as usize].inline_fields);
    let expected_point = serde_json::json!({"x": 7, "y": [8, 9]});
    let input = session.input_value(point, &expected_point).unwrap();
    let mut result = session.invoke_values(callable, &[input]).unwrap();
    let expected = serde_json::json!([[expected_point, expected_point], expected_point, 9, {}]);
    assert_eq!(session.output_value(result).unwrap(), expected);
    // Every copy must own the complete finite payload. An absolute pointer into
    // input storage would make these otherwise immutable results change here.
    session
        .write(
            input.pointer as usize
                + INLINE_DATA as usize
                + descriptor.fields[0].offset as usize
                + DATA as usize,
            &99i64.to_le_bytes(),
        )
        .unwrap();
    assert_eq!(session.output_value(input).unwrap()["x"], 99);
    assert_eq!(session.output_value(result).unwrap(), expected);
    for _ in 0..4 {
        result = session.collect_work(&[result]).unwrap().0[0];
        assert_eq!(session.output_value(result).unwrap(), expected);
    }
}
