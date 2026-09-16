//! Host-owned buffers. Capacities, unlike content lengths, are multiples of eight.
const EMPTY: u32 = 8;

fn capacity(cap: u32) {
    assert_eq!(cap & 7, 0, "unaligned buffer capacity");
}

unsafe fn allocation(pointer: u32, cap: u32) {
    capacity(cap);
    assert_ne!(pointer, 0);
    assert_eq!(pointer & 7, 0);
    if cap == 0 {
        assert_eq!(pointer, EMPTY);
    } else {
        let end = pointer.checked_add(cap).expect("buffer extent overflow");
        assert!(pointer as usize >= core::ptr::addr_of!(crate::__heap_base) as usize);
        assert!(end <= unsafe { crate::telora_heap_end() });
    }
}

#[unsafe(export_name = "mem-alloc")]
pub unsafe extern "C" fn alloc(cap: u32) -> u32 {
    capacity(cap);
    if cap == 0 {
        EMPTY
    } else {
        unsafe { crate::telora_alloc(cap) }
    }
}

#[unsafe(export_name = "mem-free")]
pub unsafe extern "C" fn free(pointer: u32, cap: u32) {
    unsafe {
        allocation(pointer, cap);
    }
    // The current append-only arena reclaims storage at its lifetime boundary.
    // The caller's ownership ends here even when physical reclamation is deferred.
}

#[unsafe(export_name = "mem-realloc")]
pub unsafe extern "C" fn realloc(pointer: u32, old_cap: u32, new_cap: u32) -> u32 {
    unsafe {
        allocation(pointer, old_cap);
        capacity(new_cap);
        if new_cap == old_cap {
            return pointer;
        }
        if new_cap == 0 {
            free(pointer, old_cap);
            return EMPTY;
        }
        let next = alloc(new_cap);
        core::ptr::copy_nonoverlapping(
            pointer as *const u8,
            next as *mut u8,
            old_cap.min(new_cap) as usize,
        );
        free(pointer, old_cap);
        next
    }
}
