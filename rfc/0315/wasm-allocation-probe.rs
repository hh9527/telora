//! Real wasm32/default-allocator probe for the production allocation registry.
extern crate alloc;
#[path = "../../crates/telora-wasm-shared/src/direct_heap.rs"]
mod direct_heap;
use direct_heap::Heap;
static mut HEAP: Option<Heap> = None;
unsafe fn heap() -> &'static mut Heap {
    unsafe { (&mut *core::ptr::addr_of_mut!(HEAP)).as_mut().unwrap() }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn configure(limit: u32) {
    unsafe {
        HEAP = Some(Heap::new(limit as usize));
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn allocate(bytes: u32) -> u32 {
    unsafe { heap().allocate(bytes as usize, 0).unwrap() as u32 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mark(reference: u32) {
    unsafe {
        heap().mark(reference as usize).unwrap();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sweep() {
    unsafe {
        heap().sweep(|_| {});
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bytes() -> u32 {
    unsafe { heap().usage().allocated as u32 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn objects() -> u32 {
    unsafe { heap().usage().objects as u32 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn store(reference: u32, value: u32) {
    unsafe {
        heap().bytes_mut(reference as usize, 1).unwrap()[0] = value as u8;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn load(reference: u32) -> u32 {
    unsafe { heap().bytes(reference as usize, 1).unwrap()[0] as u32 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn owner(reference: u32) -> u32 {
    unsafe { heap().block(reference as usize).unwrap().address as u32 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn set_limit(limit: u32) -> u32 {
    unsafe { u32::from(heap().set_limit(limit as usize).is_ok()) }
}
