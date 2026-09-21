//! Shared physical layout; consumed by both codegen and Rust-compiled RT.
pub const VERSION: u32 = 28;
pub const HEADER_BYTES: u32 = 8;
pub const SCALAR_BYTES: u32 = 16;
pub const FUNCTION_BYTES: u32 = 16;
pub const STRING_BYTES: u32 = HEADER_BYTES + 16;
pub const DYN_BYTES: u32 = HEADER_BYTES + 24;
// A value head contains only its packed source range. Its closed type comes
// from the signature, enclosing layout, or typed Host handle.
pub const SOURCE: u64 = 0;
pub const LOC_BYTES: u32 = 8;
pub const DATA: u64 = 8;
pub const ENVIRONMENT: u64 = 12;
pub const NULL: u32 = 0;
pub const TABLE_BASE: u32 = 64;
pub const TABLE_BYTES: u32 = 16;
pub const TABLE_COUNT: u32 = 7;
pub const STATIC_BASE: u32 = TABLE_BASE + TABLE_BYTES * TABLE_COUNT;
// Direct ordinary-object kinds are deliberately outside the resource table
// index space. They are collector dispatch tags, never table addresses.
pub const RECORDS: u32 = u32::MAX;
pub const ARRAYS: u32 = u32::MAX - 1;
pub const VALUES: u32 = u32::MAX - 2;
pub const ENVIRONMENTS: u32 = u32::MAX - 3;
pub const NEWTYPES: u32 = u32::MAX - 4;
pub const BLAMES: u32 = 0;
pub const DIAGNOSTICS: u32 = 1;
pub const FORMATS: u32 = 2;
pub const REGEXES: u32 = 3;
pub const HASHES: u32 = 4;
pub const TESTS: u32 = 5;
pub const DEBUG_EVENTS: u32 = 6;
// Reserved protocol word below the table descriptors; disabled by default.
pub const DEBUG_ENABLED: u32 = 16;
// Read-only Host view of Guest source records: pointer/count, records of 5 u32.
pub const SOURCE_REGISTRY: u32 = 32;
// Read-only content base/length. Re-read after every Guest call.
pub const CONTENT_VIEW: u32 = 48;
pub const WORDS_VIEW: u32 = 40;
pub const WORDS_ORIGIN: u32 = 56;
pub const DIAGNOSTIC_BYTES: u32 = 40;
pub const DEMAND_BYTES: u32 = 12;

pub fn table_address(table: u32) -> u32 {
    TABLE_BASE + table * TABLE_BYTES
}

pub const DIAG_CODE: u64 = 12;
pub const DIAG_MESSAGE: u64 = 16;
pub const DIAG_SUBJECTS: u64 = 20;
pub const DIAG_COUNT: u64 = 24;
pub const DIAG_WARNING: u64 = 28;
pub const DIAG_ROOT: u64 = 32;
pub const DIAG_MESSAGE_TYPE: u64 = 36;
pub const BLAME_COUNT: u32 = STRING_BYTES;
pub const BLAME_SUBJECTS: u32 = STRING_BYTES + 8;
