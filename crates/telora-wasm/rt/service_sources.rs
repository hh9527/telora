//! Guest-owned initialization slots. Names come from the sealed entry's property
//! result; Host supplies bytes, never identities or materialized language values.
use alloc::{format, vec::Vec};
use crate::{abi::*, tables::telora_table_get, values::{string_span, word}};

#[repr(C)]
struct Slot {
    id: u32,
    name: u32,
    length: u32,
    key: u32,
    value: u32,
    supplied: bool,
}

struct Sources {
    slots: Vec<Slot>,
    failed: bool,
    sealed: bool,
}

static mut SOURCES: Option<Sources> = None;

unsafe fn sources() -> &'static mut Sources {
    unsafe { (&mut *core::ptr::addr_of_mut!(SOURCES)).as_mut().expect("service sources not prepared") }
}

/// Internal generated-entry glue. Must run once, before injection and freeze.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_sources_prepare(names: u32) -> u32 {
    unsafe {
        assert!((&*core::ptr::addr_of!(SOURCES)).is_none());
        let base = word(telora_table_get(table_address(ARRAYS), word(names, DATA)), 0);
        let start = word(names, DATA + 4);
        let end = word(names, DATA + 8);
        assert!(start <= end);
        let mut slots = Vec::new();
        let mut previous = None;
        for index in start..end {
            let key = base.checked_add(index.checked_mul(STRING_BYTES).unwrap()).unwrap();
            let (pointer, length) = string_span(key);
            let text = core::str::from_utf8(core::slice::from_raw_parts(pointer as *const u8, length as usize)).unwrap();
            if let Some(previous) = previous { assert!(previous < text); }
            previous = Some(text);
            // Inline String bytes are not necessarily aligned. Public names are.
            let name = if length == 0 { 8 } else { crate::telora_alloc(length) };
            core::ptr::copy_nonoverlapping(pointer as *const u8, name as *mut u8, length as usize);
            let diagnostic_name = format!("@service/{text}");
            let diagnostic_pointer = crate::telora_alloc(diagnostic_name.len().try_into().unwrap());
            core::ptr::copy_nonoverlapping(diagnostic_name.as_ptr(), diagnostic_pointer as *mut u8, diagnostic_name.len());
            let id = crate::sources::next_id();
            assert_eq!(crate::sources::telora_register_source(id, diagnostic_pointer, diagnostic_name.len().try_into().unwrap()), 1);
            slots.push(Slot { id, name, length, key, value: 0, supplied: false });
        }
        *core::ptr::addr_of_mut!(SOURCES) = Some(Sources { slots, failed: false, sealed: false });
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_source_count() -> u32 {
    unsafe { sources().slots.len().try_into().unwrap() }
}

/// Internal descriptor; generated wrapper loads three words for Wasm multi-value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_source_name(index: u32) -> u32 {
    unsafe { &sources().slots[index as usize] as *const Slot as u32 }
}

/// Reserve the slot before parsing. Failed input cannot be replaced/retried.
/// The generated wrapper materializes a successful plan, then stores its value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_source_parse(id: u32, pointer: u32, length: u32, format: u32) -> u32 {
    unsafe {
        let state = sources();
        assert!(!state.sealed && !state.failed);
        let slot = state.slots.iter_mut().find(|slot| slot.id == id).expect("unknown source slot");
        assert!(!slot.supplied);
        slot.supplied = true;
        let packet = crate::data_parse::telora_parse_data(pointer, length, format, id);
        if word(packet, 12) != 0 { state.failed = true; }
        packet
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_source_store(id: u32, value: u32) -> u32 {
    unsafe {
        let state = sources();
        assert!(!state.sealed && !state.failed);
        let slot = state.slots.iter_mut().find(|slot| slot.id == id).expect("unknown source slot");
        assert!(slot.supplied && slot.value == 0);
        slot.value = value;
        if value == 0 { state.failed = true; }
        value
    }
}

/// Close injection before user init. Returns status only; values stay in Guest.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_sources_seal() -> u32 {
    unsafe {
        let state = sources();
        if state.failed { return 1; }
        assert!(!state.sealed);
        state.sealed = true;
        state.failed = state.slots.iter().any(|slot| slot.value == 0);
        u32::from(state.failed)
    }
}
