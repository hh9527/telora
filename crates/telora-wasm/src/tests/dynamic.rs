use super::*;

#[test]
fn service_collection_field_witness_initializes_and_dispatches() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/service-collection-witness.telora"
    ))
    .unwrap();
    let bytes = compile_export(&source, "inspect").unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([true, true, true])
    );
    let bytes = compile_export(&source, "inspect_invalid_routes").unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([true, true, true, true, true])
    );
    let bytes = compile_export(&source, "inspect_sources").unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([true, true, true])
    );
    let bytes = compile_export(&source, "inspect_plan_sources").unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(["a", "shared", "z"])
    );
}

#[test]
fn service_collection_survives_initialization_and_request_reset() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/service-collection-witness.telora"
    ))
    .unwrap();
    let mir = super::graph(&source);
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "main")
        .unwrap();
    let bytes = crate::compile_service(&mir.seal_export(export).unwrap()).unwrap();
    let mut service = crate::transform_service::TransformSession::new(
        crate::session::Session::load(&bytes, 100_000_000).unwrap(),
    )
    .unwrap();
    assert_eq!(service.sources(), &["a", "shared", "z"]);
    assert!(
        service
            .initialize(&[
                crate::transform_service::SourceInput {
                    name: "a",
                    data: b"null",
                    format: telora_core::data_plan::Format::Json,
                },
                crate::transform_service::SourceInput {
                    name: "shared",
                    data: br#""ready""#,
                    format: telora_core::data_plan::Format::Json,
                },
                crate::transform_service::SourceInput {
                    name: "z",
                    data: b"null",
                    format: telora_core::data_plan::Format::Json,
                },
            ])
            .unwrap()
            .success
    );
    service.seal_initialization().unwrap();
    for (method, expected) in [("first", "first"), ("second", "second"), ("first", "first")] {
        let request = serde_json::json!({"method":method,"input":null});
        let reply = service.transform(request.to_string().as_bytes()).unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(reply["ok"], expected);
        assert_eq!(reply["error"], false);
        service.reset().unwrap();
    }
    let unknown = serde_json::json!({"method":"absent","input":null});
    let reply = service.transform(unknown.to_string().as_bytes()).unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["error"], true);
    assert!(!reply["diagnostics"].as_array().unwrap().is_empty());
    service.reset().unwrap();
    let request = serde_json::json!({"method":"second","input":null});
    let reply = service.transform(request.to_string().as_bytes()).unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["ok"], "second");
    service.reset().unwrap();
    for (verb, path, expected) in [
        ("POST", "/first", serde_json::json!("first")),
        ("GET", "/second/42", serde_json::json!({"item":"42"})),
    ] {
        let request = serde_json::json!({"http":{"method":verb,"path":path},"input":null});
        let reply = service.transform(request.to_string().as_bytes()).unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(reply["ok"], expected);
        service.reset().unwrap();
    }
    for (verb, path, status, allow) in [("GET", "/absent", 404, ""), ("GET", "/first", 405, "POST")]
    {
        let request = serde_json::json!({"http":{"method":verb,"path":path},"input":null});
        let reply = service.transform(request.to_string().as_bytes()).unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(reply["httpStatus"], status);
        assert_eq!(reply["allow"], allow);
        service.reset().unwrap();
    }
}

#[test]
fn dyn_fields_construct_closed_structs_and_newtypes() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/from-dyn-fields.telora"
    ))
    .unwrap();
    let bytes = compile_export(&source, "inspect").unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        assert_eq!(
            session.call(&[]).unwrap(),
            serde_json::json!(vec![true; 10])
        );
    }
    assert!(session.diagnostics().unwrap().is_empty());
    let bytes = compile_export(&source, "inspect_checks").unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!([true, true]));
}

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
