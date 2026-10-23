//! RFC 0315 allocation/ownership prototype. Not the production runtime.
//! Run with rustc --edition=2024 --test; no external dependencies.
use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ptr::NonNull;

#[derive(Debug, PartialEq, Eq)]
enum Error {
    Overflow,
    Limit,
    Allocation,
    InvalidReference,
}

struct Block {
    pointer: NonNull<u8>,
    layout: Layout,
    bytes: usize,
    descriptor: u32,
    marked: bool,
}
impl Block {
    fn base(&self) -> usize {
        self.pointer.as_ptr() as usize
    }
    fn contains(&self, address: usize) -> bool {
        address
            .checked_sub(self.base())
            .is_some_and(|offset| offset < self.bytes)
    }
}
impl Drop for Block {
    fn drop(&mut self) {
        // The exact original Layout belongs to this unique registry entry.
        unsafe {
            dealloc(self.pointer.as_ptr(), self.layout);
        }
    }
}

struct Heap {
    // Linear lookup is deliberately only a semantic prototype. Production needs
    // a measured interval index; no allocator free-list/size classes are added.
    blocks: Vec<Block>,
    allocated: usize,
    limit: usize,
}
impl Heap {
    fn new(limit: usize) -> Self {
        Self {
            blocks: Vec::new(),
            allocated: 0,
            limit,
        }
    }
    fn allocate(&mut self, bytes: usize, descriptor: u32) -> Result<usize, Error> {
        self.allocate_with(bytes, descriptor, |blocks| {
            blocks.try_reserve(1).map_err(|_| Error::Allocation)
        })
    }
    fn allocate_with(
        &mut self,
        bytes: usize,
        descriptor: u32,
        reserve: impl FnOnce(&mut Vec<Block>) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        let bytes = bytes.max(1);
        let layout = Layout::from_size_align(bytes, 8).map_err(|_| Error::Overflow)?;
        let total = self.allocated.checked_add(bytes).ok_or(Error::Overflow)?;
        if total > self.limit {
            return Err(Error::Limit);
        }
        // Reserve registration capacity BEFORE acquiring object storage: failure
        // cannot strand an allocation. Registration after alloc cannot allocate.
        reserve(&mut self.blocks)?;
        let pointer = NonNull::new(unsafe { alloc_zeroed(layout) }).ok_or(Error::Allocation)?;
        let block = Block {
            pointer,
            layout,
            bytes,
            descriptor,
            marked: false,
        };
        let address = block.base();
        self.blocks.push(block);
        self.allocated = total;
        Ok(address)
    }
    fn owner(&self, address: usize) -> Result<&Block, Error> {
        self.blocks
            .iter()
            .find(|block| block.contains(address))
            .ok_or(Error::InvalidReference)
    }
    fn bytes_mut(&mut self, address: usize, length: usize) -> Result<&mut [u8], Error> {
        let block = self
            .blocks
            .iter_mut()
            .find(|b| b.contains(address))
            .ok_or(Error::InvalidReference)?;
        let offset = address - block.base();
        if offset.checked_add(length).ok_or(Error::Overflow)? > block.bytes {
            return Err(Error::InvalidReference);
        }
        Ok(unsafe { std::slice::from_raw_parts_mut(block.pointer.as_ptr().add(offset), length) })
    }
    fn mark(&mut self, address: usize) -> Result<bool, Error> {
        let block = self
            .blocks
            .iter_mut()
            .find(|b| b.contains(address))
            .ok_or(Error::InvalidReference)?;
        let first = !block.marked;
        block.marked = true;
        Ok(first)
    }
    fn sweep(&mut self, mut release: impl FnMut(usize, u32)) {
        let mut freed = 0;
        self.blocks.retain_mut(|block| {
            if block.marked {
                block.marked = false;
                true
            } else {
                release(block.base(), block.descriptor);
                freed += block.bytes;
                false // Block::drop frees storage exactly once.
            }
        });
        self.allocated -= freed;
    }
}

#[test]
fn independent_allocations_are_stable_zeroed_and_aligned() {
    let mut heap = Heap::new(1 << 20);
    let first = heap.allocate(17, 3).unwrap();
    assert_eq!(first % 8, 0);
    assert_eq!(heap.bytes_mut(first, 17).unwrap(), [0; 17]);
    heap.bytes_mut(first + 16, 1).unwrap()[0] = 42;
    for _ in 0..512 {
        heap.allocate(31, 0).unwrap();
    }
    assert_eq!(heap.bytes_mut(first + 16, 1).unwrap(), [42]);
    assert_eq!(heap.owner(first + 16).unwrap().base(), first);
}

#[test]
fn interior_roots_keep_the_owner_alive_and_release_once() {
    let mut heap = Heap::new(1024);
    let live = heap.allocate(24, 11).unwrap();
    let dead = heap.allocate(16, 12).unwrap();
    assert!(heap.mark(live + 8).unwrap());
    assert!(!heap.mark(live + 23).unwrap());
    let mut released = Vec::new();
    heap.sweep(|base, descriptor| released.push((base, descriptor)));
    assert_eq!(released, [(dead, 12)]);
    assert_eq!(heap.allocated, 24);
    assert!(heap.owner(live).is_ok());
    heap.sweep(|base, descriptor| released.push((base, descriptor)));
    heap.sweep(|_, _| panic!("double release"));
    assert_eq!(released, [(dead, 12), (live, 11)]);
    assert_eq!(heap.allocated, 0);
}

#[test]
fn published_actor_root_survives_calls_without_freeze_or_reset() {
    let mut heap = Heap::new(4096);
    let actor = heap.allocate(24, 1).unwrap();
    let property = heap.allocate(32, 2).unwrap();
    heap.bytes_mut(property, 1).unwrap()[0] = 7;
    for _ in 0..100 {
        heap.allocate(128, 3).unwrap();
        // A closed-layout scanner would discover property from the actor.
        heap.mark(actor).unwrap();
        heap.mark(property).unwrap();
        heap.sweep(|_, _| {});
        assert_eq!(heap.allocated, 56);
        assert_eq!(heap.bytes_mut(property, 1).unwrap(), [7]);
    }
    heap.sweep(|_, _| {});
    assert_eq!(heap.allocated, 0);
}

#[test]
fn checked_boundaries_and_registration_failure_are_atomic() {
    let mut heap = Heap::new(64);
    let zero = heap.allocate(0, 0).unwrap();
    assert_eq!(heap.owner(zero).unwrap().bytes, 1);
    let value = heap.allocate(16, 0).unwrap();
    assert_eq!(heap.mark(value + 16), Err(Error::InvalidReference));
    assert_eq!(heap.bytes_mut(value + 15, 2), Err(Error::InvalidReference));
    assert_eq!(heap.allocate(64, 0), Err(Error::Limit));
    assert_eq!(heap.allocate(usize::MAX, 0), Err(Error::Overflow));
    let before = (heap.blocks.len(), heap.allocated);
    assert_eq!(
        heap.allocate_with(8, 0, |_| Err(Error::Allocation)),
        Err(Error::Allocation)
    );
    assert_eq!((heap.blocks.len(), heap.allocated), before);
    assert_eq!(heap.mark(0), Err(Error::InvalidReference));
}
