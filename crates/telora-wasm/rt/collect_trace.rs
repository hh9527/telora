//! Fixed trace opcodes supplied by codegen's closed type descriptions.
use crate::{abi::*, collect::Collector, values::word};
use telora_wasm_shared::layout_image as layout;

impl Collector {
    pub unsafe fn trace_location(&mut self, id: u32) {
        if id != 0 {
            self.sources.insert(id);
        }
    }

    unsafe fn handle(&mut self, table: u32, old: u32, at: u32, layout: u32) {
        unsafe {
            let id = self.object(table, self.old_word(old, 0), layout);
            self.put(at, id);
        }
    }

    unsafe fn detail_type(&self, ty: u32, index: u32) -> u32 {
        unsafe {
            let desc = self.types + ty * layout::ENTRY_BYTES;
            assert!(index < word(desc, layout::DETAIL_COUNT as u64));
            let detail = self.types + word(desc, layout::DETAIL_OFFSET as u64)
                + index * layout::DETAIL_BYTES;
            let result = word(detail, layout::DETAIL_TYPE as u64);
            assert_ne!(result, layout::NO_TYPE);
            result
        }
    }

    unsafe fn trace_value(&mut self, ty: u32, old: u32, at: u32) {
        unsafe {
            self.trace_location(self.old_location(old));
            self.trace_data(ty, old + DATA as u32, at + DATA as u32);
        }
    }

    pub unsafe fn trace_object(&mut self, table: u32, old: u32, at: u32, bytes: u32, ty: u32) {
        unsafe {
            match table {
                HASHES => {}
                ARRAYS => {
                    let width = word(self.types + ty * layout::ENTRY_BYTES, layout::VALUE_BYTES as u64);
                    assert!(
                        (width == 0 && bytes == 0)
                            || (width >= HEADER_BYTES && bytes % width == 0)
                    );
                    let mut offset = 0;
                    while offset < bytes {
                        self.trace_value(ty, old + offset, at + offset);
                        offset += width;
                    }
                }
                VALUES => self.trace_value(ty, old, at),
                NEWTYPES => self.trace_value(self.detail_type(ty, 0), old, at),
                RECORDS => {
                    let desc = self.types + ty * layout::ENTRY_BYTES;
                    for index in 0..word(desc, layout::DETAIL_COUNT as u64) {
                        let detail = self.types + word(desc, layout::DETAIL_OFFSET as u64)
                            + index * layout::DETAIL_BYTES;
                        let field = word(detail, layout::DETAIL_TYPE as u64);
                        let offset = word(detail, layout::DETAIL_OFFSET_OR_TAG as u64);
                        assert_ne!(field, layout::NO_TYPE);
                        self.trace_value(field, old + offset, at + offset);
                    }
                }
                ENVIRONMENTS => {
                    let count = self.old_word(old, 0);
                    assert_eq!(bytes, 8 + count * 8);
                    for index in 0..count {
                        let pointer = self.old_word(old, 8 + index as u64 * 4);
                        let ty = self.old_word(old, 8 + u64::from((count + index) * 4));
                        let next = self.value(pointer, ty);
                        self.put(at + 8 + index * 4, next);
                    }
                }
                BLAMES => {
                    self.trace_location(self.old_location(old));
                    for index in 0..self.old_word(old, BLAME_COUNT as u64) {
                        self.trace_location(self.old_word(old, BLAME_SUBJECTS as u64 + index as u64 * LOC_BYTES as u64));
                    }
                    self.string(old + DATA as u32, at + DATA as u32);
                }
                FORMATS => {
                    let count = if self.old_word(old, 0) == 4 { 2 } else { 1 };
                    for index in 0..count {
                        let next = self.value(
                            self.old_word(old, 4 + index * 4),
                            self.old_word(old, 12 + index * 4),
                        );
                        self.put(at + 4 + index as u32 * 4, next);
                    }
                }
                TESTS => {
                    let count = self.old_word(old, 4);
                    for index in 0..count {
                        let next = self.value(
                            self.old_word(old, 8 + index as u64 * 4),
                            self.old_word(old, 8 + u64::from(count) * 4 + index as u64 * 4),
                        );
                        self.put(at + 8 + index * 4, next);
                    }
                }
                DEBUG_EVENTS => {
                    let next = self.value(self.old_word(old, 4), self.old_word(old, 8));
                    self.put(at + 4, next);
                }
                DIAGNOSTICS => {
                        self.trace_location(self.old_location(old));
                    let message = self.value(
                        self.old_word(old, DIAG_MESSAGE),
                        self.old_word(old, DIAG_MESSAGE_TYPE),
                    );
                    self.put(at + DIAG_MESSAGE as u32, message);
                    let count = self.old_word(old, DIAG_COUNT);
                    let subjects = self.old_word(old, DIAG_SUBJECTS);
                    if count != 0 {
                        let next = self.copy_bytes(subjects, count.checked_mul(LOC_BYTES).unwrap());
                        self.put(at + DIAG_SUBJECTS as u32, next);
                        for index in 0..count {
                            self.trace_location(self.old_location(subjects + index * LOC_BYTES));
                        }
                    } else {
                        self.put(at + DIAG_SUBJECTS as u32, 0);
                    }
                }
                _ => core::arch::wasm32::unreachable(),
            }
        }
    }

    unsafe fn string(&mut self, _old: u32, at: u32) {
        self.content.push(at);
    }

    unsafe fn trace_data(&mut self, ty: u32, old: u32, at: u32) {
        unsafe {
            let desc = self.types + ty * layout::ENTRY_BYTES;
            match word(desc, layout::KIND as u64) {
                0 => {}
                1 | 2 => self.string(old, at),
                3 => self.handle(RECORDS, old, at, ty),
                4 => self.handle(ARRAYS, old, at, self.detail_type(ty, 0)),
                5 => {
                    // Dictionary keys are String; the layout image records it
                    // before the value element type.
                    self.handle(ARRAYS, old, at, self.detail_type(ty, 0));
                    self.handle(ARRAYS, old + 8, at + 8, self.detail_type(ty, 1));
                }
                6 => {
                    let tag = self.old_word(old, 0);
                    assert!(tag < word(desc, layout::DETAIL_COUNT as u64));
                    let variant = self.types + word(desc, layout::DETAIL_OFFSET as u64)
                        + tag * layout::DETAIL_BYTES;
                    let payload = word(variant, layout::DETAIL_TYPE as u64);
                    if payload != u32::MAX {
                        if word(variant, layout::DETAIL_FLAGS as u64) & layout::DETAIL_BOXED != 0 {
                            self.handle(VALUES, old + 8, at + 8, payload);
                        } else {
                            self.trace_location(self.old_word(old, 8));
                            self.trace_data(payload, old + 8 + HEADER_BYTES, at + 8 + HEADER_BYTES);
                        }
                    }
                }
                7 => {
                    let payload = self.old_word(old, 0);
                    if self.old_word(old, 4) == 1 {
                        self.handle(VALUES, old + 8, at + 8, payload);
                    } else {
                        self.trace_data(payload, old + 8, at + 8);
                    }
                }
                8 => {
                    self.trace_function_demands(self.old_word(old, 0));
                    let environment = self.old_word(old, 4);
                    if environment != 0 {
                        let next = self.object(ENVIRONMENTS, environment, ty);
                        self.put(at + 4, next);
                    }
                }
                9 => self.handle(word(desc, layout::RESOURCE_TABLE as u64), old, at, ty),
                10 => self.handle(NEWTYPES, old, at, self.detail_type(ty, 0)),
                _ => core::arch::wasm32::unreachable(),
            }
        }
    }
}
