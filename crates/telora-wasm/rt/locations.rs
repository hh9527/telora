//! Instance-owned static and initialization locations; no Host callbacks.
use telora_wasm_shared::{
    location_tables::LocationTables,
    locations::{LocId, LocationRecord},
};

static mut TABLES: Option<LocationTables<'static>> = None;

/// Internal linker bootstrap, not a Host-supplied location-table protocol.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_locations_bootstrap(pointer: u32, bytes: u32) {
    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(TABLES);
        assert!(slot.is_none());
        let input = core::slice::from_raw_parts(pointer as *const u8, bytes as usize);
        *slot = Some(LocationTables::new(input).unwrap());
        descriptor(crate::abi::STATIC_LOCS, pointer, bytes / 20);
        descriptor(crate::abi::INITIALIZATION_LOCS, 8, 0);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_location_get(id: u32) -> u32 {
    unsafe {
        let tables = (&*core::ptr::addr_of!(TABLES)).as_ref().unwrap();
        tables
            .bytes(LocId::from_bits(id))
            .unwrap()
            .map(|record| record.as_ptr() as u32)
            .unwrap_or(0)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_location_add(pointer: u32) -> u32 {
    unsafe {
        let bytes = core::slice::from_raw_parts(pointer as *const u8, 20);
        let record = LocationRecord::decode(bytes).unwrap();
        assert!(record.source != 0);
        assert!((record.start_line, record.start_offset) <= (record.end_line, record.end_offset));
        let tables = (&mut *core::ptr::addr_of_mut!(TABLES)).as_mut().unwrap();
        let id = tables.append(record).unwrap();
        let records = tables.initialization_storage();
        descriptor(
            crate::abi::INITIALIZATION_LOCS,
            records.as_ptr() as u32,
            records.len() as u32,
        );
        id.bits()
    }
}

pub unsafe fn freeze() {
    unsafe {
        (&mut *core::ptr::addr_of_mut!(TABLES))
            .as_mut()
            .unwrap()
            .freeze();
    }
}

unsafe fn descriptor(address: u32, base: u32, count: u32) {
    unsafe {
        (address as *mut u32).write(base);
        ((address + 4) as *mut u32).write(count);
    }
}
