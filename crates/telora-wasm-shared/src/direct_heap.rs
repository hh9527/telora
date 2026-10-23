//! Independent allocations owned by an exact, nonmoving collector.
//!
//! The default allocator owns placement, free space and reuse. This registry
//! only owns allocation layouts, interior-reference lookup, marks and budgets.
//! Addresses are usize here so the same implementation is tested on the host;
//! the wasm32 runtime must validate its reference namespace at the boundary.
use alloc::{
    alloc::{alloc_zeroed, dealloc},
    vec::Vec,
};
use core::{alloc::Layout, ptr::NonNull};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Overflow,
    Limit,
    Allocation,
    InvalidReference,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub address: usize,
    /// Requested extent (at least one byte), excluding alignment padding.
    pub bytes: usize,
    pub descriptor: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Requested bytes of allocations which have not been swept.
    pub allocated: usize,
    /// Owned allocation layouts, including explicit alignment padding.
    pub storage: usize,
    /// Registry backing capacity. Allocator-private overhead is not measurable.
    pub metadata: usize,
    pub objects: usize,
}

struct Allocation {
    pointer: NonNull<u8>,
    layout: Layout,
    bytes: usize,
    descriptor: u32,
    marked: bool,
}
impl Allocation {
    fn block(&self) -> Block {
        Block {
            address: self.pointer.as_ptr() as usize,
            bytes: self.bytes,
            descriptor: self.descriptor,
        }
    }
}
impl Drop for Allocation {
    fn drop(&mut self) {
        // Each successful allocation has exactly one owning registry entry.
        unsafe {
            dealloc(self.pointer.as_ptr(), self.layout);
        }
    }
}

pub struct Heap {
    allocations: Vec<Allocation>,
    sorted: bool,
    allocated: usize,
    storage: usize,
    limit: usize,
    #[cfg(test)]
    fail_next: Option<FailurePoint>,
}
#[cfg(test)]
#[derive(Clone, Copy, PartialEq)]
enum FailurePoint {
    Registration,
    Storage,
}
impl Heap {
    pub const fn new(limit: usize) -> Self {
        Self {
            allocations: Vec::new(),
            sorted: true,
            allocated: 0,
            storage: 0,
            limit,
            #[cfg(test)]
            fail_next: None,
        }
    }

    pub fn usage(&self) -> Usage {
        Usage {
            allocated: self.allocated,
            storage: self.storage,
            metadata: self.allocations.capacity() * core::mem::size_of::<Allocation>(),
            objects: self.allocations.len(),
        }
    }

    pub fn set_limit(&mut self, limit: usize) -> Result<(), Error> {
        if self
            .storage
            .checked_add(self.usage().metadata)
            .ok_or(Error::Overflow)?
            > limit
        {
            return Err(Error::Limit);
        }
        self.limit = limit;
        Ok(())
    }

    pub fn allocate(&mut self, bytes: usize, descriptor: u32) -> Result<usize, Error> {
        let bytes = bytes.max(1);
        let layout = Layout::from_size_align(bytes, 8)
            .map_err(|_| Error::Overflow)?
            .pad_to_align();
        let storage = self
            .storage
            .checked_add(layout.size())
            .ok_or(Error::Overflow)?;
        self.reserve_registration(storage)?;
        // No operation after acquiring storage can grow the registry or fail.
        #[cfg(test)]
        if self.fail_next == Some(FailurePoint::Storage) {
            self.fail_next = None;
            return Err(Error::Allocation);
        }
        let pointer = NonNull::new(unsafe { alloc_zeroed(layout) }).ok_or(Error::Allocation)?;
        let address = pointer.as_ptr() as usize;
        self.allocations.push(Allocation {
            pointer,
            layout,
            bytes,
            descriptor,
            marked: false,
        });
        self.storage = storage;
        self.allocated += bytes; // bounded by storage, already checked above
        self.sorted = false;
        Ok(address)
    }

    fn reserve_registration(&mut self, storage: usize) -> Result<(), Error> {
        let width = core::mem::size_of::<Allocation>();
        let capacity = self.allocations.capacity();
        let available = self.limit.checked_sub(storage).ok_or(Error::Limit)? / width;
        if capacity > available {
            return Err(Error::Limit);
        }
        if self.allocations.len() == capacity {
            let required = capacity.checked_add(1).ok_or(Error::Overflow)?;
            if required > available {
                return Err(Error::Limit);
            }
            let target = capacity
                .saturating_add((capacity / 2).max(4))
                .max(required)
                .min(available);
            #[cfg(test)]
            if self.fail_next == Some(FailurePoint::Registration) {
                self.fail_next = None;
                return Err(Error::Allocation);
            }
            self.allocations
                .try_reserve_exact(target - self.allocations.len())
                .map_err(|_| Error::Allocation)?;
            if self.allocations.capacity() > available {
                return Err(Error::Limit);
            }
        }
        Ok(())
    }

    /// Prepare an interval index at a tracing boundary, without allocating.
    /// Allocation itself remains append-only in the registry; no sorted insertion
    /// or per-object tree-node allocation is required on the hot path.
    fn index(&mut self, address: usize) -> Result<usize, Error> {
        if !self.sorted {
            self.allocations
                .sort_unstable_by_key(|a| a.pointer.as_ptr() as usize);
            self.sorted = true;
        }
        let index = self
            .allocations
            .partition_point(|a| a.pointer.as_ptr() as usize <= address)
            .checked_sub(1)
            .ok_or(Error::InvalidReference)?;
        let allocation = &self.allocations[index];
        let offset = address - allocation.pointer.as_ptr() as usize;
        if offset >= allocation.bytes {
            return Err(Error::InvalidReference);
        }
        Ok(index)
    }

    pub fn block(&mut self, address: usize) -> Result<Block, Error> {
        let index = self.index(address)?;
        Ok(self.allocations[index].block())
    }

    /// Mark an owning allocation once, including through an interior reference.
    pub fn mark(&mut self, address: usize) -> Result<Option<Block>, Error> {
        let index = self.index(address)?;
        let allocation = &mut self.allocations[index];
        if allocation.marked {
            return Ok(None);
        }
        allocation.marked = true;
        Ok(Some(allocation.block()))
    }

    /// Sweep only after exact tracing has finished. Release callbacks must not
    /// panic, allocate managed objects or invoke language code.
    pub fn sweep(&mut self, mut release: impl FnMut(Block)) {
        let allocated = &mut self.allocated;
        let storage = &mut self.storage;
        self.allocations.retain_mut(|allocation| {
            if allocation.marked {
                allocation.marked = false;
                true
            } else {
                release(allocation.block());
                *allocated -= allocation.bytes;
                *storage -= allocation.layout.size();
                false
            }
        });
    }

    /// Checked interior byte access. Padding and one-past references are not
    /// language objects, even though the allocator owns that storage.
    pub fn bytes(&mut self, address: usize, length: usize) -> Result<&[u8], Error> {
        let index = self.index(address)?;
        let allocation = &self.allocations[index];
        let offset = address - allocation.pointer.as_ptr() as usize;
        if offset.checked_add(length).ok_or(Error::Overflow)? > allocation.bytes {
            return Err(Error::InvalidReference);
        }
        Ok(unsafe { core::slice::from_raw_parts(allocation.pointer.as_ptr().add(offset), length) })
    }

    pub fn bytes_mut(&mut self, address: usize, length: usize) -> Result<&mut [u8], Error> {
        let index = self.index(address)?;
        let allocation = &mut self.allocations[index];
        let offset = address - allocation.pointer.as_ptr() as usize;
        if offset.checked_add(length).ok_or(Error::Overflow)? > allocation.bytes {
            return Err(Error::InvalidReference);
        }
        Ok(unsafe {
            core::slice::from_raw_parts_mut(allocation.pointer.as_ptr().add(offset), length)
        })
    }
}

#[cfg(test)]
mod tests;
