//! Rust implementation of the Wasm runtime, statically linked into artifacts.
#![no_std]
extern crate alloc;
mod allocator;
mod locations;
mod host_memory;
mod collect;
mod collect_trace;
mod math;
mod regex;
mod regex_contract;
mod hash;
use telora_wasm_shared::json_text;
mod json_writer;
mod data_parse;

use telora_wasm_shared::abi;
mod format;
mod format_nodes;
mod path;
mod sort;
mod sources;
mod service_sources;
mod tables;
mod template;
mod text;
mod values;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}

unsafe extern "C" {
    static __heap_base: u8;
}
static mut NEXT: u64 = 0;

/// Primitive used only by the Rust global allocator. No call back into alloc.
/// The linker places the arena after static data and the Rust stack.
pub(crate) unsafe fn arena_alloc(bytes: u32) -> u32 {
    unsafe {
        let start = if NEXT == 0 {
            core::ptr::addr_of!(__heap_base) as u64
        } else {
            NEXT
        };
        let end = (start + u64::from(bytes) + 7) & !7;
        if end > 0xffff_fff8 {
            core::arch::wasm32::unreachable();
        }
        let pages = end.div_ceil(65536) as usize;
        let current = core::arch::wasm32::memory_size::<0>();
        if pages > current && core::arch::wasm32::memory_grow::<0>(pages - current) == usize::MAX {
            core::arch::wasm32::unreachable();
        }
        NEXT = end;
        start as u32
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_end() -> u32 { unsafe { NEXT as u32 } }

/// Called by the generated Wasm start function, before any allocation or input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_reserve_static(end: u32) {
    unsafe {
        assert!(NEXT == 0 && end as usize >= core::ptr::addr_of!(__heap_base) as usize);
        NEXT = end as u64;
    }
}

/// Generated values require zeroed storage; use the same allocator as Vec/String.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_alloc(bytes: u32) -> u32 {
    let layout = core::alloc::Layout::from_size_align(bytes.max(1) as usize, 8).unwrap();
    let pointer = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if pointer.is_null() { alloc::alloc::handle_alloc_error(layout); }
    pointer as u32
}
