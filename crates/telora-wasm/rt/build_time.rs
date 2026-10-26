//! Explicit Host-owned build scope; ordinary execution never opens it.
static mut ACTIVE: bool = false;
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_build_time_scope(active: u32) {
    unsafe {
        assert!(active <= 1);
        assert!(ACTIVE != (active != 0), "invalid build scope transition");
        ACTIVE = active != 0;
        if ACTIVE {
            crate::information::open();
        } else {
            crate::information::close();
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_build_time_active() -> u32 {
    unsafe { ACTIVE as u32 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_build_information_publish(name: u32, value: u32) -> u32 {
    unsafe {
        if !ACTIVE {
            return 0;
        }
        crate::information::telora_service_metadata_publish(name, value)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_build_information_export(result: u32) {
    unsafe {
        assert!(!ACTIVE);
        crate::information::telora_service_metadata_export(result);
    }
}
