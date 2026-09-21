//! Host buffers use the same Rust global allocator and layouts as Vec/String.
use core::alloc::Layout;

fn layout(cap: u32, align: u32) -> Layout {
    Layout::from_size_align(cap as usize, align as usize).expect("invalid buffer layout")
}

unsafe fn allocation(pointer: u32, cap: u32, align: u32) -> Layout {
    let layout = layout(cap, align);
    assert_ne!(pointer, 0);
    assert_eq!(pointer % align, 0);
    if cap == 0 {
        assert_eq!(pointer, align, "invalid empty buffer sentinel");
    } else {
        let end = pointer.checked_add(cap).expect("buffer extent overflow");
        assert!(pointer as usize >= core::ptr::addr_of!(crate::__heap_base) as usize);
        assert!(end <= unsafe { crate::telora_heap_end() });
    }
    layout
}

pub(crate) unsafe fn range(pointer: u32, length: u32, align: u32) {
    unsafe {
        allocation(pointer, length, align);
    }
}

#[unsafe(export_name = "mem-alloc")]
pub unsafe extern "C" fn alloc(cap: u32, align: u32) -> u32 {
    let layout = layout(cap, align);
    if cap == 0 { return align; }
    let pointer = unsafe { alloc::alloc::alloc(layout) };
    if pointer.is_null() { alloc::alloc::handle_alloc_error(layout); }
    pointer as u32
}

#[unsafe(export_name = "mem-free")]
pub unsafe extern "C" fn free(pointer: u32, cap: u32, align: u32) {
    let layout = unsafe { allocation(pointer, cap, align) };
    if cap != 0 { unsafe { alloc::alloc::dealloc(pointer as *mut u8, layout) }; }
}

/// Reallocation preserves alignment. To change it, allocate/copy/free explicitly.
#[unsafe(export_name = "mem-realloc")]
pub unsafe extern "C" fn realloc(pointer: u32, old_cap: u32, new_cap: u32, align: u32) -> u32 {
    unsafe {
        let old = allocation(pointer, old_cap, align);
        let new = layout(new_cap, align);
        if new_cap == old_cap { return pointer; }
        if new_cap == 0 {
            free(pointer, old_cap, align);
            return align;
        }
        if old_cap == 0 { return alloc(new_cap, align); }
        let next = alloc::alloc::realloc(pointer as *mut u8, old, new_cap as usize);
        if next.is_null() { alloc::alloc::handle_alloc_error(new); }
        next as u32
    }
}
