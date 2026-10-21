//! Copy/relocate adapter for the shared exact layout scanner.
use crate::collect::Collector;
use telora_wasm_shared::trace::{self, Edge, Error, Visitor};

impl Visitor for Collector {
    fn types(&self) -> u32 { self.types }
    fn read(&self, reference: u32) -> Result<u32, Error> {
        unsafe { Ok(self.old_word(reference, 0)) }
    }
    fn edge(&mut self, edge: Edge) -> Result<(), Error> {
        unsafe {
            match edge {
                Edge::Inline { old, at, ty } => {
                    self.pending.push((crate::abi::VALUES, old, at, 0, ty));
                }
                Edge::Source(reference) => self.trace_location(self.old_location(reference)),
                Edge::Content { at, .. } => self.content.push(at),
                Edge::Object { table, id, ty, at } => {
                    let next = self.object(table, id, ty);
                    self.put(at, next);
                }
                Edge::Value { reference, ty, at } => {
                    let next = self.value(reference, ty);
                    self.put(at, next);
                }
                Edge::Storage { reference, bytes, at } => {
                    let next = if bytes == 0 { 0 } else { self.copy_bytes(reference, bytes) };
                    self.put(at, next);
                }
                Edge::Function(function) => self.trace_function_demands(function),
            }
        }
        Ok(())
    }
}

impl Collector {
    pub unsafe fn trace_location(&mut self, id: u32) {
        if id != 0 { self.sources.insert(id); }
    }
    pub unsafe fn trace_object(&mut self, table: u32, old: u32, at: u32, bytes: u32, ty: u32) {
        trace::object(self, table, old, at, bytes, ty).expect("invalid closed trace layout");
    }
}
