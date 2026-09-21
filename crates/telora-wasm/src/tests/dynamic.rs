use super::*;

#[test]
fn dyn_array_observation_honors_the_abi_slice_range() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/language/src/test/runtime-reflection/dynamic-sequences.telora"
        ))
        .expect("read test source"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let function = session.entry().unwrap();
    let signature = session.manifest.types[session.manifest.entry_type as usize]
        .arguments
        .clone();
    let value = session
        .input(signature[0], &serde_json::json!([10, 20, 30, 40]), 0)
        .unwrap();
    // No source syntax currently exposes Array slicing; construct its valid ABI descriptor.
    session
        .write(
            value as usize + crate::abi::DATA as usize + 4,
            &1u32.to_le_bytes(),
        )
        .unwrap();
    session
        .write(
            value as usize + crate::abi::DATA as usize + 8,
            &3u32.to_le_bytes(),
        )
        .unwrap();
    let args = session.allocate(4).unwrap();
    session.write(args as usize, &value.to_le_bytes()).unwrap();
    let invoke = session
        .instance
        .get_typed_func::<(i32, i32), i32>(&session.store, "telora_invoke")
        .unwrap();
    let result = invoke
        .call(&mut session.store, (function as i32, args as i32))
        .unwrap();
    let output = crate::output::Output {
        memory: session.memory.data(&session.store),
        manifest: &session.manifest,
    };
    assert_eq!(
        output.json(result as u64, signature[1], 0).unwrap(),
        serde_json::json!([20, 30])
    );
}

#[test]
fn dynamic_projection_uses_exact_type_ids_and_shared_box_identity() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/language/src/test/runtime-reflection/dynamic.telora"
        ))
        .expect("read test source"),
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
