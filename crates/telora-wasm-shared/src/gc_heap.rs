//! Non-moving logical pages. The runtime supplies exact roots and scan descriptors.
//! No allocator operation invokes language code or recursively scans objects.
use alloc::vec::Vec;

const PAGE_BYTES: u32 = 16 * 1024;
const ALIGN: u32 = 8;
const MAX_SMALL: u32 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Overflow,
    Limit,
    InvalidReference,
    Allocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub reference: u32,
    pub bytes: u32,
    pub descriptor: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub allocated: u64,
    pub committed: u64,
    pub metadata: u64,
    pub objects: u32,
    pub pages: u32,
}

#[derive(Clone, Copy, Default)]
struct Slot {
    // 0 = free, 1 = allocated, 2 = marked.
    state: u8,
    bytes: u32,
    descriptor: u32,
}

struct Page {
    base: u32,
    reserved: u32,
    stride: u32,
    slots: Vec<Slot>,
    available: Vec<u32>,
}

impl Page {
    fn extent(&self) -> u32 {
        self.reserved
    }

    fn slot(&self, reference: u32) -> Option<(usize, u32)> {
        if self.slots.is_empty() {
            return None;
        }
        let offset = reference.checked_sub(self.base)?;
        let index = (offset / self.stride) as usize;
        let within = offset % self.stride;
        let slot = self.slots.get(index)?;
        (slot.state != 0 && within < slot.bytes).then_some((index, within))
    }
}

pub struct Heap {
    origin: u32,
    image_end: u32,
    words: Vec<u64>,
    pages: Vec<Page>,
    next: u32,
    limit: u64,
    committed: u64,
    metadata: u64,
    allocated: u64,
    objects: u32,
}

impl Heap {
    /// Logical references are independent of Rust/Wasm allocation addresses.
    pub fn new(origin: u32, limit: u64) -> Result<Self, Error> {
        if origin == 0 || !origin.is_multiple_of(ALIGN) {
            return Err(Error::InvalidReference);
        }
        Ok(Self {
            origin,
            image_end: origin,
            words: Vec::new(),
            pages: Vec::new(),
            next: origin,
            limit,
            committed: 0,
            metadata: 0,
            allocated: 0,
            objects: 0,
        })
    }

    /// Adopt an immutable image prefix. Dynamic allocations start after it.
    pub fn from_words(origin: u32, limit: u64, words: Vec<u64>) -> Result<Self, Error> {
        let mut heap = Self::new(origin, limit)?;
        let length = u32::try_from(words.len().checked_mul(8).ok_or(Error::Overflow)?)
            .map_err(|_| Error::Overflow)?;
        heap.next = origin.checked_add(length).ok_or(Error::Overflow)?;
        heap.image_end = heap.next;
        heap.committed = (words.capacity() as u64) * 8;
        if heap.committed > limit {
            return Err(Error::Limit);
        }
        heap.words = words;
        Ok(heap)
    }

    pub fn words(&self) -> &[u64] {
        &self.words
    }
    pub fn into_words(self) -> Vec<u64> {
        self.words
    }

    pub fn is_image(&self, reference: u32) -> bool {
        reference < self.image_end
    }

    pub fn set_limit(&mut self, limit: u64) -> Result<(), Error> {
        if self
            .committed
            .checked_add(self.metadata)
            .ok_or(Error::Overflow)?
            > limit
        {
            return Err(Error::Limit);
        }
        self.limit = limit;
        Ok(())
    }

    pub fn allocate(&mut self, bytes: u32, descriptor: u32) -> Result<u32, Error> {
        let bytes = bytes.max(1);
        let rounded = bytes.checked_add(ALIGN - 1).ok_or(Error::Overflow)? & !(ALIGN - 1);
        let stride = if rounded <= MAX_SMALL {
            rounded.checked_next_power_of_two().ok_or(Error::Overflow)?
        } else {
            rounded
        };
        let index = if let Some(index) = self
            .pages
            .iter()
            .position(|page| page.stride == stride && !page.available.is_empty())
        {
            index
        } else {
            self.add_page(stride)?
        };
        let page = &mut self.pages[index];
        let slot = page.available.pop().expect("nonempty free list") as usize;
        page.slots[slot] = Slot {
            state: 1,
            bytes,
            descriptor,
        };
        let start = ((page.base - self.origin) / 8) as usize + slot * page.stride as usize / 8;
        self.words[start..start + page.stride as usize / 8].fill(0);
        self.allocated += u64::from(bytes);
        self.objects += 1;
        Ok(page.base + slot as u32 * page.stride)
    }

    fn add_page(&mut self, stride: u32) -> Result<usize, Error> {
        let count = if stride <= MAX_SMALL {
            PAGE_BYTES / stride
        } else {
            1
        };
        let extent = count.checked_mul(stride).ok_or(Error::Overflow)?;
        let reuse = self
            .pages
            .iter()
            .position(|page| page.slots.is_empty() && page.reserved >= extent);
        let next = if reuse.is_some() {
            self.next
        } else {
            self.next.checked_add(extent).ok_or(Error::Overflow)?
        };
        let split = reuse.is_some_and(|index| self.pages[index].reserved > extent);
        let metadata = u64::from(count) * (core::mem::size_of::<Slot>() as u64 + 4)
            + if reuse.is_some() && !split {
                0
            } else {
                core::mem::size_of::<Page>() as u64
            };
        let base = reuse
            .map(|index| self.pages[index].base)
            .unwrap_or(self.next);
        let required =
            usize::try_from(base.checked_add(extent).ok_or(Error::Overflow)? - self.origin)
                .map_err(|_| Error::Overflow)?
                / 8;
        let committed = (self.words.capacity().max(required) as u64) * 8;
        let total = committed
            .checked_add(self.metadata)
            .and_then(|v| v.checked_add(metadata))
            .ok_or(Error::Overflow)?;
        if total > self.limit {
            return Err(Error::Limit);
        }
        // Reserve backing before publishing the page. No physical borrow survives this call.
        self.words
            .try_reserve_exact(required.saturating_sub(self.words.len()))
            .map_err(|_| Error::Allocation)?;
        self.words.resize(self.words.len().max(required), 0);
        self.committed = (self.words.capacity() * 8) as u64;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(count as usize)
            .map_err(|_| Error::Allocation)?;
        slots.resize(count as usize, Slot::default());
        let mut available = Vec::new();
        available
            .try_reserve_exact(count as usize)
            .map_err(|_| Error::Allocation)?;
        available.extend((0..count).rev());
        let base = reuse
            .map(|index| self.pages[index].base)
            .unwrap_or(self.next);
        if split {
            self.pages
                .try_reserve_exact(1)
                .map_err(|_| Error::Allocation)?;
        }
        let page = Page {
            base,
            reserved: extent,
            stride,
            slots,
            available,
        };
        let index = if let Some(index) = reuse {
            if split {
                let reserved = self.pages[index].reserved - extent;
                self.pages.insert(
                    index + 1,
                    Page {
                        base: base + extent,
                        reserved,
                        stride: 0,
                        slots: Vec::new(),
                        available: Vec::new(),
                    },
                );
            }
            self.pages[index] = page;
            index
        } else {
            self.pages
                .try_reserve_exact(1)
                .map_err(|_| Error::Allocation)?;
            self.pages.push(page);
            self.pages.len() - 1
        };
        self.next = next;
        self.committed = (self.words.capacity() * 8) as u64;
        self.metadata = self.retained_metadata();
        Ok(index)
    }

    fn page(&self, reference: u32) -> Option<usize> {
        let index = self
            .pages
            .partition_point(|page| page.base <= reference)
            .checked_sub(1)?;
        let page = &self.pages[index];
        (reference - page.base < page.extent()).then_some(index)
    }

    fn retained_metadata(&self) -> u64 {
        (self.pages.capacity() * core::mem::size_of::<Page>()) as u64
            + self
                .pages
                .iter()
                .map(|page| {
                    (page.slots.capacity() * core::mem::size_of::<Slot>()
                        + page.available.capacity() * 4) as u64
                })
                .sum::<u64>()
    }

    pub fn block(&self, reference: u32) -> Option<Block> {
        let page = &self.pages[self.page(reference)?];
        let (index, _) = page.slot(reference)?;
        let slot = page.slots[index];
        Some(Block {
            reference: page.base + index as u32 * page.stride,
            bytes: slot.bytes,
            descriptor: slot.descriptor,
        })
    }

    /// Mark the owning allocation, including when the root points into an element.
    /// Return its scan work exactly once; never infer edges from arbitrary words.
    pub fn mark(&mut self, reference: u32) -> Result<Option<Block>, Error> {
        let index = self.page(reference).ok_or(Error::InvalidReference)?;
        let page = &mut self.pages[index];
        let (index, _) = page.slot(reference).ok_or(Error::InvalidReference)?;
        let slot = &mut page.slots[index];
        if slot.state == 2 {
            return Ok(None);
        }
        slot.state = 2;
        Ok(Some(Block {
            reference: page.base + index as u32 * page.stride,
            bytes: slot.bytes,
            descriptor: slot.descriptor,
        }))
    }

    /// Exact tracing must finish before sweep. Callbacks release only RT resources.
    pub fn sweep(&mut self, mut release: impl FnMut(Block)) {
        for page in &mut self.pages {
            for (index, slot) in page.slots.iter_mut().enumerate() {
                match slot.state {
                    2 => slot.state = 1,
                    1 => {
                        self.allocated -= u64::from(slot.bytes);
                        self.objects -= 1;
                        release(Block {
                            reference: page.base + index as u32 * page.stride,
                            bytes: slot.bytes,
                            descriptor: slot.descriptor,
                        });
                        *slot = Slot::default();
                        page.available.push(index as u32);
                    }
                    _ => {}
                }
            }
            if !page.slots.is_empty() && page.available.len() == page.slots.len() {
                self.metadata -= (page.slots.len() * (core::mem::size_of::<Slot>() + 4)) as u64;
                page.slots = Vec::new();
                page.available = Vec::new();
            }
        }
        // Coalesce vacant logical extents so varying large requests do not
        // exhaust wasm32 reference space while the actual live graph is empty.
        let mut index = 1;
        while index < self.pages.len() {
            if self.pages[index - 1].slots.is_empty() && self.pages[index].slots.is_empty() {
                let extent = self.pages[index].reserved;
                self.pages[index - 1].reserved += extent;
                self.pages.remove(index);
                self.metadata -= core::mem::size_of::<Page>() as u64;
            } else {
                index += 1;
            }
        }
        self.metadata = self.retained_metadata();
    }

    /// Interior borrows must stay within the requested block, including empty ranges.
    pub fn bytes(&self, reference: u32, length: u32) -> Result<&[u8], Error> {
        let page = &self.pages[self.page(reference).ok_or(Error::InvalidReference)?];
        let (index, within) = page.slot(reference).ok_or(Error::InvalidReference)?;
        if within.checked_add(length).ok_or(Error::Overflow)? > page.slots[index].bytes {
            return Err(Error::InvalidReference);
        }
        let offset = (reference - self.origin) as usize;
        // Words own initialized, aligned bytes; no typed references escape this module.
        let all = unsafe {
            core::slice::from_raw_parts(self.words.as_ptr().cast::<u8>(), self.words.len() * 8)
        };
        Ok(&all[offset..offset + length as usize])
    }

    pub fn bytes_mut(&mut self, reference: u32, length: u32) -> Result<&mut [u8], Error> {
        self.bytes(reference, length)?;
        let offset = (reference - self.origin) as usize;
        let all = unsafe {
            core::slice::from_raw_parts_mut(
                self.words.as_mut_ptr().cast::<u8>(),
                self.words.len() * 8,
            )
        };
        Ok(&mut all[offset..offset + length as usize])
    }

    pub fn usage(&self) -> Usage {
        Usage {
            committed: self.committed,
            metadata: self.metadata,
            pages: self.pages.len() as u32,
            allocated: self.allocated,
            objects: self.objects,
        }
    }
}

#[cfg(test)]
mod tests;
