//! Fixed trace opcodes supplied by codegen's closed type descriptions.
use crate::{abi::*, collect::Collector, values::word};

impl Collector {
    unsafe fn handle(&mut self, table: u32, old: u32, at: u32) {
        unsafe {
            let id = self.object(table, word(old, 0));
            self.put(at, id);
        }
    }

    pub unsafe fn trace_object(&mut self, table: u32, old: u32, at: u32, bytes: u32) {
        unsafe {
            match table {
                STRINGS | BYTES | HASHES => {}
                RECORDS | ARRAYS | VALUES | NEWTYPES => {
                    let mut offset = 0;
                    while offset < bytes {
                        let ty = word(old + offset, TYPE);
                        self.sources.insert(word(old + offset, SOURCE) & 0xffff);
                        let width = word(self.types + ty * 20, 4);
                        assert!(width >= HEADER_BYTES && width <= bytes - offset);
                        self.trace_data(ty, old + offset + 16, at + offset + 16);
                        offset += width;
                    }
                }
                ENVIRONMENTS => {
                    for index in 0..(bytes & !ENV_RAW_PARENT) / 4 {
                        let pointer = word(old, index as u64 * 4);
                        let next = if bytes & ENV_RAW_PARENT != 0 {
                            self.environment_pointer(pointer)
                        } else {
                            self.value(pointer)
                        };
                        self.put(at + index * 4, next);
                    }
                }
                BLAMES => {
                    self.sources.insert(word(old, SOURCE) & 0xffff);
                    for index in 0..word(old, 32) {
                        self.sources.insert(word(old, 40 + index as u64 * 12) & 0xffff);
                    }
                    self.string(old + 16, at + 16);
                }
                FORMATS => {
                    let count = if word(old, 0) == 4 { 2 } else { 1 };
                    for index in 0..count {
                        let next = self.value(word(old, 4 + index * 4));
                        self.put(at + 4 + index as u32 * 4, next);
                    }
                }
                TESTS => {
                    for index in 0..word(old, 4) {
                        let next = self.value(word(old, 8 + index as u64 * 4));
                        self.put(at + 8 + index * 4, next);
                    }
                }
                _ => core::arch::wasm32::unreachable(),
            }
        }
    }

    unsafe fn string(&mut self, old: u32, at: u32) {
        unsafe {
            if *(old as *const u8) == 1 {
                self.handle(STRINGS, old + 4, at + 4);
            }
        }
    }

    unsafe fn trace_data(&mut self, ty: u32, old: u32, at: u32) {
        unsafe {
            let desc = self.types + ty * 20;
            match word(desc, 0) {
                0 => {}
                1 => self.string(old, at),
                2 => self.handle(BYTES, old, at),
                3 => self.handle(RECORDS, old, at),
                4 => self.handle(ARRAYS, old, at),
                5 => {
                    self.handle(ARRAYS, old, at);
                    self.handle(ARRAYS, old + 8, at + 8);
                }
                6 => {
                    let tag = word(old, 0);
                    assert!(tag < word(desc, 12));
                    let variant = self.types + word(desc, 16) + tag * 8;
                    let payload = word(variant, 0);
                    if payload != u32::MAX {
                        if word(variant, 4) != 0 {
                            self.handle(VALUES, old + 8, at + 8);
                        } else {
                            self.sources.insert(word(old, 8) & 0xffff);
                            self.trace_data(payload, old + 24, at + 24);
                        }
                    }
                }
                7 => {
                    let payload = word(old, 0);
                    if word(old, 4) == 1 {
                        self.handle(VALUES, old + 8, at + 8);
                    } else {
                        self.trace_data(payload, old + 8, at + 8);
                    }
                }
                8 => {
                    let id = word(old, 4);
                    if id != 0 {
                        let next = self.object(ENVIRONMENTS, id - 1);
                        self.put(at + 4, next + 1);
                    }
                }
                9 => self.handle(word(desc, 8), old, at),
                10 => self.handle(NEWTYPES, old, at),
                _ => core::arch::wasm32::unreachable(),
            }
        }
    }
}
