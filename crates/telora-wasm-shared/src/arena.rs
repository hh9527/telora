//! Offset-only storage primitives. Neither arena exports persistent raw pointers.
use alloc::vec::Vec;

pub mod content;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Overflow,
    Bounds,
    Frozen,
    Phase,
}

/// Word index, with zero reserved for the absent reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct HeapId(pub u32);

#[derive(Default)]
pub struct Words {
    words: Vec<u64>,
    static_end: Option<usize>,
    work_base: Option<usize>,
}

impl Words {
    pub fn allocate(&mut self, bytes: u32) -> Result<HeapId, Error> {
        if self.words.is_empty() { self.words.push(0); }
        let count = (bytes.max(1) as usize).div_ceil(8);
        let start = self.words.len();
        let end = start.checked_add(count).ok_or(Error::Overflow)?;
        // Wasm32 accesses ultimately require a byte address, not just a word ID.
        if end > u32::MAX as usize / 8 { return Err(Error::Overflow); }
        self.words.resize(end, 0);
        Ok(HeapId(start as u32))
    }

    pub fn seal_static(&mut self) -> Result<(), Error> {
        if self.static_end.is_some() { return Err(Error::Phase); }
        if self.words.is_empty() { self.words.push(0); }
        self.static_end = Some(self.words.len());
        Ok(())
    }

    pub fn seal_work(&mut self) -> Result<(), Error> {
        if self.static_end.is_none() || self.work_base.is_some() { return Err(Error::Phase); }
        self.work_base = Some(self.words.len());
        Ok(())
    }

    pub fn reset(&mut self) -> Result<(), Error> {
        self.words.truncate(self.work_base.ok_or(Error::Phase)?);
        Ok(())
    }

    pub fn copy_static(&self) -> Result<Self, Error> {
        let end = self.static_end.ok_or(Error::Phase)?;
        Ok(Self { words: self.words[..end].to_vec(), static_end: Some(end), work_base: None })
    }

    pub fn is_static(&self, id: HeapId) -> bool {
        id.0 != 0 && self.static_end.is_some_and(|end| (id.0 as usize) < end)
    }

    pub fn get(&self, id: HeapId, count: usize) -> Result<&[u64], Error> {
        if id.0 == 0 { return Err(Error::Bounds); }
        let start = id.0 as usize;
        let end = start.checked_add(count).ok_or(Error::Overflow)?;
        self.words.get(start..end).ok_or(Error::Bounds)
    }

    pub fn get_mut(&mut self, id: HeapId, count: usize) -> Result<&mut [u64], Error> {
        if id.0 == 0 { return Err(Error::Bounds); }
        let start = id.0 as usize;
        let frozen = self.work_base.or(self.static_end).unwrap_or(0);
        if start < frozen { return Err(Error::Frozen); }
        let end = start.checked_add(count).ok_or(Error::Overflow)?;
        self.words.get_mut(start..end).ok_or(Error::Bounds)
    }

    pub fn len(&self) -> usize { self.words.len() }
    pub fn is_empty(&self) -> bool { self.words.is_empty() }
}
