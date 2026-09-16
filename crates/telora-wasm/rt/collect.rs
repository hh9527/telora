//! Event-boundary copying collector. All traversal follows closed physical types.
use crate::{
    abi::*,
    tables::{Slot, Table},
    values::word,
};
use alloc::{collections::BTreeMap, vec, vec::Vec};

pub(crate) unsafe fn freeze() {
    unsafe {
        crate::heap::freeze();
        crate::sources::freeze();
    }
}

pub(crate) struct Collector {
    pub types: u32,
    pub old: [Table; TABLE_COUNT as usize],
    pub slots: Vec<Vec<Slot>>,
    pub objects: BTreeMap<(u32, u32), u32>,
    pub values: BTreeMap<u32, u32>,
    pub pending: Vec<(u32, u32, u32, u32)>, // table, old pointer, destination offset, bytes
    pub sources: alloc::collections::BTreeSet<u32>,
}

impl Collector {
    pub fn reserve(&mut self, bytes: u32) -> u32 {
        unsafe { crate::telora_alloc(bytes) }
    }
    pub unsafe fn copy_bytes(&mut self, pointer: u32, bytes: u32) -> u32 {
        let at = self.reserve(bytes);
        unsafe {
            core::ptr::copy_nonoverlapping(
                pointer as *const u8,
                at as *mut u8,
                bytes as usize,
            );
        }
        at
    }
    pub fn put(&mut self, at: u32, value: u32) {
        unsafe { (at as *mut u32).write_unaligned(value); }
    }
    pub unsafe fn value(&mut self, pointer: u32) -> u32 {
        unsafe {
            if pointer == 0 || crate::heap::is_frozen(pointer) {
                return pointer;
            }
            if let Some(&at) = self.values.get(&pointer) {
                return at;
            }
            let ty = word(pointer, TYPE);
            self.trace_location(word(pointer, SOURCE));
            let bytes = word(self.types + ty * 20, 4);
            assert!(bytes >= HEADER_BYTES);
            let at = self.copy_bytes(pointer, bytes);
            self.values.insert(pointer, at);
            self.pending.push((VALUES, pointer, at, bytes));
            at
        }
    }
    pub unsafe fn object(&mut self, table: u32, id: u32) -> u32 {
        unsafe {
            let old = self.old[table as usize];
            assert!(id < old.length);
            if id < old.frozen {
                return id;
            }
            if let Some(&next) = self.objects.get(&(table, id)) {
                return next;
            }
            let slot = (old.buffer as *const Slot).add(id as usize).read();
            let next = self.slots[table as usize].len() as u32;
            self.objects.insert((table, id), next);
            // Publish forwarding before traversal, including cyclic environments.
            self.slots[table as usize].push(Slot {
                payload: 0,
                bytes: slot.bytes,
            });
            let at = if table == REGEXES {
                let pattern = crate::regex::pattern(slot.payload);
                self.copy_bytes(pattern.as_ptr() as u32, pattern.len() as u32)
            } else {
                self.copy_bytes(slot.payload, slot.bytes)
            };
            self.slots[table as usize][next as usize] = Slot {
                payload: at,
                bytes: if table == REGEXES {
                    crate::regex::pattern(slot.payload).len() as u32
                } else {
                    slot.bytes
                },
            };
            if table != REGEXES {
                self.pending.push((table, slot.payload, at, slot.bytes));
            }
            next
        }
    }
    pub unsafe fn finish(mut self) {
        unsafe {
            while let Some((table, old, at, bytes)) = self.pending.pop() {
                self.trace_object(table, old, at, bytes);
            }
            // Sources are Host metadata, but RT may render their names in captures.
            crate::sources::collect(&mut self);
            let mut tables = self.old;
            for i in 0..TABLE_COUNT as usize {
                let count = self.slots[i].len() as u32;
                let at = self.reserve(count * 8);
                for index in 0..count as usize {
                    let slot = self.slots[i][index];
                    self.put(at + index as u32 * 8, slot.payload);
                    self.put(at + index as u32 * 8 + 4, slot.bytes);
                }
                tables[i].buffer = at;
                tables[i].length = count;
                tables[i].capacity = count;
            }
            for i in 0..TABLE_COUNT {
                (table_address(i) as *mut Table).write(tables[i as usize]);
            }
            let old_regex = self.old[REGEXES as usize];
            for id in old_regex.frozen..old_regex.length {
                let slot = (old_regex.buffer as *const Slot).add(id as usize).read();
                crate::regex::release(slot.payload);
            }
            let regex = tables[REGEXES as usize];
            for id in regex.frozen..regex.length {
                let slot = (regex.buffer as *mut Slot).add(id as usize);
                let text = core::str::from_utf8(core::slice::from_raw_parts(
                    (*slot).payload as *const u8,
                    (*slot).bytes as usize,
                ))
                .unwrap();
                *slot = crate::regex::restore(text);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_collect(types: u32, roots: u32, count: u32) -> u32 {
    unsafe {
        // Keep old blocks alive until all traversal and pointer patching finishes.
        let old_work = crate::heap::take_work();
        let old = core::array::from_fn(|i| (table_address(i as u32) as *const Table).read());
        let slots = old
            .iter()
            .map(|table| {
                if table.frozen == 0 {
                    vec![]
                } else {
                    core::slice::from_raw_parts(table.buffer as *const Slot, table.frozen as usize)
                        .to_vec()
                }
            })
            .collect();
        let mut gc = Collector {
            types,
            old,
            slots,
            objects: BTreeMap::new(),
            values: BTreeMap::new(),
            pending: vec![],
            sources: alloc::collections::BTreeSet::new(),
        };
        let result = gc.reserve(count * 4);
        for index in 0..count {
            let pointer = gc.value(word(roots, index as u64 * 4));
            gc.put(result + index * 4, pointer);
        }
        gc.finish();
        drop(old_work);
        result
    }
}
