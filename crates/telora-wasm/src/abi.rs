//! Wasm32 value ABI. All addresses are linear-memory offsets, never host pointers.
pub use crate::runtime_abi::*;

pub const ALLOC: u32 = 0;
pub const INVOKE: u32 = 1;
pub const TABLE_PUSH: u32 = 2;
pub const TABLE_GET: u32 = 3;
pub const FREEZE: u32 = 4;
pub const STRING_COMPARE: u32 = 5;
pub const SOURCE_NAME: u32 = 6;
pub const SUBJECT_LABEL: u32 = 7;
pub const SORT_PAIRS: u32 = 8;
pub const DUPLICATE_KEY_MESSAGE: u32 = 9;
pub const TEXT_QUERY: u32 = 10;
pub const TEXT_BUILD: u32 = 11;
pub const TEXT_SPLIT: u32 = 12;
pub const PATH: u32 = 13;
pub const FORMAT_RENDER: u32 = 14;
pub const FORMAT_MESSAGE: u32 = 15;
pub const FORMAT_JOIN: u32 = 16;
pub const TEMPLATE_PREPARE: u32 = 17;
pub const MEMBER_MESSAGE: u32 = 18;
pub const REGEX: u32 = 19;
pub const HASH: u32 = 20;
pub const FIRST_FUNCTION: u32 = 21;
pub const CALL_TYPE: u32 = 1;
pub const ERROR_GLOBAL: u32 = 0;
pub const PHASE_GLOBAL: u32 = 1;

pub const ERROR_OVERFLOW: u32 = 1;
pub const ERROR_DIVISION: u32 = 2;
pub const ERROR_CYCLE: u32 = 3;
pub const ERROR_INDEX: u32 = 4;
pub const ERROR_KEY: u32 = 5;
pub const ERROR_PROPERTY: u32 = 6;
pub const ERROR_MATCH: u32 = 7;
pub const ERROR_DATA: u32 = 8;
pub const ERROR_USER: u32 = 9;

pub fn memory(offset: u64, align: u32) -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset,
        align,
        memory_index: 0,
    }
}
