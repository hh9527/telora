//! Shared physical layout; consumed by both codegen and Rust-compiled RT.
pub const VERSION: u32 = 2;
pub const HEADER_BYTES: u32 = 16;
pub const SCALAR_BYTES: u32 = 24;
pub const FUNCTION_BYTES: u32 = 24;
pub const SOURCE: u64 = 0;
pub const START: u64 = 4;
pub const END: u64 = 8;
pub const TYPE: u64 = 12;
pub const DATA: u64 = 16;
pub const ENVIRONMENT: u64 = 20;
pub const NULL: u32 = 0;
pub const TABLE_BASE: u32 = 64;
pub const TABLE_BYTES: u32 = 16;
pub const TABLE_COUNT: u32 = 9;
pub const STATIC_BASE: u32 = TABLE_BASE + TABLE_BYTES * TABLE_COUNT;
pub const STRINGS: u32 = 0;
pub const BYTES: u32 = 1;
pub const RECORDS: u32 = 2;
pub const ARRAYS: u32 = 3;
pub const VALUES: u32 = 4;
pub const ENVIRONMENTS: u32 = 5;
pub const NEWTYPES: u32 = 6;
pub const BLAMES: u32 = 7;
pub const DIAGNOSTICS: u32 = 8;
pub const DIAGNOSTIC_BYTES: u32 = 32;
pub const DEMAND_BYTES: u32 = 8;

pub fn table_address(table: u32) -> u32 {
    TABLE_BASE + table * TABLE_BYTES
}
