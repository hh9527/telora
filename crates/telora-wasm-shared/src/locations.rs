//! RFC 0300 location identities. IDs are table indices, never memory pointers.

pub const STATIC_BIT: u32 = 0x8000_0000;
pub const RECORD_BYTES: u32 = 20;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct LocId(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocationIndex {
    None,
    Static(u32),
    Initialization(u32),
}

impl LocId {
    pub const NONE: Self = Self(0);

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }
    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn static_index(index: u32) -> Option<Self> {
        if index < STATIC_BIT {
            Some(Self(STATIC_BIT | index))
        } else {
            None
        }
    }

    pub const fn initialization_index(index: u32) -> Option<Self> {
        if index < STATIC_BIT - 1 {
            Some(Self(index + 1))
        } else {
            None
        }
    }

    pub const fn index(self) -> LocationIndex {
        if self.0 == 0 {
            LocationIndex::None
        } else if self.0 & STATIC_BIT != 0 {
            LocationIndex::Static(self.0 & !STATIC_BIT)
        } else {
            LocationIndex::Initialization(self.0 - 1)
        }
    }
}

/// Canonical little-endian record, independent of the host's structure layout.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct LocationRecord {
    pub source: u32,
    pub start_line: u32,
    pub start_offset: u32,
    pub end_line: u32,
    pub end_offset: u32,
}

impl LocationRecord {
    pub fn encode(self) -> [u8; RECORD_BYTES as usize] {
        let mut bytes = [0; RECORD_BYTES as usize];
        for (chunk, word) in bytes.chunks_exact_mut(4).zip([
            self.source,
            self.start_line,
            self.start_offset,
            self.end_line,
            self.end_offset,
        ]) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes = bytes.get(..RECORD_BYTES as usize)?;
        let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        Some(Self {
            source: word(0),
            start_line: word(4),
            start_offset: word(8),
            end_line: word(12),
            end_offset: word(16),
        })
    }
}

/// Checks both table bounds and the full record extent before computing an address.
pub fn record_address(base: u32, count: u32, index: u32, memory_bytes: u64) -> Option<u32> {
    if index >= count {
        return None;
    }
    let address = base.checked_add(index.checked_mul(RECORD_BYTES)?)?;
    let end = u64::from(address) + u64::from(RECORD_BYTES);
    if end > memory_bytes || end > 1u64 << 32 {
        return None;
    }
    Some(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_have_disjoint_spaces_and_checked_limits() {
        assert_eq!(LocId::NONE.index(), LocationIndex::None);
        for index in [0, 1, STATIC_BIT - 2] {
            assert_eq!(
                LocId::static_index(index).unwrap().index(),
                LocationIndex::Static(index)
            );
            assert_eq!(
                LocId::initialization_index(index).unwrap().index(),
                LocationIndex::Initialization(index)
            );
        }
        assert_eq!(
            LocId::static_index(STATIC_BIT - 1).unwrap().bits(),
            u32::MAX
        );
        assert!(LocId::static_index(STATIC_BIT).is_none());
        assert!(LocId::initialization_index(STATIC_BIT - 1).is_none());
    }

    #[test]
    fn records_preserve_full_width_coordinates() {
        let loc = LocationRecord {
            source: 70_000,
            start_line: 90_000,
            start_offset: 1 << 25,
            end_line: u32::MAX,
            end_offset: u32::MAX,
        };
        assert_eq!(LocationRecord::decode(&loc.encode()), Some(loc));
        assert_eq!(LocationRecord::decode(&[0; 19]), None);
        assert_eq!(&loc.encode()[..4], &70_000u32.to_le_bytes());
    }

    #[test]
    fn addressing_rejects_out_of_bounds_and_wraparound() {
        assert_eq!(record_address(64, 2, 1, 104), Some(84));
        assert_eq!(record_address(64, 2, 1, 103), None);
        assert_eq!(record_address(64, 2, 2, 1024), None);
        assert_eq!(record_address(u32::MAX - 7, 1, 0, 1u64 << 32), None);
        assert_eq!(record_address(64, u32::MAX, u32::MAX - 1, 1u64 << 32), None);
    }
}
