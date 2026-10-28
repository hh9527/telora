//! Compiler-owned request for compact column maps and their private builder.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Request {
    pub operation: u32,
    pub input: u32,
    pub key: u32,
    pub value: u32,
    pub comparator: u32,
    pub order: u32,
    pub key_type: u32,
    pub key_width: u32,
    pub value_type: u32,
    pub value_width: u32,
    pub pair_type: u32,
    pub pair_width: u32,
    pub pair_bytes: u32,
    pub key_offset: u32,
    pub value_offset: u32,
    pub pair_inline: u32,
    /// MAX denotes comparator failure. Otherwise operation-specific status.
    pub status: u32,
}
impl Request {
    pub const BYTES: u32 = core::mem::size_of::<Self>() as u32;
    pub const STATUS: u32 = core::mem::offset_of!(Self, status) as u32;
    pub fn words(self) -> [u32; 17] {
        [
            self.operation,
            self.input,
            self.key,
            self.value,
            self.comparator,
            self.order,
            self.key_type,
            self.key_width,
            self.value_type,
            self.value_width,
            self.pair_type,
            self.pair_width,
            self.pair_bytes,
            self.key_offset,
            self.value_offset,
            self.pair_inline,
            self.status,
        ]
    }
}
