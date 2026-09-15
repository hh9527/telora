use crate::{session::Session, transport::Value};

#[test]
fn static_service_initializes_and_captures_each_request() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/transform-service.telora")).unwrap();
    let bytes = super::compile(&source).unwrap();
    assert_eq!(bytes, super::compile(&source).unwrap(), "entry codegen must be deterministic");
    let mut session = Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let plan = Value {pointer: session.entry().unwrap(), ty: session.manifest.entry_type};
    let (names, init) = session.pair(plan).unwrap();
    assert_eq!(session.output_value(names).unwrap(), serde_json::json!(["a", "b"]));
    let ctx_ty = session.manifest.types[init.ty as usize].arguments[0];
    let ctx = session.input_value(ctx_ty, &serde_json::json!({"sources": {"a": 42, "b": null}})).unwrap();
    let handler = session.invoke_values(init, &[ctx]).unwrap();
    let input_ty = session.manifest.types[handler.ty as usize].arguments[0];
    for input in [serde_json::json!(1), serde_json::Value::Null, serde_json::json!(2)] {
        let value = session.input_value(input_ty, &input).unwrap();
        let result = session.invoke_values(handler, &[value]).unwrap();
        let output = session.output_value(result).unwrap();
        if input.is_null() {
            assert_eq!(output["Err"][0]["message"], "missing query");
            assert!(!output["Err"][0]["labels"].as_array().unwrap().is_empty());
        } else {
            assert_eq!(output["Ok"][0], serde_json::json!([42, input]));
        }
        assert!(session.diagnostics().unwrap().is_empty());
    }
}

#[test]
fn reset_restores_initialized_service_after_fuel_and_memory_traps() {
    use crate::transform_service::TransformSession;
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/transform-service.telora")).unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    session.memory_limit = 1024 * 1024;
    let mut service = TransformSession::new(session).unwrap();
    let ty = service.session().manifest.value_type.unwrap();
    let a = service.session_mut().input_value(ty, &serde_json::json!(42)).unwrap();
    let b = service.session_mut().input_value(ty, &serde_json::Value::Null).unwrap();
    service.initialize(&std::collections::BTreeMap::from([("a".into(), a), ("b".into(), b)])).unwrap();
    service.seal_initialization().unwrap();
    for input in ["ok", "loop", "ok", "grow", "ok"] {
        service.reset().unwrap();
        if input == "loop" { service.session_mut().store.set_fuel(100_000).unwrap(); }
        let value = service.session_mut().input_value(ty, &input.into()).unwrap();
        let result = service.transform(value);
        match input {
            "loop" => assert!(result.unwrap_err().contains("fuel")),
            "grow" => assert!(result.unwrap_err().contains("growth")),
            _ => assert_eq!(result.unwrap()["Ok"][0], serde_json::json!([42, "ok"])),
        }
    }
    let mut size = None;
    for _ in 0..256 {
        service.reset().unwrap();
        let current = service.session().memory.data_size(&service.session().store);
        if let Some(expected) = size { assert_eq!(current, expected); }
        size = Some(current);
        let value = service.session_mut().input_value(ty, &serde_json::json!(42)).unwrap();
        assert_eq!(service.transform(value).unwrap()["Ok"][0], serde_json::json!([42, 42]));
    }
}
