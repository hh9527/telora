#[test]
fn host_buffers_share_allocator_layout_and_preserve_reallocated_bytes() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/runtime-gaps.telora")).unwrap();
    let bytes = super::compile(&source).unwrap();
    for align in [1, 2, 8, 16, 64] {
        let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
        let alloc = session.instance.get_typed_func::<(u32, u32), u32>(&session.store, "mem-alloc").unwrap();
        let free = session.instance.get_typed_func::<(u32, u32, u32), ()>(&session.store, "mem-free").unwrap();
        let realloc = session.instance.get_typed_func::<(u32, u32, u32, u32), u32>(&session.store, "mem-realloc").unwrap();
        assert_eq!(alloc.call(&mut session.store, (0, align)).unwrap(), align);
        free.call(&mut session.store, (align, 0, align)).unwrap();
        let pointer = realloc.call(&mut session.store, (align, 0, 7, align)).unwrap();
        assert_ne!(pointer, 0);
        assert_eq!(pointer % align, 0);
        session.memory.write(&mut session.store, pointer as usize, b"abcdefg").unwrap();
        let grown = realloc.call(&mut session.store, (pointer, 7, 25, align)).unwrap();
        assert_eq!(grown % align, 0);
        assert_eq!(&session.memory.data(&session.store)[grown as usize..grown as usize + 7], b"abcdefg");
        let shrunk = realloc.call(&mut session.store, (grown, 25, 3, align)).unwrap();
        assert_eq!(shrunk % align, 0);
        assert_eq!(&session.memory.data(&session.store)[shrunk as usize..shrunk as usize + 3], b"abc");
        assert_eq!(realloc.call(&mut session.store, (shrunk, 3, 0, align)).unwrap(), align);
    }
    for (pointer, cap, align) in [(0, 0, 1), (8, 0, 1), (9, 8, 8), (8, 7, 0), (8, 7, 3), (u32::MAX - 7, 8, 8)] {
        let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
        let free = session.instance.get_typed_func::<(u32, u32, u32), ()>(&session.store, "mem-free").unwrap();
        assert!(free.call(&mut session.store, (pointer, cap, align)).is_err());
    }
    for (cap, align) in [(1, 0), (1, 3), (u32::MAX, 1)] {
        let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
        let alloc = session.instance.get_typed_func::<(u32, u32), u32>(&session.store, "mem-alloc").unwrap();
        assert!(alloc.call(&mut session.store, (cap, align)).is_err());
    }
}
