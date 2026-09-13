use crate::{abi::*, telora_alloc};

#[repr(C)]
struct Table {
    buffer: u32,
    length: u32,
    capacity: u32,
    frozen: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    payload: u32,
    bytes: u32,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_table_push(address: u32, payload: u32, bytes: u32) -> u32 {
    unsafe {
        let table = &mut *(address as *mut Table);
        let id = table.length;
        if id == table.capacity {
            if table.capacity > 0x0fff_ffff {
                core::arch::wasm32::unreachable();
            }
            let capacity = if table.capacity == 0 {
                8
            } else {
                table.capacity * 2
            };
            let buffer = telora_alloc(capacity * 8);
            if id != 0 {
                core::ptr::copy_nonoverlapping(
                    table.buffer as *const Slot,
                    buffer as *mut Slot,
                    id as usize,
                );
            }
            table.buffer = buffer;
            table.capacity = capacity;
        }
        (table.buffer as *mut Slot)
            .add(id as usize)
            .write(Slot { payload, bytes });
        table.length = id + 1;
        id
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_table_get(address: u32, id: u32) -> u32 {
    unsafe {
        let table = &*(address as *const Table);
        if id >= table.length {
            core::arch::wasm32::unreachable();
        }
        table.buffer + id * 8
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_freeze() -> u32 {
    unsafe {
        for index in 0..TABLE_COUNT {
            let table = &mut *(table_address(index) as *mut Table);
            table.frozen = table.length;
        }
        1
    }
}
