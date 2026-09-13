//! Standalone Rust source: compile to a wasm32 object with rustc --emit=obj.
#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}

unsafe extern "C" {
    fn telora_callback(value: i64) -> i64;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_apply(value: i64) -> i64 {
    unsafe { telora_callback(value) }
}

// The linker owns static data and the Rust stack. Heap allocation begins only
// at its synthesized __heap_base, never at the old generated module's offset.
unsafe extern "C" {
    static __heap_base: u8;
}
static mut NEXT: usize = 0;

unsafe fn allocate(bytes: usize) -> *mut u8 {
    unsafe {
        let start = if NEXT == 0 { core::ptr::addr_of!(__heap_base) as usize } else { NEXT };
        let start = start.checked_add(7).unwrap() & !7;
        let end = start.checked_add(bytes).unwrap();
        let pages = end.div_ceil(65536);
        let current = core::arch::wasm32::memory_size::<0>();
        if pages > current {
            assert_ne!(core::arch::wasm32::memory_grow::<0>(pages - current), usize::MAX);
        }
        NEXT = end;
        start as *mut u8
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_map_sum(callback: unsafe extern "C" fn(i64) -> i64) -> i64 {
    unsafe {
        let values = allocate(24).cast::<i64>();
        for index in 0..3 {
            values.add(index).write(index as i64 + 1);
        }
        let mut total = 0;
        for index in 0..3 {
            total += callback(values.add(index).read());
        }
        total
    }
}
