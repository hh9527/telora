//! Wasm32 value ABI. All addresses are linear-memory offsets, never host pointers.
pub use crate::runtime_abi::*;

pub const ALLOC: u32 = 0;
pub const INVOKE: u32 = 1;
pub const TABLE_PUSH: u32 = 2;
pub const TABLE_GET: u32 = 3;
pub const FREEZE: u32 = 4;
pub const STRING_COMPARE: u32 = 5;
pub const SOURCE_NAME: u32 = 6;
pub const FIRST_FUNCTION: u32 = 7;
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
