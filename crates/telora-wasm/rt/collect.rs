//! Event-boundary copying collector. All traversal follows closed physical types.
use crate::{
    abi::*,
    tables::{Slot, Table},
    values::word,
};
use alloc::{collections::BTreeMap, vec, vec::Vec};
use telora_wasm_shared::layout_image as layout;
mod initialization;
pub(crate) use initialization::collect as collect_initialization;

static mut TRACE_TYPES: u32 = 0;
static mut DEMANDS: (u32, u32) = (0, 0);

pub(crate) unsafe fn snapshot_demands() -> Vec<u8> {
    unsafe {
        let (pointer, count) = DEMANDS;
        core::slice::from_raw_parts(pointer as *const u8, count as usize * DEMAND_BYTES as usize)
            .to_vec()
    }
}

pub(crate) unsafe fn restore_demands(bytes: &[u8]) {
    unsafe {
        let (pointer, count) = DEMANDS;
        assert_eq!(bytes.len(), count as usize * DEMAND_BYTES as usize);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len());
    }
}

pub(crate) unsafe fn snapshot_stats() -> [u32; 5] {
    unsafe { initialization::snapshot_stats() }
}

pub(crate) unsafe fn restore_stats(stats: [u32; 5]) {
    unsafe { initialization::restore_stats(stats); }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_collection_bootstrap(types: u32, demands: u32, count: u32) {
    unsafe {
        assert_eq!(*core::ptr::addr_of!(TRACE_TYPES), 0);
        TRACE_TYPES = types;
        DEMANDS = (demands, count);
    }
}

pub(crate) unsafe fn freeze() {
    unsafe {
        crate::heap::freeze();
        crate::content::freeze();
        crate::sources::freeze();
    }
}

pub(crate) struct Collector {
    pub initialization: bool,
    pub heap: crate::heap::OldWords,
    pub types: u32,
    pub old: [Table; TABLE_COUNT as usize],
    pub slots: Vec<Vec<Slot>>,
    pub objects: BTreeMap<(u32, u32), u32>,
    pub values: BTreeMap<u32, u32>,
    pub pending: Vec<(u32, u32, u32, u32)>, // table, old pointer, destination offset, bytes
    pub sources: alloc::collections::BTreeSet<u32>,
    pub content: Vec<u32>,
}

impl Collector {
    pub unsafe fn old_word(&self, reference: u32, offset: u64) -> u32 {
        unsafe { self.heap.read(reference + offset as u32) }
    }
    pub unsafe fn old_location(&self, reference: u32) -> u32 {
        unsafe {
            telora_wasm_shared::source_range::SourceRange::unpack(self.heap.read(reference))
                .expect("invalid packed source range")
                .source
        }
    }
    pub fn reserve(&mut self, bytes: u32) -> u32 {
        unsafe { crate::telora_alloc(bytes) }
    }
    pub unsafe fn copy_bytes(&mut self, pointer: u32, bytes: u32) -> u32 {
        let at = self.reserve(bytes);
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.heap.ptr::<u8>(pointer),
                crate::heap::ptr::<u8>(at),
                bytes as usize,
            );
        }
        at
    }
    pub fn put(&mut self, at: u32, value: u32) {
        unsafe { crate::heap::write(at, value); }
    }
    pub unsafe fn value(&mut self, pointer: u32) -> u32 {
        unsafe {
            if pointer == 0 || crate::heap::is_frozen(pointer) {
                return pointer;
            }
            if let Some(&at) = self.values.get(&pointer) {
                return at;
            }
            let ty = self.old_word(pointer, TYPE);
            self.trace_location(self.old_location(pointer));
            let bytes = word(
                self.types + ty * layout::ENTRY_BYTES,
                layout::VALUE_BYTES as u64,
            );
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
            let slot: Slot = self.heap.read(old.buffer + id * 8);
            let next = self.slots[table as usize].len() as u32;
            self.objects.insert((table, id), next);
            // Publish forwarding before traversal, including cyclic environments.
            self.slots[table as usize].push(Slot {
                payload: 0,
                bytes: slot.bytes,
            });
            let at = if table == REGEXES {
                // Owned Rust resource: move its table ownership, not its bytes.
                slot.payload
            } else {
                self.copy_bytes(slot.payload, slot.bytes)
            };
            self.slots[table as usize][next as usize] = Slot {
                payload: at,
                bytes: slot.bytes,
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
            crate::content::collect(&self.content, self.initialization);
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
                if !self.objects.contains_key(&(REGEXES, id)) {
                    let slot: Slot = self.heap.read(old_regex.buffer + id * 8);
                    crate::regex::release(slot.payload);
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_collect(roots: u32, count: u32) -> u32 {
    unsafe {
        let mut gc = Collector::begin(false);
        let result = gc.reserve(count * 4);
        for index in 0..count {
            let pointer = gc.value(gc.old_word(roots, index as u64 * 4));
            gc.put(result + index * 4, pointer);
        }
        gc.finish();
        result
    }
}

impl Collector {
    unsafe fn begin(initialization: bool) -> Self {
      unsafe {
        // Keep the old arena alive until all traversal and patching finishes.
        let old_work = if initialization { crate::heap::take_initialization() }
            else { crate::heap::take_work() };
        let old = core::array::from_fn(|i| (table_address(i as u32) as *const Table).read());
        let slots = old
            .iter()
            .map(|table| {
                if table.frozen == 0 {
                    vec![]
                } else {
                    core::slice::from_raw_parts(old_work.ptr::<Slot>(table.buffer), table.frozen as usize)
                        .to_vec()
                }
            })
            .collect();
        Collector {
            initialization,
            heap: old_work,
            types: TRACE_TYPES,
            old,
            slots,
            objects: BTreeMap::new(),
            values: BTreeMap::new(),
            pending: vec![],
            sources: alloc::collections::BTreeSet::new(),
            content: vec![],
        }
      }
    }
}
