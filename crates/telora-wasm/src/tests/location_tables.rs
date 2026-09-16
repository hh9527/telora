use telora_wasm_shared::locations::{LocationRecord, RECORD_BYTES, STATIC_BIT};

#[test]
fn guest_location_tables_are_embedded_and_initialization_ids_survive_growth() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/runtime-gaps.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    let get = session
        .instance
        .get_typed_func::<u32, u32>(&session.store, "telora_location_get")
        .unwrap();
    let add = session
        .instance
        .get_typed_func::<u32, u32>(&session.store, "telora_location_add")
        .unwrap();
    let alloc = session
        .instance
        .get_typed_func::<u32, u32>(&session.store, "telora_alloc")
        .unwrap();
    let static_pointer = get.call(&mut session.store, STATIC_BIT).unwrap();
    let read = |session: &crate::session::Session, pointer: u32| {
        LocationRecord::decode(&session.memory.data(&session.store)[pointer as usize..]).unwrap()
    };
    let static_record = read(&session, static_pointer);
    assert_ne!(static_record.source, 0);
    // Names are already present before Host registration or language initialization.
    let name = session
        .instance
        .get_typed_func::<u32, u32>(&session.store, "telora_source_name")
        .unwrap();
    let span = name.call(&mut session.store, static_record.source).unwrap() as usize;
    let memory = session.memory.data(&session.store);
    let pointer = u32::from_le_bytes(memory[span..span + 4].try_into().unwrap()) as usize;
    let length = u32::from_le_bytes(memory[span + 4..span + 8].try_into().unwrap()) as usize;
    let expected = &session
        .manifest
        .sources
        .iter()
        .find(|s| s.id == static_record.source)
        .unwrap()
        .name;
    assert_eq!(&memory[pointer..pointer + length], expected.as_bytes());
    let record = LocationRecord {
        source: 123,
        start_line: 70_000,
        start_offset: 1 << 25,
        end_line: 70_001,
        end_offset: 15,
    };
    let pointer = alloc.call(&mut session.store, RECORD_BYTES).unwrap();
    session
        .memory
        .write(&mut session.store, pointer as usize, &record.encode())
        .unwrap();
    let id = add.call(&mut session.store, pointer).unwrap();
    assert_eq!(id, 1);
    for _ in 0..100 {
        add.call(&mut session.store, pointer).unwrap();
    }
    let relocated = get.call(&mut session.store, id).unwrap();
    assert_eq!(read(&session, relocated), record);
    assert_eq!(
        get.call(&mut session.store, STATIC_BIT).unwrap(),
        static_pointer
    );
    assert_eq!(read(&session, static_pointer), static_record);
    assert_eq!(get.call(&mut session.store, 0).unwrap(), 0);
    session.initialize().unwrap();
    let pointer_after_initialize = get.call(&mut session.store, id).unwrap();
    assert_eq!(read(&session, pointer_after_initialize), record);
    assert!(add.call(&mut session.store, pointer).is_err());
}


#[test]
fn guest_parsers_register_initialization_spans_but_not_request_spans() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/runtime-gaps.telora"
    )).unwrap();
    let bytes = super::compile(&text).unwrap();
    for (format, input, expected_line, expected_offset) in [
        (1, "{\r\n  \"a\": 42\r\n}", 1, 7),
        (2, "a: 42\r\n", 0, 3),
        (3, "a = 42\r\n", 0, 4),
    ] {
        let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
        let source = session.manifest.sources[0].id;
        let parse = session.instance
            .get_typed_func::<(u32, u32, u32, u32), u32>(&session.store, "telora_parse_data")
            .unwrap();
        let pointer = session.allocate(input.len()).unwrap();
        session.write(pointer as usize, input.as_bytes()).unwrap();
        let packet = parse.call(&mut session.store, (pointer, input.len() as u32, format, source)).unwrap();
        let output = session.output();
        assert_eq!(output.word(packet as u64 + 12).unwrap(), 0);
        let rows = output.word(packet as u64).unwrap();
        let count = output.word(packet as u64 + 4).unwrap();
        let root = output.word(packet as u64 + 8).unwrap();
        let integer = (0..count).find(|i| output.word((rows + i * 16) as u64).unwrap() == 3).unwrap();
        let loc = output.word((rows + integer * 16 + 4) as u64).unwrap();
        let record = output.location(loc).unwrap().unwrap();
        assert_eq!(record.source, source);
        assert_eq!((record.start_line, record.start_offset), (expected_line, expected_offset));
        assert_eq!((record.end_line, record.end_offset), (expected_line, expected_offset + 2));
        let entry = output.word((rows + root * 16 + 8) as u64).unwrap();
        let key = output.word(entry as u64 + 12).unwrap();
        assert_ne!(key, loc);
        let key = output.location(key).unwrap().unwrap();
        assert_eq!(key.source, source);
        assert_eq!(key.start_line, expected_line);
        assert!(key.end_offset <= expected_offset);
        let baseline = output.word(crate::abi::INITIALIZATION_LOCS as u64 + 4).unwrap();
        assert_eq!(baseline, count + 1);
        session.initialize().unwrap();
        // The table is now frozen. Requests parse normally with LocId 0.
        for _ in 0..8 {
            let packet = parse.call(&mut session.store, (pointer, input.len() as u32, format, 0)).unwrap();
            let output = session.output();
            assert_eq!(output.word(packet as u64 + 12).unwrap(), 0);
            let rows = output.word(packet as u64).unwrap();
            for i in 0..count {
                assert_eq!(output.word((rows + i * 16 + 4) as u64).unwrap(), 0);
            }
            let entry = output.word((rows + root * 16 + 8) as u64).unwrap();
            assert_eq!(output.word(entry as u64 + 12).unwrap(), 0);
            assert_eq!(output.word(crate::abi::INITIALIZATION_LOCS as u64 + 4).unwrap(), baseline);
        }
    }
}


#[test]
fn guest_input_materialization_owns_text_after_transfer_buffer_is_reused() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/semantic-value.telora"
    )).unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    let parse = session.instance
        .get_typed_func::<(u32, u32, u32, u32), u32>(&session.store, "telora_parse_data").unwrap();
    let materialize = session.instance
        .get_typed_func::<(u32, u32), u32>(&session.store, "telora_materialize_data").unwrap();
    let input = r#"{"long key outside inline storage": ["original long string é🦀", "escaped\ntext", 9223372036854775807]}"#;
    let pointer = session.allocate(input.len()).unwrap();
    session.write(pointer as usize, input.as_bytes()).unwrap();
    let source_id = session.manifest.sources[0].id;
    let packet = parse.call(&mut session.store, (pointer, input.len() as u32, 1, source_id)).unwrap();
    let value = materialize.call(&mut session.store, (packet, 0)).unwrap();
    session.write(pointer as usize, &vec![b'x'; input.len()]).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.output_value(crate::transport::Value {
        pointer: value, ty: session.manifest.value_type.unwrap(),
    }).unwrap(), serde_json::json!({
        "long key outside inline storage": ["original long string é🦀", "escaped\ntext", i64::MAX]
    }));
    let loc = session.output().word(value as u64).unwrap();
    assert_eq!(session.output().location(loc).unwrap().unwrap().source, source_id);
}
