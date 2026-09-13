//! Library allocations share the session's append-only Wasm heap.
use core::alloc::{GlobalAlloc, Layout};

struct ArenaAllocator;

unsafe impl GlobalAlloc for ArenaAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align();
        let bytes = layout
            .size()
            .checked_add(align - 1)
            .and_then(|bytes| u32::try_from(bytes).ok())
            .unwrap_or_else(|| core::arch::wasm32::unreachable());
        let raw = unsafe { crate::telora_alloc(bytes) } as usize;
        ((raw + align - 1) & !(align - 1)) as *mut u8
    }

    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {
        // Main and work allocations live for this Wasm instance's lifetime.
    }
}

#[global_allocator]
static ALLOCATOR: ArenaAllocator = ArenaAllocator;
