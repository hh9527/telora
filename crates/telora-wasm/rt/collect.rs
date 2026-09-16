//! Event-boundary copying collector. All traversal follows closed physical types.
use crate::{
    abi::*,
    tables::{Slot, Table},
    values::word,
};
use alloc::{collections::BTreeMap, vec, vec::Vec};

static mut MAIN_END: u32 = 0;
pub(crate) unsafe fn freeze() {
    unsafe {
        MAIN_END = crate::NEXT as u32;
        crate::sources::freeze();
    }
}

pub(crate) struct Collector {
    pub base: u32,
    pub types: u32,
    pub image: Vec<u8>,
    pub old: [Table; TABLE_COUNT as usize],
    pub slots: Vec<Vec<Slot>>,
    pub objects: BTreeMap<(u32, u32), u32>,
    pub values: BTreeMap<u32, u32>,
    pub environments: BTreeMap<u32, u32>,
    pub pending: Vec<(u32, u32, u32, u32)>, // table, old pointer, destination offset, bytes
    pub patches: Vec<(u32, u32)>,
    pub sources: alloc::collections::BTreeSet<u32>,
}

impl Collector {
    pub fn reserve(&mut self, bytes: u32) -> u32 {
        let at = u32::try_from(self.image.len()).unwrap();
        self.image
            .resize((self.image.len() + bytes as usize + 7) & !7, 0);
        at
    }
    pub unsafe fn copy_bytes(&mut self, pointer: u32, bytes: u32) -> u32 {
        let at = self.reserve(bytes);
        unsafe {
            core::ptr::copy_nonoverlapping(
                pointer as *const u8,
                self.image.as_mut_ptr().add(at as usize),
                bytes as usize,
            );
        }
        at
    }
    pub fn put(&mut self, at: u32, value: u32) {
        self.image[at as usize..at as usize + 4].copy_from_slice(&value.to_le_bytes());
    }
    pub unsafe fn value(&mut self, pointer: u32) -> u32 {
        unsafe {
            if pointer == 0 || pointer < self.base {
                return pointer;
            }
            if let Some(&at) = self.values.get(&pointer) {
                return self.base + at;
            }
            let ty = word(pointer, TYPE);
            self.trace_location(word(pointer, SOURCE));
            let bytes = word(self.types + ty * 20, 4);
            assert!(bytes >= HEADER_BYTES);
            let at = self.copy_bytes(pointer, bytes);
            self.values.insert(pointer, at);
            self.pending.push((VALUES, pointer, at, bytes));
            self.base + at
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
            let bytes = slot.bytes & !ENV_RAW_PARENT;
            let at = if table == REGEXES {
                let pattern = crate::regex::pattern(slot.payload);
                self.copy_bytes(pattern.as_ptr() as u32, pattern.len() as u32)
            } else {
                self.copy_bytes(slot.payload, bytes)
            };
            self.slots[table as usize][next as usize] = Slot {
                payload: self.base + at,
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
    pub unsafe fn environment_pointer(&mut self, pointer: u32) -> u32 {
        unsafe {
            let id = *self
                .environments
                .get(&pointer)
                .expect("registered parent environment");
            let next = self.object(ENVIRONMENTS, id);
            self.slots[ENVIRONMENTS as usize][next as usize].payload
        }
    }
    pub unsafe fn finish(mut self) -> u32 {
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
                tables[i].buffer = self.base + at;
                tables[i].length = count;
                tables[i].capacity = count;
            }
            let end = self.base.checked_add(self.image.len() as u32).unwrap();
            for &(pointer, value) in &self.patches {
                (pointer as *mut u32).write_unaligned(value);
            }
            core::ptr::copy(self.image.as_ptr(), self.base as *mut u8, self.image.len());
            for i in 0..TABLE_COUNT {
                (table_address(i) as *mut Table).write(tables[i as usize]);
            }
            let base = self.base;
            // Scratch maps/vectors were allocated after old work; their storage is
            // discarded with the arena. Never run destructors after reusing it.
            core::mem::forget(self);
            crate::NEXT = end as u64;
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
            base
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_collect(types: u32, roots: u32, count: u32) -> u32 {
    unsafe {
        assert!(MAIN_END != 0);
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
            base: MAIN_END,
            types,
            image: vec![],
            old,
            slots,
            objects: BTreeMap::new(),
            values: BTreeMap::new(),
            environments: BTreeMap::new(),
            pending: vec![],
            patches: vec![],
            sources: alloc::collections::BTreeSet::new(),
        };
        gc.reserve(count * 4);
        let env = old[ENVIRONMENTS as usize];
        for id in 0..env.length {
            let slot = (env.buffer as *const Slot).add(id as usize).read();
            gc.environments.insert(slot.payload, id);
        }
        for index in 0..count {
            let pointer = gc.value(word(roots, index as u64 * 4));
            gc.put(index * 4, pointer);
        }
        // Interpreter memo cells in main environments may refer to work values.
        // Their immutable captures stay put; only these exact pointer cells patch.
        for id in 0..env.frozen {
            let slot = (env.buffer as *const Slot).add(id as usize).read();
            for index in 0..(slot.bytes & !ENV_RAW_PARENT) / 4 {
                let cell = slot.payload + index * 4;
                let old = word(cell, 0);
                let next = if slot.bytes & ENV_RAW_PARENT != 0 {
                    gc.environment_pointer(old)
                } else {
                    gc.value(old)
                };
                if next != old {
                    gc.patches.push((cell, next));
                }
            }
        }
        gc.finish()
    }
}
