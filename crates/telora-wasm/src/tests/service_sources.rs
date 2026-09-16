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
    session.initialize().unwrap();
    let plan = Value { pointer: session.entry().unwrap(), ty: session.manifest.entry_type };
    let (names, _) = session.pair(plan).unwrap();
    session.instance.get_typed_func::<u32, u32>(&session.store, "telora_service_sources_prepare")
        .unwrap().call(&mut session.store, names.pointer).unwrap();
    let count = session.instance.get_typed_func::<(), u32>(&session.store, "telora_service_source_count")
        .unwrap().call(&mut session.store, ()).unwrap();
    assert_eq!(count, 2);
    let mut ids = Vec::new();
    for (index, expected) in ["a", "b"].into_iter().enumerate() {
        let descriptor = session.instance.get_typed_func::<u32, u32>(&session.store, "telora_service_source_name")
            .unwrap().call(&mut session.store, index as u32).unwrap();
        let output = session.output();
        let id = output.word(descriptor as u64).unwrap();
        let pointer = output.word(descriptor as u64 + 4).unwrap();
        let length = output.word(descriptor as u64 + 8).unwrap();
        assert_ne!(pointer, 0);
        assert_eq!(pointer % 8, 0);
        assert_eq!(output.bytes(pointer as u64, length as u64).unwrap(), expected.as_bytes());
        assert!(!session.manifest.sources.iter().any(|source| source.id == id));
        ids.push(id);
    }
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
            let loc = session.output().word(value as u64).unwrap();
            assert!(loc > 0 && loc < 0x8000_0000);
            let record = session.instance.get_typed_func::<u32, u32>(&session.store, "telora_location_get")
                .unwrap().call(&mut session.store, loc).unwrap();
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
