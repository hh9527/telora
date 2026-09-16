//! Ownership of language blocks, allocated by std like every other Rust object.
//! This is not a global allocator. Host buffers and Rust temporaries are ordinary
//! allocations and never enter these main/work ownership lists.
use std::{boxed::Box, vec::Vec};

pub(crate) struct Block {
    words: Box<[u64]>,
    used: usize,
}
static mut MAIN: Vec<Block> = Vec::new();
static mut WORK: Vec<Block> = Vec::new();
static mut STATIC_END: u32 = 0;
static mut FROZEN: bool = false;

pub unsafe fn set_static_end(end: u32) {
    unsafe {
        assert_eq!(*core::ptr::addr_of!(STATIC_END), 0);
        STATIC_END = end;
    }
}

pub unsafe fn allocate(bytes: u32) -> u32 {
    let words = (bytes.max(1) as usize).div_ceil(8);
    unsafe {
        let work = &mut *core::ptr::addr_of_mut!(WORK);
        if work.last().is_none_or(|block| block.words.len() - block.used < words) {
            // Small values share stable zeroed storage. Large payloads get an
            // exact block; neither case changes the Rust global allocator.
            let capacity = if words <= 1024 { 4096 } else { words };
            work.push(Block { words: vec![0; capacity].into_boxed_slice(), used: 0 });
        }
        let block = work.last_mut().unwrap();
        let pointer = block.words.as_mut_ptr().add(block.used) as u32;
        block.used += words;
        pointer
    }
}

pub unsafe fn freeze() {
    unsafe {
        let main = &mut *core::ptr::addr_of_mut!(MAIN);
        main.append(&mut *core::ptr::addr_of_mut!(WORK));
        main.sort_unstable_by_key(|block| block.words.as_ptr() as usize);
        FROZEN = true;
    }
}

pub unsafe fn is_frozen(pointer: u32) -> bool {
    unsafe {
        if pointer < STATIC_END { return true; }
        let main = &*core::ptr::addr_of!(MAIN);
        let index = main.partition_point(|block| (block.words.as_ptr() as u32) <= pointer);
        index != 0 && {
            let block = &main[index - 1];
            (pointer as u64) < block.words.as_ptr() as u64 + (block.used as u64 * 8)
        }
    }
}

pub unsafe fn take_work() -> Vec<Block> {
    unsafe {
        assert!(*core::ptr::addr_of!(FROZEN));
        core::mem::take(&mut *core::ptr::addr_of_mut!(WORK))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_bytes() -> u32 {
    unsafe {
        (&*core::ptr::addr_of!(MAIN)).iter()
            .chain((&*core::ptr::addr_of!(WORK)).iter())
            .map(|block| block.used as u64 * 8).sum::<u64>().try_into().unwrap()
    }
}
