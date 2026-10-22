//! Fixed byte-view encoding. Raw storage is owned by the managed Wasm heap.
use super::Error;

/// Internal offsets, never a user-visible identity or a linear-memory pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slice {
    pub start: u32,
    pub end: u32,
    pub raw: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bytes {
    Inline { data: [u8; 15], len: u8 },
    Slice(Slice),
}

impl Bytes {
    /// The final byte is 0..15 for inline content, 16 for a raw reference and relative slice.
    pub fn encode(self) -> Result<[u8; 16], Error> {
        let mut result = [0; 16];
        match self {
            Self::Inline { data, len } => {
                if len >= 16 {
                    return Err(Error::Bounds);
                }
                result[..usize::from(len)].copy_from_slice(&data[..usize::from(len)]);
                result[15] = len;
            }
            Self::Slice(slice) => {
                if slice.raw == 0 || slice.start > slice.end || slice.end - slice.start < 16 {
                    return Err(Error::Bounds);
                }
                for (at, word) in [slice.raw, slice.start, slice.end].into_iter().enumerate() {
                    result[at * 4..at * 4 + 4].copy_from_slice(&word.to_le_bytes());
                }
                result[15] = 16;
            }
        }
        Ok(result)
    }

    pub fn decode(bytes: [u8; 16]) -> Result<Self, Error> {
        match bytes[15] {
            len @ 0..=15 => Ok(Self::Inline {
                data: bytes[..15].try_into().unwrap(),
                len,
            }),
            16 => {
                if bytes[12..15] != [0; 3] {
                    return Err(Error::Bounds);
                }
                let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                let slice = Slice {
                    raw: word(0),
                    start: word(4),
                    end: word(8),
                };
                if slice.raw == 0 || slice.start > slice.end || slice.end - slice.start < 16 {
                    return Err(Error::Bounds);
                }
                Ok(Self::Slice(slice))
            }
            _ => Err(Error::Bounds),
        }
    }
}
