use telora_wasm_shared::frame_scratch::Scratch;

static mut SCRATCH: Scratch = Scratch::new();

unsafe fn state() -> &'static mut Scratch {
    unsafe { &mut *core::ptr::addr_of_mut!(SCRATCH) }
}

pub unsafe fn address(reference: u32) -> u32 {
    unsafe { state().address(reference).expect("scratch reference") as u32 }
}

pub unsafe fn reset() { unsafe { state().reset(); crate::gc_runtime::reset_frames(); } }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_frame_enter(bytes: u32, args: u32) -> u32 {
    unsafe { state().enter(bytes, args).expect("scratch frame entry") }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_frame_leave() -> u32 {
    unsafe { state().leave().expect("scratch frame exit"); }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_frame_tail(packet: u32, bytes: u32, count: u32) -> u32 {
    unsafe { state().tail(packet, bytes, count).expect("scratch tail handoff") }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_frame_slot(reference: u32, bytes: u32) -> u32 {
    unsafe {
        state().address(reference.checked_add(bytes).unwrap()).expect("scratch slot extent");
        let pointer = state().address(reference).expect("scratch slot");
        core::ptr::write_bytes(pointer, 0, bytes as usize);
    }
    reference
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_frame_depth() -> u32 { unsafe { state().depth() as u32 } }

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_frame_bytes() -> u32 { unsafe { state().capacity_bytes() as u32 } }
