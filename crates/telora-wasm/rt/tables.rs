use crate::{abi::*, telora_alloc};

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Table {
    pub buffer: u32,
    pub length: u32,
    pub capacity: u32,
    pub frozen: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Slot {
    pub payload: u32,
    pub bytes: u32,
}

static mut BASELINE: Option<[Table; TABLE_COUNT as usize]> = None;

pub(crate) unsafe fn reset() {
    unsafe {
        let baseline = (*core::ptr::addr_of!(BASELINE)).expect("tables not frozen");
        let regex = (table_address(REGEXES) as *const Table).read();
        for id in regex.frozen..regex.length {
            let slot: Slot = crate::heap::read(regex.buffer + id * 8);
            crate::regex::release(slot.payload);
        }
        for (index, table) in baseline.into_iter().enumerate() {
            (table_address(index as u32) as *mut Table).write(table);
        }
        crate::heap::reset();
        crate::content::reset();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_table_push(address: u32, payload: u32, bytes: u32) -> u32 {
    unsafe {
        let mut table = crate::heap::read::<Table>(address);
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
                    crate::heap::ptr::<Slot>(table.buffer),
                    crate::heap::ptr::<Slot>(buffer),
                    id as usize,
                );
            }
            table.buffer = buffer;
            table.capacity = capacity;
        }
        crate::heap::ptr::<Slot>(table.buffer)
            .add(id as usize)
            .write(Slot { payload, bytes });
        table.length = id + 1;
        crate::heap::write(address, table);
        id
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_table_get(address: u32, id: u32) -> u32 {
    unsafe {
        let table = crate::heap::read::<Table>(address);
        if id >= table.length {
            core::arch::wasm32::unreachable();
        }
        table.buffer + id * 8
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_freeze() -> u32 {
    unsafe {
        crate::collect::collect_initialization();
        for index in 0..TABLE_COUNT {
            let table = &mut *(table_address(index) as *mut Table);
            table.frozen = table.length;
        }
        crate::collect::freeze();
        BASELINE = Some(core::array::from_fn(|index|
            (table_address(index as u32) as *const Table).read()));
        1
    }
}
