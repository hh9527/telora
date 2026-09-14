//! Portable source coordinates. Parser byte spans are converted at the boundary.
use super::{Loc, LocationError};
use alloc::vec::Vec;

/// Three little-endian words: source and high position bytes, then low positions.
/// Each position is `(zero_based_line << 24) | utf8_byte_column`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompactLoc(pub [u32; 3]);

impl CompactLoc {
    pub const fn source(self) -> u32 {
        self.0[0] & 0xffff
    }
    pub const fn start(self) -> u64 {
        self.0[1] as u64 | (((self.0[0] >> 16) & 0xff) as u64) << 32
    }
    pub const fn end(self) -> u64 {
        self.0[2] as u64 | ((self.0[0] >> 24) as u64) << 32
    }
    pub const fn position(point: u64) -> (u32, u32) {
        ((point >> 24) as u32, (point & 0xffffff) as u32)
    }
    pub fn new(source: u32, start: u64, end: u64) -> Result<Self, LocationError> {
        if source == 0 || source > 0xffff || start >= 1 << 40 || end >= 1 << 40 || start > end {
            return Err(LocationError::CompactCapacity);
        }
        Ok(Self([
            source | ((start >> 32) as u32) << 16 | ((end >> 32) as u32) << 24,
            start as u32,
            end as u32,
        ]))
    }
}

/// Host/compiler-only index, never serialized into an artifact.
#[derive(Clone, Debug)]
pub struct LineIndex {
    starts: Vec<u32>,
    ends: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: &str) -> Result<Self, LocationError> {
        if text.len() > u32::MAX as usize {
            return Err(LocationError::SourceTooLarge);
        }
        let bytes = text.as_bytes();
        let mut starts = vec![0];
        let mut ends = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            if matches!(bytes[at], b'\r' | b'\n') {
                if starts.len() == 1 << 16 {
                    return Err(LocationError::CompactCapacity);
                }
                ends.push(at as u32);
                if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
                    at += 1;
                }
                starts.push((at + 1) as u32);
            }
            at += 1;
        }
        ends.push(at as u32);
        if starts.len() > 1 << 16
            || starts
                .iter()
                .zip(&ends)
                .any(|(start, end)| end - start > 0xffffff)
        {
            return Err(LocationError::CompactCapacity);
        }
        Ok(Self { starts, ends })
    }
    pub fn point(&self, byte: u32) -> u64 {
        let line = self.starts.partition_point(|&start| start <= byte) - 1;
        // The interior of CRLF denotes the preceding line's end.
        ((line as u64) << 24) | (byte.min(self.ends[line]) - self.starts[line]) as u64
    }
    pub fn byte(&self, point: u64) -> Option<u32> {
        let (line, column) = CompactLoc::position(point);
        let start = *self.starts.get(line as usize)?;
        let end = *self.ends.get(line as usize)?;
        (column <= end - start).then_some(start + column)
    }
    pub fn pack(&self, loc: Loc) -> CompactLoc {
        CompactLoc::new(loc.source.get(), self.point(loc.start), self.point(loc.end))
            .expect("registered source fits compact coordinates")
    }
}
