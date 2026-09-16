//! Immutable byte allocations and views; collection copies each live envelope once.
use alloc::{collections::BTreeMap, vec::Vec};
use super::Error;

/// Internal offsets, never a user-visible identity or a linear-memory pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slice {
    pub start: u32,
    pub end: u32,
    pub raw_start: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bytes {
    Inline { data: [u8; 15], len: u8 },
    Slice(Slice),
}

impl Bytes {
    /// The final byte is 0..15 for inline content, 16 for a three-offset slice.
    pub fn encode(self) -> Result<[u8; 16], Error> {
        let mut result = [0; 16];
        match self {
            Self::Inline { data, len } => {
                if len >= 16 { return Err(Error::Bounds); }
                result[..usize::from(len)].copy_from_slice(&data[..usize::from(len)]);
                result[15] = len;
            }
            Self::Slice(slice) => {
                if slice.raw_start > slice.start || slice.start > slice.end
                    || slice.end - slice.start < 16 { return Err(Error::Bounds); }
                for (at, word) in [slice.start, slice.end, slice.raw_start].into_iter().enumerate() {
                    result[at * 4..at * 4 + 4].copy_from_slice(&word.to_le_bytes());
                }
                result[15] = 16;
            }
        }
        Ok(result)
    }

    pub fn decode(bytes: [u8; 16]) -> Result<Self, Error> {
        match bytes[15] {
            len @ 0..=15 => Ok(inline(&bytes[..usize::from(len)])),
            16 => {
                if bytes[12..15] != [0; 3] { return Err(Error::Bounds); }
                let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
                let slice = Slice { start: word(0), end: word(4), raw_start: word(8) };
                if slice.raw_start > slice.start || slice.start > slice.end
                    || slice.end - slice.start < 16 { return Err(Error::Bounds); }
                Ok(Self::Slice(slice))
            }
            _ => Err(Error::Bounds),
        }
    }
}

#[derive(Default)]
pub struct Content {
    bytes: Vec<u8>,
    work_base: Option<usize>,
}

impl Content {
    pub fn insert(&mut self, bytes: &[u8]) -> Result<Bytes, Error> {
        if bytes.len() < 16 { return Ok(inline(bytes)); }
        let start = self.bytes.len();
        let end = start.checked_add(bytes.len()).ok_or(Error::Overflow)?;
        let end = u32::try_from(end).map_err(|_| Error::Overflow)?;
        self.bytes.extend_from_slice(bytes);
        Ok(Bytes::Slice(Slice { start: start as u32, end, raw_start: start as u32 }))
    }

    pub fn view<'a>(&'a self, value: &'a Bytes) -> Result<&'a [u8], Error> {
        match value {
            Bytes::Inline { data, len } => data.get(..usize::from(*len)).ok_or(Error::Bounds),
            Bytes::Slice(slice) => {
                if slice.raw_start > slice.start { return Err(Error::Bounds); }
                self.bytes.get(slice.start as usize..slice.end as usize).ok_or(Error::Bounds)
            }
        }
    }

    /// Byte ranges only. String callers additionally enforce UTF-8 boundaries.
    pub fn slice(&self, value: &Bytes, start: u32, end: u32) -> Result<Bytes, Error> {
        let bytes = self.view(value)?;
        let selected = bytes.get(start as usize..end as usize).ok_or(Error::Bounds)?;
        if selected.len() < 16 { return Ok(inline(selected)); }
        let Bytes::Slice(parent) = value else { return Err(Error::Bounds); };
        Ok(Bytes::Slice(Slice {
            start: parent.start.checked_add(start).ok_or(Error::Overflow)?,
            end: parent.start.checked_add(end).ok_or(Error::Overflow)?,
            raw_start: parent.raw_start,
        }))
    }

    pub fn seal_work(&mut self) -> Result<(), Error> {
        if self.work_base.is_some() { return Err(Error::Phase); }
        self.work_base = Some(self.bytes.len());
        Ok(())
    }

    pub fn reset(&mut self) -> Result<(), Error> {
        self.bytes.truncate(self.work_base.ok_or(Error::Phase)?);
        Ok(())
    }

    pub fn len(&self) -> usize { self.bytes.len() }
    pub fn is_empty(&self) -> bool { self.bytes.is_empty() }
}

fn inline(bytes: &[u8]) -> Bytes {
    let mut data = [0; 15];
    data[..bytes.len()].copy_from_slice(bytes);
    Bytes::Inline { data, len: bytes.len() as u8 }
}

/// Only GC owns this table. There is no persistent RawBytes registry.
#[derive(Default)]
pub struct LiveRanges(BTreeMap<u32, (u32, u32)>);

impl LiveRanges {
    pub fn observe(&mut self, content: &Content, value: &Bytes) -> Result<(), Error> {
        content.view(value)?;
        if let Bytes::Slice(slice) = value {
            let range = self.0.entry(slice.raw_start).or_insert((slice.start, slice.end));
            range.0 = range.0.min(slice.start);
            range.1 = range.1.max(slice.end);
        }
        Ok(())
    }

    /// Consumes the discovery phase, preventing late widening after copying.
    pub fn copy(self, source: &Content) -> Result<(Content, Relocation), Error> {
        let mut target = Content::default();
        let mut moved = BTreeMap::new();
        for (raw, (lo, hi)) in self.0 {
            let base = u32::try_from(target.bytes.len()).map_err(|_| Error::Overflow)?;
            let bytes = source.bytes.get(lo as usize..hi as usize).ok_or(Error::Bounds)?;
            let end = target.bytes.len().checked_add(bytes.len()).ok_or(Error::Overflow)?;
            u32::try_from(end).map_err(|_| Error::Overflow)?;
            target.bytes.extend_from_slice(bytes);
            moved.insert(raw, (lo, hi, base));
        }
        Ok((target, Relocation(moved)))
    }
}

pub struct Relocation(BTreeMap<u32, (u32, u32, u32)>);

impl Relocation {
    pub fn apply(&self, value: Bytes) -> Result<Bytes, Error> {
        let Bytes::Slice(slice) = value else { return Ok(value); };
        let &(lo, hi, base) = self.0.get(&slice.raw_start).ok_or(Error::Bounds)?;
        if slice.start < lo || slice.end < slice.start || slice.end > hi { return Err(Error::Bounds); }
        Ok(Bytes::Slice(Slice {
            start: base.checked_add(slice.start - lo).ok_or(Error::Overflow)?,
            end: base.checked_add(slice.end - lo).ok_or(Error::Overflow)?,
            raw_start: base,
        }))
    }
}
