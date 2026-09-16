use crate::{session::Session, transport::Value};

fn artifact() -> Vec<u8> {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/semantic-value.telora")).unwrap();
    super::compile(&source).unwrap()
}

fn register(session: &mut Session, input: &str) -> u32 {
    let id = session.manifest.sources.iter().map(|source| source.id).max().unwrap() + 1;
    let lines = telora_core::source::LineIndex::new(input).unwrap().ranges().collect();
    session.manifest.sources.push(crate::artifact::Source { id, name: "@test/input".into(), lines });
    session.register_sources().unwrap();
    id
}

#[test]
fn parsers_write_inline_ranges_and_requests_have_no_source() {
    let bytes = artifact();
    for (format, input, line, offset) in [
        (1, "{\r\n  \"a\": 42\r\n}", 1, 7),
        (2, "a: 42\r\n", 0, 3),
        (3, "a = 42\r\n", 0, 4),
    ] {
        let mut session = Session::load(&bytes, 10_000_000).unwrap();
        for name in ["telora_location_get", "telora_location_add", "telora_locations_bootstrap"] {
            assert!(session.instance.get_export(&session.store, name).is_none());
        }
        let source = register(&mut session, input);
        let parse = session.instance.get_typed_func::<(u32,u32,u32,u32),u32>(
            &session.store, "telora_parse_data").unwrap();
        let pointer = session.exports.alloc.call(&mut session.store, (input.len() as u32, 1)).unwrap();
        session.memory.write(&mut session.store, pointer as usize, input.as_bytes()).unwrap();
        let packet = parse.call(&mut session.store, (pointer, input.len() as u32, format, source)).unwrap();
        let output = session.output();
        assert_eq!(output.word(packet as u64 + 12).unwrap(), 0);
        let rows = output.word(packet as u64).unwrap();
        let count = output.word(packet as u64 + 4).unwrap();
        let integer = (0..count).find(|i| output.word((rows + i * 24) as u64).unwrap() == 3).unwrap();
        let origin = (rows + integer * 24 + 4) as u64;
        assert_eq!(output.word(origin + 4).unwrap(), input.find("42").unwrap() as u32);
        assert_eq!(output.location_words(origin).unwrap(), [source, line, offset, line, offset + 2]);
        session.initialize().unwrap();
        for _ in 0..8 {
            let packet = parse.call(&mut session.store, (pointer, input.len() as u32, format, 0)).unwrap();
            let output = session.output();
            let rows = output.word(packet as u64).unwrap();
            for index in 0..count {
                for field in [4, 8, 12] {
                    assert_eq!(output.word((rows + index * 24 + field) as u64).unwrap(), 0);
                }
            }
        }
        session.exports.free.call(&mut session.store, (pointer, input.len() as u32, 1)).unwrap();
    }
}

#[test]
fn guest_input_owns_text_after_transfer_buffer_is_reused() {
    let mut session = Session::load(&artifact(), 10_000_000).unwrap();
    let input = r#"{"long key outside inline storage": ["original long string é🦀", "escaped\ntext", 9223372036854775807]}"#;
    let source = register(&mut session, input);
    let pointer = session.exports.alloc.call(&mut session.store, (input.len() as u32, 1)).unwrap();
    session.memory.write(&mut session.store, pointer as usize, input.as_bytes()).unwrap();
    let parse = session.instance.get_typed_func::<(u32,u32,u32,u32),u32>(&session.store, "telora_parse_data").unwrap();
    let materialize = session.instance.get_typed_func::<(u32,u32),u32>(&session.store, "telora_materialize_data").unwrap();
    let packet = parse.call(&mut session.store, (pointer, input.len() as u32, 1, source)).unwrap();
    let value = materialize.call(&mut session.store, (packet, 0)).unwrap();
    session.memory.write(&mut session.store, pointer as usize, &vec![b'x'; input.len()]).unwrap();
    session.exports.free.call(&mut session.store, (pointer, input.len() as u32, 1)).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.output_value(Value { pointer: value, ty: session.manifest.value_type.unwrap() }).unwrap(),
        serde_json::json!({"long key outside inline storage": ["original long string é🦀", "escaped\ntext", i64::MAX]}));
    assert_eq!(session.output().word(value as u64).unwrap(), source);
}

#[test]
fn diagnostic_ranges_keep_full_width_columns_and_reject_invalid_origins() {
    let bytes = artifact();
    let mut session = Session::load(&bytes, 10_000_000).unwrap();
    let id = session.manifest.sources.iter().map(|source| source.id).max().unwrap() + 1;
    // Synthetic metadata exercises wasm32 boundaries without allocating a 4 GiB file.
    session.manifest.sources.push(crate::artifact::Source {
        id, name: "wide-input".into(), lines: vec![[0, u32::MAX]],
    });
    session.register_sources().unwrap();
    let pointer = session.allocate(12).unwrap();
    let expand = session.instance.get_typed_func::<u32, u32>(
        &session.store, "telora_source_range").unwrap();
    for (index, word) in [id, u32::MAX - 1, u32::MAX].into_iter().enumerate() {
        session.write(pointer as usize + index * 4, &word.to_le_bytes()).unwrap();
    }
    let result = expand.call(&mut session.store, pointer).unwrap();
    assert_eq!(session.output().word(result as u64 + 8).unwrap(), u32::MAX - 1);
    assert_eq!(session.output().word(result as u64 + 16).unwrap(), u32::MAX);
    for range in [[0u32, 0, 1], [1, 2, 1], [u32::MAX, 0, 0]] {
        let mut session = Session::load(&bytes, 10_000_000).unwrap();
        let pointer = session.allocate(12).unwrap();
        for (index, word) in range.into_iter().enumerate() {
            session.write(pointer as usize + index * 4, &word.to_le_bytes()).unwrap();
        }
        let expand = session.instance.get_typed_func::<u32, u32>(
            &session.store, "telora_source_range").unwrap();
        assert!(expand.call(&mut session.store, pointer).is_err());
    }
}

#[test]
fn host_native_invocation_has_no_fabricated_computation_source() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native-origin.telora")).unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let callable = Value { pointer: session.entry().unwrap(), ty: session.manifest.entry_type };
    let input_ty = session.manifest.types[callable.ty as usize].arguments[0];
    let input = session.input_value(input_ty, &serde_json::json!("abc")).unwrap();
    let result = session.invoke_values(callable, &[input]).unwrap();
    assert_eq!(session.output_value(result).unwrap(), serde_json::json!(3));
    assert_eq!(session.output().location_words(result.pointer as u64).unwrap(), [0; 5]);
}
