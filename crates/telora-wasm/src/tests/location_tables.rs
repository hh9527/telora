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
