//! Wasm32 value ABI. All addresses are linear-memory offsets, never host pointers.
pub const VERSION: u32 = 1;
pub const HEADER_BYTES: u32 = 16;
pub const SCALAR_BYTES: u32 = 24;
pub const FUNCTION_BYTES: u32 = 24;
pub const SOURCE: u64 = 0;
pub const START: u64 = 4;
pub const END: u64 = 8;
pub const TYPE: u64 = 12;
pub const DATA: u64 = 16;
pub const ENVIRONMENT: u64 = 20;

/// Zero is reserved for a failed computation, never for a Telora value.
pub const NULL: u32 = 0;
pub const TABLE_BASE: u32 = 64;
pub const TABLE_BYTES: u32 = 16;
pub const TABLE_COUNT: u32 = 7;
pub const STATIC_BASE: u32 = TABLE_BASE + TABLE_BYTES * TABLE_COUNT;
pub const STRINGS: u32 = 0;
pub const BYTES: u32 = 1;
pub const RECORDS: u32 = 2;
pub const ARRAYS: u32 = 3;
pub const VALUES: u32 = 4;
pub const ENVIRONMENTS: u32 = 5;
pub const NEWTYPES: u32 = 6;
pub fn table_address(table: u32) -> u32 {
    TABLE_BASE + table * TABLE_BYTES
}
pub const DEMAND_BYTES: u32 = 8;

pub const ALLOC: u32 = 0;
pub const INVOKE: u32 = 1;
pub const TABLE_PUSH: u32 = 2;
pub const TABLE_GET: u32 = 3;
pub const FREEZE: u32 = 4;
pub const STRING_COMPARE: u32 = 5;
pub const FIRST_FUNCTION: u32 = 6;
pub const CALL_TYPE: u32 = 1;
pub const BUMP_GLOBAL: u32 = 0;
pub const ERROR_GLOBAL: u32 = 1;
pub const PHASE_GLOBAL: u32 = 2;

pub const ERROR_OVERFLOW: u32 = 1;
pub const ERROR_DIVISION: u32 = 2;
pub const ERROR_CYCLE: u32 = 3;
pub const ERROR_INDEX: u32 = 4;
pub const ERROR_KEY: u32 = 5;
pub const ERROR_PROPERTY: u32 = 6;
pub const ERROR_MATCH: u32 = 7;

pub fn memory(offset: u64, align: u32) -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset,
        align,
        memory_index: 0,
    }
}
