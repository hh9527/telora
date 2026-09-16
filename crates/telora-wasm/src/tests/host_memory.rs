#[test]
fn host_buffers_use_nonzero_aligned_move_contract() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/runtime-gaps.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    let alloc = session
        .instance
        .get_typed_func::<u32, u32>(&session.store, "mem-alloc")
        .unwrap();
    let free = session
        .instance
        .get_typed_func::<(u32, u32), ()>(&session.store, "mem-free")
        .unwrap();
    let realloc = session
        .instance
        .get_typed_func::<(u32, u32, u32), u32>(&session.store, "mem-realloc")
        .unwrap();
    assert_eq!(alloc.call(&mut session.store, 0).unwrap(), 8);
    free.call(&mut session.store, (8, 0)).unwrap();
    let pointer = realloc.call(&mut session.store, (8, 0, 8)).unwrap();
    assert_ne!(pointer, 0);
    assert_eq!(pointer % 8, 0);
    session
        .memory
        .write(&mut session.store, pointer as usize, b"abcdefgh")
        .unwrap();
    let grown = realloc.call(&mut session.store, (pointer, 8, 24)).unwrap();
    assert_eq!(
        &session.memory.data(&session.store)[grown as usize..grown as usize + 8],
        b"abcdefgh"
    );
    let shrunk = realloc.call(&mut session.store, (grown, 24, 8)).unwrap();
    assert_eq!(
        &session.memory.data(&session.store)[shrunk as usize..shrunk as usize + 8],
        b"abcdefgh"
    );
    assert_eq!(realloc.call(&mut session.store, (shrunk, 8, 0)).unwrap(), 8);
    assert!(alloc.call(&mut session.store, 7).is_err());
    // Every trap ends this test instance. Independent instances test other violations.
    for (pointer, cap) in [(0, 0), (16, 0), (9, 8), (8, 7), (u32::MAX - 7, 8)] {
        let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
        let free = session
            .instance
            .get_typed_func::<(u32, u32), ()>(&session.store, "mem-free")
            .unwrap();
        assert!(free.call(&mut session.store, (pointer, cap)).is_err());
    }
}
