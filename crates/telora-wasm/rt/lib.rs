//! Rust implementation of the Wasm runtime, statically linked into artifacts.
#![no_std]

#[allow(dead_code)]
mod abi;
mod sources;
mod tables;
mod values;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}

unsafe extern "C" {
    static __heap_base: u8;
}
static mut NEXT: u64 = 0;

/// wasm32 addresses; the linker places heap after static data and Rust stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_alloc(bytes: u32) -> u32 {
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
