use crate::{session::Session, transport::Value};

fn artifact() -> Vec<u8> {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/transform-service.telora")).unwrap();
    let mir = super::graph(&source);
    let export = mir.exports.iter().flatten().copied()
        .find(|id| mir.symbols[id.index()].name == "answer").unwrap();
    crate::compile_service(&mir.seal_export(export).unwrap()).unwrap()
}

fn prepare(bytes: &[u8]) -> (Session, Vec<u32>) {
    let mut session = Session::load(bytes, 100_000_000).unwrap();
    // Enumeration prepares the sealed entry without exposing its language value.
    let count = session.instance.get_typed_func::<(), u32>(&session.store, "get-data-source-count")
        .unwrap().call(&mut session.store, ()).unwrap();
    assert_eq!(session.instance.get_typed_func::<(), u32>(&session.store, "get-data-source-count")
        .unwrap().call(&mut session.store, ()).unwrap(), count);
    assert_eq!(count, 2);
    let mut ids = Vec::new();
    let result = session.instance.get_typed_func::<(u32, u32), u32>(&session.store, "mem-alloc")
        .unwrap().call(&mut session.store, (12, 4)).unwrap();
    for (index, expected) in ["a", "b"].into_iter().enumerate() {
        session.instance.get_typed_func::<(u32, u32), ()>(&session.store, "get-data-source-name")
            .unwrap().call(&mut session.store, (index as u32, result)).unwrap();
        let output = session.output();
        let id = output.word(result as u64).unwrap();
        let pointer = output.word(result as u64 + 4).unwrap();
        let length = output.word(result as u64 + 8).unwrap();
        assert_ne!(pointer, 0);
        assert_eq!(pointer % 8, 0);
        assert_eq!(output.bytes(pointer as u64, length as u64).unwrap(), expected.as_bytes());
        assert!(!session.manifest.sources.iter().any(|source| source.id == id));
        ids.push(id);
    }
    session.instance.get_typed_func::<(u32, u32, u32), ()>(&session.store, "mem-free")
        .unwrap().call(&mut session.store, (result, 12, 4)).unwrap();
    assert!(ids[0] < ids[1]);
    (session, ids)
}

fn parse(session: &mut Session, id: u32, text: &[u8], format: u32) -> Result<u32, wasmi::Error> {
    let cap = text.len() as u32;
    let pointer = session.instance.get_typed_func::<(u32, u32), u32>(&session.store, "mem-alloc")
        .unwrap().call(&mut session.store, (cap, 1)).unwrap();
    session.memory.write(&mut session.store, pointer as usize, text).unwrap();
    let result = session.instance.get_typed_func::<(u32, u32, u32, u32), u32>(&session.store, "telora_service_source_parse")
        .unwrap().call(&mut session.store, (id, pointer, text.len() as u32, format));
    if result.is_ok() {
        // The input borrow ended on return; Host still owns this allocation.
        // Overwrite before free proves parsed spans own their retained text.
        session.memory.write(&mut session.store, pointer as usize, &vec![b'x'; text.len()]).unwrap();
        session.instance.get_typed_func::<(u32, u32, u32), ()>(&session.store, "mem-free")
            .unwrap().call(&mut session.store, (pointer, cap, 1)).unwrap();
    }
    result
}

#[test]
fn guest_slots_own_names_inputs_and_location_identity() {
    let bytes = artifact();
    let mut expected_ids = None;
    for (format, input) in [(1, "{\"value\":42}"), (2, "value: 42\n"), (3, "value = 42\n")] {
        let (mut session, ids) = prepare(&bytes);
        if let Some(expected) = &expected_ids { assert_eq!(&ids, expected); }
        expected_ids = Some(ids.clone());
        let pointer = session.instance.get_typed_func::<(u32, u32), u32>(&session.store, "mem-alloc")
            .unwrap().call(&mut session.store, (64, 1)).unwrap();
        let mut retained = Vec::new();
        for id in ids {
            session.memory.write(&mut session.store, pointer as usize, input.as_bytes()).unwrap();
            let packet = session.instance.get_typed_func::<(u32, u32, u32, u32), u32>(&session.store, "telora_service_source_parse")
                .unwrap().call(&mut session.store, (id, pointer, input.len() as u32, format)).unwrap();
            session.memory.write(&mut session.store, pointer as usize, &[b'x'; 64]).unwrap();
            assert_eq!(session.output().word(packet as u64 + 12).unwrap(), 0);
            let value = session.instance.get_typed_func::<(u32, u32), u32>(&session.store, "telora_materialize_data")
                .unwrap().call(&mut session.store, (packet, 0)).unwrap();
            assert_eq!(session.output_value(Value { pointer: value, ty: session.manifest.value_type.unwrap() }).unwrap(),
                serde_json::json!({"value":42}));
            retained.push(value);
            assert_eq!(session.output().word(value as u64).unwrap(), id);
            let record = session.instance.get_typed_func::<u32, u32>(&session.store, "telora_source_range")
                .unwrap().call(&mut session.store, value).unwrap();
            assert_eq!(session.output().word(record as u64).unwrap(), id);
            session.instance.get_typed_func::<(u32, u32), u32>(&session.store, "telora_service_source_store")
                .unwrap().call(&mut session.store, (id, value)).unwrap();
        }
        session.instance.get_typed_func::<(u32, u32, u32), ()>(&session.store, "mem-free")
            .unwrap().call(&mut session.store, (pointer, 64, 1)).unwrap();
        for pointer in retained {
            assert_eq!(session.output_value(Value { pointer, ty: session.manifest.value_type.unwrap() }).unwrap(),
                serde_json::json!({"value":42}));
        }
        let seal = session.instance.get_typed_func::<(), u32>(&session.store, "telora_service_sources_seal").unwrap();
        assert_eq!(seal.call(&mut session.store, ()).unwrap(), 0);
        assert!(seal.call(&mut session.store, ()).is_err());
    }
}

#[test]
fn guest_slot_failures_are_terminal_and_bad_format_always_traps() {
    let bytes = artifact();
    let (mut session, ids) = prepare(&bytes);
    let packet = parse(&mut session, ids[0], b"{", 1).unwrap();
    assert_ne!(session.output().word(packet as u64 + 12).unwrap(), 0);
    let seal = session.instance.get_typed_func::<(), u32>(&session.store, "telora_service_sources_seal").unwrap();
    assert_eq!(seal.call(&mut session.store, ()).unwrap(), 1);
    assert!(parse(&mut session, ids[0], b"{}", 1).is_err());
    let (mut session, _) = prepare(&bytes);
    let seal = session.instance.get_typed_func::<(), u32>(&session.store, "telora_service_sources_seal").unwrap();
    assert_eq!(seal.call(&mut session.store, ()).unwrap(), 1);
    let (mut session, ids) = prepare(&bytes);
    assert!(parse(&mut session, ids[0], &[255], 99).is_err());
}

#[test]
fn public_source_injection_materializes_values_without_host_type_access() {
    let bytes = artifact();
    let (mut session, ids) = prepare(&bytes);
    let pointer = session.instance.get_typed_func::<(u32, u32), u32>(&session.store, "mem-alloc")
        .unwrap().call(&mut session.store, (64, 1)).unwrap();
    let inject = session.instance.get_typed_func::<(u32, u32, u32, u32), ()>(&session.store, "set-data-source").unwrap();
    for id in ids {
        session.memory.write(&mut session.store, pointer as usize, b"{\"value\":42}").unwrap();
        inject.call(&mut session.store, (id, pointer, 12, 1)).unwrap();
        session.memory.write(&mut session.store, pointer as usize, &[b'x'; 64]).unwrap();
    }
    session.instance.get_typed_func::<(u32, u32, u32), ()>(&session.store, "mem-free")
        .unwrap().call(&mut session.store, (pointer, 64, 1)).unwrap();
    let create = session.instance.get_typed_func::<(), i32>(&session.store, "create-service").unwrap();
    assert_eq!(create.call(&mut session.store, ()).unwrap(), 0);
    let alloc = session.instance.get_typed_func::<(u32, u32), u32>(&session.store, "mem-alloc").unwrap();
    let free = session.instance.get_typed_func::<(u32, u32, u32), ()>(&session.store, "mem-free").unwrap();
    let input = alloc.call(&mut session.store, (64, 1)).unwrap();
    let result = alloc.call(&mut session.store, (12, 4)).unwrap();
    let run = session.instance.get_typed_func::<(u32, u32, u32, u32, u32), ()>(&session.store, "run-service").unwrap();
    let (mut output, mut cap) = (1, 0);
    for request in ["1", "null", "{", r#""bytes""#, "2"] {
        session.memory.write(&mut session.store, input as usize, request.as_bytes()).unwrap();
        run.call(&mut session.store, (input, request.len() as u32, output, cap, result)).unwrap();
        output = session.output().word(result as u64).unwrap();
        let length = session.output().word(result as u64 + 4).unwrap();
        cap = session.output().word(result as u64 + 8).unwrap();
        assert!(length <= cap);
        let reply: serde_json::Value = serde_json::from_slice(
            session.output().bytes(output as u64, length as u64).unwrap()).unwrap();
        assert_eq!(reply["schema"], "telora.service/v1");
        if request == "null" || request == "{" || request == r#""bytes""# {
            assert_eq!(reply["error"], true);
            assert!(!reply["diagnostics"].as_array().unwrap().is_empty());
        } else {
            assert_eq!(reply["error"], false);
            assert_eq!(reply["ok"][0], serde_json::json!({"value":42}));
            assert_eq!(reply["ok"][1], request.parse::<i64>().unwrap());
        }
    }
    free.call(&mut session.store, (output, cap, 1)).unwrap();
    free.call(&mut session.store, (input, 64, 1)).unwrap();
    free.call(&mut session.store, (result, 12, 4)).unwrap();
    assert!(create.call(&mut session.store, ()).is_err());
    let (mut session, _) = prepare(&bytes);
    let create = session.instance.get_typed_func::<(), i32>(&session.store, "create-service").unwrap();
    assert_eq!(create.call(&mut session.store, ()).unwrap(), 1);
    assert_eq!(create.call(&mut session.store, ()).unwrap(), 1);
}
