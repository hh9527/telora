//! Shared physical layout; consumed by both codegen and Rust-compiled RT.
pub const VERSION: u32 = 24;
pub const HEADER_BYTES: u32 = 16;
pub const SCALAR_BYTES: u32 = 24;
pub const FUNCTION_BYTES: u32 = 24;
pub const STRING_BYTES: u32 = HEADER_BYTES + 16;
pub const DYN_BYTES: u32 = HEADER_BYTES + 24;
// A value head contains src/start/end byte offsets and one sealed TypeId.
pub const SOURCE: u64 = 0;
pub const LOC_BYTES: u32 = 12;
pub const START: u64 = 4;
pub const END: u64 = 8;
pub const TYPE: u64 = 12;
pub const DATA: u64 = 16;
pub const ENVIRONMENT: u64 = 20;
pub const NULL: u32 = 0;
pub const TABLE_BASE: u32 = 64;
pub const TABLE_BYTES: u32 = 16;
pub const TABLE_COUNT: u32 = 12;
pub const STATIC_BASE: u32 = TABLE_BASE + TABLE_BYTES * TABLE_COUNT;
pub const RECORDS: u32 = 0;
pub const ARRAYS: u32 = 1;
pub const VALUES: u32 = 2;
pub const ENVIRONMENTS: u32 = 3;
pub const NEWTYPES: u32 = 4;
pub const BLAMES: u32 = 5;
pub const DIAGNOSTICS: u32 = 6;
pub const FORMATS: u32 = 7;
pub const REGEXES: u32 = 8;
pub const HASHES: u32 = 9;
pub const TESTS: u32 = 10;
pub const DEBUG_EVENTS: u32 = 11;
// Reserved protocol word below the table descriptors; disabled by default.
pub const DEBUG_ENABLED: u32 = 16;
// Read-only Host view of Guest source records: pointer/count, records of 5 u32.
pub const SOURCE_REGISTRY: u32 = 32;
// Read-only content base/length. Re-read after every Guest call.
pub const CONTENT_VIEW: u32 = 48;
pub const WORDS_VIEW: u32 = 40;
pub const WORDS_ORIGIN: u32 = 56;
pub const DIAGNOSTIC_BYTES: u32 = 40;
pub const DEMAND_BYTES: u32 = 8;

pub fn table_address(table: u32) -> u32 {
    TABLE_BASE + table * TABLE_BYTES
}

pub const DIAG_CODE: u64 = 12;
pub const DIAG_MESSAGE: u64 = 16;
pub const DIAG_SUBJECTS: u64 = 20;
pub const DIAG_COUNT: u64 = 24;
pub const DIAG_WARNING: u64 = 28;
pub const DIAG_ROOT: u64 = 32;
pub const BLAME_COUNT: u32 = STRING_BYTES;
pub const BLAME_SUBJECTS: u32 = STRING_BYTES + 8;
