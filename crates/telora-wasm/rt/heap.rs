//! Single word arena. Language references are byte offsets biased above the
//! immutable Wasm image, never allocation addresses. Host buffers are separate.
use std::vec::Vec;
use crate::abi::{WORDS_VIEW, WORDS_ORIGIN};
static mut WORDS: Vec<u64> = Vec::new();
static mut ORIGIN: u32 = 0;
static mut WORK_BASE: Option<usize> = None;

unsafe fn publish() {
    unsafe {
        let words = &*core::ptr::addr_of!(WORDS);
        (WORDS_VIEW as *mut u32).write(words.as_ptr() as u32);
        ((WORDS_VIEW + 4) as *mut u32).write((words.len() * 8) as u32);
    }
}
pub unsafe fn set_static_end(end: u32) {
    unsafe {
        assert_eq!(*core::ptr::addr_of!(ORIGIN), 0);
        ORIGIN = end;
        (WORDS_ORIGIN as *mut u32).write(end);
        publish();
    }
}
pub unsafe fn allocate(bytes: u32) -> u32 {
    unsafe {
        let words = &mut *core::ptr::addr_of_mut!(WORDS);
        let start = words.len();
        let end = start.checked_add((bytes.max(1) as usize).div_ceil(8)).unwrap();
        let offset = u32::try_from(end.checked_mul(8).unwrap()).unwrap();
        ORIGIN.checked_add(offset).expect("language heap exceeds wasm32");
        words.resize(end, 0);
        publish();
        ORIGIN + (start * 8) as u32
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_address(reference: u32) -> u32 {
    unsafe {
        if reference < ORIGIN { return reference; }
        let offset = reference - ORIGIN;
        let words = &*core::ptr::addr_of!(WORDS);
        assert!((offset as usize) <= words.len() * 8);
        (words.as_ptr() as u32).checked_add(offset).unwrap()
    }
}
pub unsafe fn ptr<T>(reference: u32) -> *mut T {
    unsafe { telora_heap_address(reference) as *mut T }
}
pub unsafe fn read<T: Copy>(reference: u32) -> T {
    unsafe { ptr::<T>(reference).read_unaligned() }
}
pub unsafe fn write<T>(reference: u32, value: T) {
    unsafe { ptr::<T>(reference).write_unaligned(value); }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_copy(to: u32, from: u32, bytes: u32) -> u32 {
    unsafe {
        if bytes != 0 { core::ptr::copy(ptr::<u8>(from), ptr::<u8>(to), bytes as usize); }
        to
    }
}
pub unsafe fn freeze() {
    unsafe {
        assert!((*core::ptr::addr_of!(WORK_BASE)).is_none());
        WORK_BASE = Some((&*core::ptr::addr_of!(WORDS)).len());
    }
}
pub(crate) unsafe fn reset() {
    unsafe {
        let base = (*core::ptr::addr_of!(WORK_BASE)).expect("heap not frozen");
        (&mut *core::ptr::addr_of_mut!(WORDS)).truncate(base);
        publish();
    }
}
pub unsafe fn is_frozen(reference: u32) -> bool {
    unsafe {
        reference < ORIGIN || (*core::ptr::addr_of!(WORK_BASE))
            .is_some_and(|base| ((reference - ORIGIN) as usize) < base * 8)
    }
}
pub(crate) struct OldWords { words: Vec<u64>, origin: u32 }
impl OldWords {
    pub fn ptr<T>(&self, reference: u32) -> *const T {
        if reference < self.origin { return reference as *const T; }
        let offset = (reference - self.origin) as usize;
        assert!(offset < self.words.len() * 8);
        unsafe { self.words.as_ptr().cast::<u8>().add(offset).cast() }
    }
    pub unsafe fn read<T: Copy>(&self, reference: u32) -> T {
        unsafe { self.ptr::<T>(reference).read_unaligned() }
    }
}
pub unsafe fn take_work() -> OldWords {
    unsafe {
        let base = (*core::ptr::addr_of!(WORK_BASE)).expect("heap not frozen");
        let words = &mut *core::ptr::addr_of_mut!(WORDS);
        let prefix = words[..base].to_vec();
        let old = OldWords { words: core::mem::replace(words, prefix), origin: ORIGIN };
        publish();
        old
    }
}
pub unsafe fn take_initialization() -> OldWords {
    unsafe {
        assert!((*core::ptr::addr_of!(WORK_BASE)).is_none());
        // All immutable tracing/reflection metadata lives in the static image;
        // the fixed words prefix is therefore empty in this implementation.
        let words = core::mem::take(&mut *core::ptr::addr_of_mut!(WORDS));
        publish();
        OldWords { words, origin: ORIGIN }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_bytes() -> u32 {
    unsafe {
        ((&*core::ptr::addr_of!(WORDS)).len() * 8)
            .checked_add(crate::content::len()).unwrap().try_into().unwrap()
    }
}
