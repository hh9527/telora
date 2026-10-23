//! Copy/relocate adapter for the shared exact layout scanner.
use crate::collect::Collector;
use telora_wasm_shared::trace::{self, Edge, Error, Visitor};

impl Visitor for Collector {
    fn types(&self) -> u32 {
        self.types
    }
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
                Edge::Content { old, at } => {
                    let bytes: [u8; 16] = self.heap.read(old);
                    if let telora_wasm_shared::arena::content::Bytes::Slice(s) =
                        telora_wasm_shared::arena::content::Bytes::decode(bytes).unwrap()
                    {
                        let raw = if crate::heap::is_static(s.raw) {
                            s.raw
                        } else if let Some(&(next, _)) =
                            self.objects.get(&(crate::abi::CONTENT_VIEW, s.raw))
                        {
                            next
                        } else {
                            let len = self.old_word(s.raw, 0);
                            self.heap.span(s.raw, len.checked_add(8).unwrap() as usize);
                            let next = crate::content::allocate_capacity(len);
                            core::ptr::copy_nonoverlapping(
                                self.heap.ptr::<u8>(s.raw),
                                crate::heap::ptr::<u8>(next),
                                len.checked_add(8).unwrap() as usize,
                            );
                            self.put(next + 4, len);
                            self.objects
                                .insert((crate::abi::CONTENT_VIEW, s.raw), (next, 0));
                            next
                        };
                        self.put(at, raw);
                    }
                }
                Edge::Object { table, id, ty, at } => {
                    let next = self.object(table, id, ty);
                    self.put(at, next);
                }
                Edge::Value { reference, ty, at } => {
                    let next = self.value(reference, ty);
                    self.put(at, next);
                }
                Edge::Storage {
                    reference,
                    bytes,
                    at,
                } => {
                    let next = if bytes == 0 {
                        0
                    } else {
                        self.copy_bytes(reference, bytes)
                    };
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
        if id != 0 {
            self.sources.insert(id);
        }
    }
    pub unsafe fn trace_object(&mut self, table: u32, old: u32, at: u32, bytes: u32, ty: u32) {
        trace::object(self, table, old, at, bytes, ty).expect("invalid closed trace layout");
    }
}
