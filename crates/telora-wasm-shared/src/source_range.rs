//! Inline source byte ranges. No interning or runtime location identity.

pub const BYTES: usize = 12;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceRange {
    pub source: u32,
    pub start: u32,
    pub end: u32,
}

impl SourceRange {
    pub const NONE: Self = Self { source: 0, start: 0, end: 0 };

    /// Source length is supplied by the source registry, not encoded per value.
    pub fn checked(source: u32, start: u32, end: u32, source_len: u32) -> Option<Self> {
        if start > end || end > source_len || (source == 0 && (start != 0 || end != 0)) {
            return None;
        }
        Some(Self { source, start, end })
    }

    pub fn encode(self) -> [u8; BYTES] {
        let mut bytes = [0; BYTES];
        for (chunk, value) in bytes.chunks_exact_mut(4).zip([self.source, self.start, self.end]) {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    pub fn decode(bytes: &[u8], source_len: u32) -> Option<Self> {
        let bytes = bytes.get(..BYTES)?;
        let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        Self::checked(word(0), word(4), word(8), source_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_range_has_no_line_partition_or_signed_id_bit() {
        assert_eq!(core::mem::size_of::<SourceRange>(), BYTES);
        let range = SourceRange::checked(u32::MAX, u32::MAX - 1, u32::MAX, u32::MAX).unwrap();
        assert_eq!(SourceRange::decode(&range.encode(), u32::MAX), Some(range));
        assert_eq!(SourceRange::decode(&range.encode()[..11], u32::MAX), None);
        assert_eq!(SourceRange::checked(1, 9, 8, 10), None);
        assert_eq!(SourceRange::checked(1, 0, 11, 10), None);
        assert_eq!(SourceRange::checked(0, 1, 1, 10), None);
        assert_eq!(SourceRange::checked(0, 0, 0, 0), Some(SourceRange::NONE));
    }

    #[test]
    fn ranges_refer_to_original_utf8_bytes_including_eol() {
        for (input, start) in [("α\nx", 3), ("α\r\nx", 4), ("α\rx", 3)] {
            let range = SourceRange::checked(1, start, start + 1, input.len() as u32).unwrap();
            assert_eq!(&input[range.start as usize..range.end as usize], "x");
            let empty = SourceRange::checked(1, range.end, range.end, input.len() as u32).unwrap();
            assert_eq!(empty.start, empty.end);
        }
    }
}
