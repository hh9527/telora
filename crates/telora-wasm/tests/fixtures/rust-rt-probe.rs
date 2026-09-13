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
