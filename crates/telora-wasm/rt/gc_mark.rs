//! Nonmoving reachability discovery over published roots and closed layouts.
//! Discovery borrows no physical address across a visitor callback.
use crate::{
    abi::*,
    tables::{Slot, Table},
    values::word,
};
use alloc::{collections::BTreeSet, vec::Vec};
use telora_wasm_shared::{
    gc_roots::{Kind, Root},
    source_range::SourceRange,
    trace::{self, Edge, Error, Visitor},
};

pub(crate) struct Mark {
    pub storage: BTreeSet<u32>,
    pub objects: BTreeSet<(u32, u32, u32)>,
    pub content: BTreeSet<u32>,
    pub sources: BTreeSet<u32>,
    values: BTreeSet<(u32, u32)>,
    pending: Vec<(u32, u32, u32, u32)>,
    types: u32,
}

impl Mark {
    pub unsafe fn trace(roots: Vec<Root>) -> Self {
        unsafe {
            let mut mark = Self {
                storage: BTreeSet::new(),
                objects: BTreeSet::new(),
                content: BTreeSet::new(),
                sources: BTreeSet::new(),
                values: BTreeSet::new(),
                pending: Vec::new(),
                types: *core::ptr::addr_of!(crate::collect::TRACE_TYPES),
            };
            for root in roots {
                mark.root(root);
            }
            let (demands, count) = *core::ptr::addr_of!(crate::collect::DEMANDS);
            for index in 0..count {
                let slot = demands + index * DEMAND_BYTES;
                if word(slot, 0) == 2 {
                    mark.value(word(slot, 4), word(slot, 8));
                } else if word(slot, 0) == 3 {
                    let packet = word(slot, 4);
                    if packet != 0 {
                        mark.span(packet);
                        mark.pending
                            .push((DIAGNOSTICS, packet, DIAGNOSTIC_BYTES, u32::MAX));
                    }
                }
            }
            for (reference, ty) in crate::service::execution_roots() {
                mark.value(reference, ty);
            }
            mark.sources.extend(crate::service_sources::source_roots());
            for table in 0..TABLE_COUNT {
                let descriptor: Table = crate::heap::read(table_address(table));
                mark.span(descriptor.buffer);
                if matches!(table, DIAGNOSTICS | DEBUG_EVENTS) {
                    for id in 0..descriptor.length {
                        mark.object(table, id, u32::MAX);
                    }
                }
            }
            while let Some((tag, reference, bytes, ty)) = mark.pending.pop() {
                if tag == ROOT_VALUE {
                    trace::value(&mut mark, ty, reference, reference)
                } else {
                    trace::object(&mut mark, tag, reference, reference, bytes, ty)
                }
                .expect("invalid published root layout");
            }
            mark
        }
    }

    fn span(&mut self, reference: u32) {
        if reference != 0 {
            self.storage.insert(reference);
        }
    }

    unsafe fn root(&mut self, root: Root) {
        unsafe {
            self.span(root.reference);
            match root.kind {
                Kind::Storage => {}
                Kind::ParsePlan => self.plan(root.reference),
                Kind::Context { string_type, codec } => {
                    self.value(
                        word(root.reference, if codec { 24 } else { 4 }),
                        string_type,
                    );
                    let error = word(root.reference, if codec { 28 } else { 8 });
                    self.span(error);
                    if error != 0 {
                        self.value(word(error, 0), string_type);
                    }
                    if codec {
                        self.rejection(error, string_type);
                    } else {
                        self.source(word(root.reference, 16));
                        self.rejection(word(root.reference, 20), string_type);
                    }
                }
                Kind::Value { type_id } => self.value(root.reference, type_id),
                Kind::Source => {
                    self.edge(Edge::Source(root.reference)).unwrap();
                }
                Kind::Object {
                    type_id,
                    tag,
                    initialized,
                } => {
                    let bytes = if tag == ENVIRONMENTS {
                        8 + word(root.reference, 0).checked_mul(8).unwrap()
                    } else {
                        initialized
                    };
                    self.pending.push((tag, root.reference, bytes, type_id));
                }
                Kind::References {
                    type_id,
                    initialized,
                } => {
                    for index in 0..initialized {
                        self.value(word(root.reference, u64::from(index) * 4), type_id);
                    }
                }
            }
        }
    }

    unsafe fn source(&mut self, reference: u32) {
        if reference != 0 {
            self.span(reference);
            self.edge(Edge::Source(reference)).expect("context source");
        }
    }

    unsafe fn plan(&mut self, packet: u32) {
        unsafe {
            let rows = word(packet, 0);
            let count = word(packet, 4);
            self.span(rows);
            let error = word(packet, 12);
            self.span(error);
            if error != 0 {
                for offset in [0, 8] {
                    if word(error, offset + 4) != 0 {
                        self.span(word(error, offset));
                    }
                }
            }
            for index in 0..count {
                let row = rows + index.checked_mul(24).unwrap();
                self.source(row + 4);
                let kind = word(row, 0);
                let payload = word(row, 16);
                let length = word(row, 20);
                if length != 0 && matches!(kind, 5..=12) {
                    self.span(payload);
                }
                if kind == 7 {
                    for index in 0..length {
                        let entry = payload + index.checked_mul(24).unwrap();
                        if word(entry, 4) != 0 {
                            self.span(word(entry, 0));
                        }
                        self.source(entry + 12);
                    }
                }
            }
        }
    }

    unsafe fn rejection(&mut self, reference: u32, string_type: u32) {
        unsafe {
            if reference == 0 {
                return;
            }
            self.span(reference);
            self.value(word(reference, 0), string_type);
            self.source(word(reference, 4));
            let blame = word(reference, 8);
            if blame != 0 {
                self.source(blame);
                self.object(BLAMES, word(blame, DATA), u32::MAX);
            }
        }
    }

    fn value(&mut self, reference: u32, ty: u32) {
        if reference != 0 && self.values.insert((reference, ty)) {
            self.span(reference);
            self.pending.push((ROOT_VALUE, reference, 0, ty));
        }
    }

    unsafe fn object(&mut self, table: u32, id: u32, ty: u32) {
        unsafe {
            let direct = matches!(table, ARRAYS | RECORDS | VALUES | NEWTYPES | ENVIRONMENTS);
            if (direct && id == 0) || !self.objects.insert((table, id, ty)) {
                return;
            }
            if direct {
                self.span(id);
                let stored = word(id, 0);
                let (payload, bytes, closed) = match table {
                    ENVIRONMENTS => (id, 8 + stored.checked_mul(8).unwrap(), ty),
                    ARRAYS => {
                        let width = word(
                            self.types + stored * telora_wasm_shared::layout_image::ENTRY_BYTES,
                            telora_wasm_shared::layout_image::VALUE_BYTES as u64,
                        );
                        let length = word(id, 8);
                        assert!(length <= word(id, 12));
                        (word(id, 4), length.checked_mul(width).unwrap(), stored)
                    }
                    _ => (id + 8, word(id, 4), stored),
                };
                self.span(payload);
                self.pending.push((table, payload, bytes, closed));
            } else {
                let descriptor: Table = crate::heap::read(table_address(table));
                assert!(id < descriptor.length, "invalid resource slot");
                let slot: Slot = crate::heap::read(descriptor.buffer + id * 8);
                if table != REGEXES {
                    self.span(slot.payload);
                    self.pending.push((table, slot.payload, slot.bytes, ty));
                }
            }
        }
    }
}

impl Visitor for Mark {
    fn types(&self) -> u32 {
        self.types
    }
    fn read(&self, reference: u32) -> Result<u32, Error> {
        unsafe { Ok(crate::heap::read(reference)) }
    }
    fn edge(&mut self, edge: Edge) -> Result<(), Error> {
        unsafe {
            match edge {
                Edge::Source(reference) => {
                    let range =
                        SourceRange::unpack(crate::heap::read(reference)).ok_or(Error::Layout)?;
                    if range.source != 0 {
                        self.sources.insert(range.source);
                    }
                }
                Edge::Content { old, .. } => {
                    self.content.insert(old);
                }
                Edge::Inline { old, ty, .. }
                | Edge::Value {
                    reference: old, ty, ..
                } => self.value(old, ty),
                Edge::Object { table, id, ty, .. } => self.object(table, id, ty),
                Edge::Storage {
                    reference, bytes, ..
                } => {
                    if bytes != 0 {
                        self.span(reference);
                    }
                }
                Edge::Function(_) => {} // All Ready demands already are semantic roots.
            }
        }
        Ok(())
    }
}
