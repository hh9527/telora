//! Rust implementation of the Wasm runtime, statically linked into artifacts.
extern crate alloc;
mod heap;
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
mod service;
mod tables;
mod template;
mod text;
mod values;

unsafe extern "C" {
    static __heap_base: u8;
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_end() -> u32 {
    u32::try_from(core::arch::wasm32::memory_size::<0>() as u64 * 65536).unwrap_or(u32::MAX)
}

/// Called by the generated Wasm start function, before any allocation or input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_reserve_static(end: u32) {
    unsafe {
        assert!(end as usize >= core::ptr::addr_of!(__heap_base) as usize);
        assert!(end <= telora_heap_end());
        heap::set_static_end(end);
    }
}

/// Generated values require zeroed storage; use the same allocator as Vec/String.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_alloc(bytes: u32) -> u32 {
    unsafe { heap::allocate(bytes) }
}
