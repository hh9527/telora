use crate::{abi::*, telora_alloc};

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Table {
    pub(crate) buffer: u32,
    pub(crate) length: u32,
    pub(crate) capacity: u32,
    pub(crate) frozen: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Slot {
    pub payload: u32,
    pub bytes: u32,
}

pub(crate) unsafe fn snapshot() -> [Table; TABLE_COUNT as usize] {
    unsafe { core::array::from_fn(|index| (table_address(index as u32) as *const Table).read()) }
}

pub(crate) unsafe fn restore(tables: [Table; TABLE_COUNT as usize]) {
    unsafe {
        assert!((*core::ptr::addr_of!(BASELINE)).is_none());
        for (index, table) in tables.into_iter().enumerate() {
            assert_eq!(table.length, table.frozen, "snapshot table is not frozen");
            (table_address(index as u32) as *mut Table).write(table);
        }
        BASELINE = Some(tables);
    }
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
        let id = (table.frozen..table.length)
            .find(|&id| crate::heap::read::<Slot>(table.buffer + id * 8).payload == 0)
            .unwrap_or(table.length);
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
        table.length = table.length.max(id + 1);
        crate::heap::write(address, table);
        id
    }
}

pub(crate) unsafe fn sweep(live: &alloc::collections::BTreeSet<(u32, u32, u32)>) {
    unsafe {
        for tag in 0..TABLE_COUNT {
            let address = table_address(tag);
            let mut table: Table = crate::heap::read(address);
            for id in table.frozen..table.length {
                if live.range((tag, id, 0)..=(tag, id, u32::MAX)).next().is_some() { continue; }
                let at = table.buffer + id * 8;
                let slot: Slot = crate::heap::read(at);
                if tag == REGEXES { crate::regex::release(slot.payload); }
                crate::heap::write(at, Slot { payload: 0, bytes: 0 });
            }
            while table.length > table.frozen && crate::heap::read::<Slot>(table.buffer + (table.length - 1) * 8).payload == 0 {
                table.length -= 1;
            }
            crate::heap::write(address, table);
        }
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
