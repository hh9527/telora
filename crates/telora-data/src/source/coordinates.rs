//! Portable source coordinates. Parser byte spans are converted at the boundary.
use super::{Loc, LocationError};
use alloc::vec::Vec;

/// Five u32 coordinates for host transport; values carry a separate LocId.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceCoordinates(pub [u32; 5]);

impl SourceCoordinates {
    pub const fn source(self) -> u32 { self.0[0] }
    pub const fn start(self) -> u64 { ((self.0[1] as u64) << 32) | self.0[2] as u64 }
    pub const fn end(self) -> u64 { ((self.0[3] as u64) << 32) | self.0[4] as u64 }
    pub const fn position(point: u64) -> (u32, u32) { ((point >> 32) as u32, point as u32) }
    pub fn new(source: u32, start: u64, end: u64) -> Result<Self, LocationError> {
        if source == 0 || start > end { return Err(LocationError::CoordinateCapacity); }
        Ok(Self([source, (start >> 32) as u32, start as u32, (end >> 32) as u32, end as u32]))
    }
}

/// Host/compiler-only index, never serialized into an artifact.
#[derive(Clone, Debug)]
pub struct LineIndex {
    starts: Vec<u32>,
    ends: Vec<u32>,
}

impl LineIndex {
    /// Per-source diagnostic index: line start and end excluding its EOL.
    pub fn ranges(&self) -> impl ExactSizeIterator<Item = [u32; 2]> + '_ {
        self.starts.iter().zip(&self.ends).map(|(&start, &end)| [start, end])
    }

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
                ends.push(at as u32);
                if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
                    at += 1;
                }
                starts.push((at + 1) as u32);
            }
            at += 1;
        }
        ends.push(at as u32);
        Ok(Self { starts, ends })
    }
    pub fn point(&self, byte: u32) -> u64 {
        let line = self.starts.partition_point(|&start| start <= byte) - 1;
        // The interior of CRLF denotes the preceding line's end.
        ((line as u64) << 32) | (byte.min(self.ends[line]) - self.starts[line]) as u64
    }
    pub fn byte(&self, point: u64) -> Option<u32> {
        let (line, column) = SourceCoordinates::position(point);
        let start = *self.starts.get(line as usize)?;
        let end = *self.ends.get(line as usize)?;
        (column <= end - start).then_some(start + column)
    }
    pub fn pack(&self, loc: Loc) -> SourceCoordinates {
        SourceCoordinates::new(loc.source.get(), self.point(loc.start), self.point(loc.end))
            .expect("registered source has valid coordinates")
    }
}
