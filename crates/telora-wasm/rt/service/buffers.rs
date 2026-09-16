//! Byte ownership at the Host boundary. Output Vec uses the same allocator and
//! align=1 layout as mem-alloc; result records are only borrowed for the call.
use alloc::vec::Vec;

pub(super) unsafe fn range(pointer: u32, length: u32, align: u32) -> (u32, u32) {
    assert_ne!(pointer, 0);
    assert_eq!(pointer % align, 0);
    let end = pointer.checked_add(length).expect("buffer range overflow");
    if length != 0 {
        assert!(pointer as usize >= core::ptr::addr_of!(crate::__heap_base) as usize);
        assert!(end <= unsafe { crate::telora_heap_end() });
    }
    (pointer, end)
}

fn separate(a: (u32, u32), b: (u32, u32)) {
    assert!(a.0 == a.1 || b.0 == b.1 || a.1 <= b.0 || b.1 <= a.0,
        "overlapping ABI buffers");
}

pub(super) unsafe fn take(input: u32, length: u32, output: u32, cap: u32, result: u32) -> Vec<u8> {
    unsafe {
        let input_range = range(input, length, 1);
        let output_range = range(output, cap, 1);
        let result_range = range(result, 12, 4);
        if cap == 0 { assert_eq!(output, 1); }
        core::alloc::Layout::from_size_align(cap as usize, 1).unwrap();
        separate(input_range, output_range);
        separate(input_range, result_range);
        separate(output_range, result_range);
        // No old output bytes are initialized for the new Vec's logical length.
        Vec::from_raw_parts(output as *mut u8, 0, cap as usize)
    }
}

pub(super) unsafe fn give(output: Vec<u8>, result: u32) {
    let mut output = core::mem::ManuallyDrop::new(output);
    let words = [output.as_mut_ptr() as u32, u32::try_from(output.len()).unwrap(),
        u32::try_from(output.capacity()).unwrap()];
    unsafe { core::ptr::copy_nonoverlapping(words.as_ptr(), result as *mut u32, 3) };
}
