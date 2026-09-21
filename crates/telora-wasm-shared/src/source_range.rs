//! Inline source byte ranges. No interning or runtime location identity.

pub const BYTES: usize = 8;
pub const SOURCE_BITS: u32 = 14;
pub const OFFSET_BITS: u32 = 25;
pub const SOURCE_LIMIT: u32 = 1 << SOURCE_BITS;
pub const OFFSET_LIMIT: u32 = 1 << OFFSET_BITS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceRange {
    pub source: u32,
    pub start: u32,
    pub end: u32,
}

impl SourceRange {
    pub const NONE: Self = Self {
        source: 0,
        start: 0,
        end: 0,
    };

    /// Source length is supplied by the source registry, not encoded per value.
    pub fn checked(source: u32, start: u32, end: u32, source_len: u32) -> Option<Self> {
        if source >= SOURCE_LIMIT
            || start >= OFFSET_LIMIT
            || end >= OFFSET_LIMIT
            || start > end
            || end > source_len
            || (source == 0 && (start != 0 || end != 0))
        {
            return None;
        }
        Some(Self { source, start, end })
    }

    pub fn encode(self) -> [u8; BYTES] {
        self.packed().to_le_bytes()
    }

    pub const fn packed(self) -> u64 {
        ((self.source as u64) << (OFFSET_BITS * 2))
            | ((self.start as u64) << OFFSET_BITS)
            | self.end as u64
    }

    pub fn unpack(packed: u64) -> Option<Self> {
        let mask = u64::from(OFFSET_LIMIT - 1);
        let result = Self {
            source: (packed >> (OFFSET_BITS * 2)) as u32,
            start: ((packed >> OFFSET_BITS) & mask) as u32,
            end: (packed & mask) as u32,
        };
        (result.start <= result.end
            && (result.source != 0 || (result.start == 0 && result.end == 0)))
            .then_some(result)
    }

    pub fn decode(bytes: &[u8], source_len: u32) -> Option<Self> {
        let packed = u64::from_le_bytes(bytes.get(..BYTES)?.try_into().ok()?);
        let result = Self::unpack(packed)?;
        Self::checked(result.source, result.start, result.end, source_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_range_has_no_line_partition_or_signed_id_bit() {
        let range = SourceRange::checked(
            SOURCE_LIMIT - 1,
            OFFSET_LIMIT - 2,
            OFFSET_LIMIT - 1,
            OFFSET_LIMIT - 1,
        )
        .unwrap();
        assert_eq!(
            SourceRange::decode(&range.encode(), OFFSET_LIMIT - 1),
            Some(range)
        );
        assert_eq!(
            SourceRange::decode(&range.encode()[..7], OFFSET_LIMIT - 1),
            None
        );
        assert_eq!(SourceRange::checked(1, 9, 8, 10), None);
        assert_eq!(SourceRange::checked(1, 0, 11, 10), None);
        assert_eq!(SourceRange::checked(0, 1, 1, 10), None);
        assert_eq!(SourceRange::checked(0, 0, 0, 0), Some(SourceRange::NONE));
        assert_eq!(SourceRange::checked(SOURCE_LIMIT, 0, 0, 0), None);
        assert_eq!(SourceRange::checked(1, 0, OFFSET_LIMIT, OFFSET_LIMIT), None);
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
