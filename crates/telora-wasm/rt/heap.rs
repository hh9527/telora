//! Paged allocation metadata over a contiguous word arena. Language references are byte offsets biased above the
//! immutable Wasm image, never allocation addresses. Host buffers are separate.
use crate::abi::{WORDS_ORIGIN, WORDS_VIEW};
use std::vec::Vec;
use telora_wasm_shared::gc_heap::Heap;
static mut HEAP_LIMIT: u64 = 32 * 1024 * 1024;
static mut HEAP: Option<Heap> = None;

unsafe fn heap() -> &'static mut Heap {
    unsafe {
        (&mut *core::ptr::addr_of_mut!(HEAP))
            .as_mut()
            .expect("heap not configured")
    }
}

unsafe fn replace_words(words: Vec<u64>) -> Vec<u64> {
    unsafe {
        let next = Heap::from_words(ORIGIN, HEAP_LIMIT, words).expect("heap image budget");
        (&mut *core::ptr::addr_of_mut!(HEAP))
            .replace(next)
            .expect("heap not configured")
            .into_words()
    }
}
static mut ORIGIN: u32 = 0;
static mut WORK_BASE: Option<usize> = None;
static mut BASE_ALLOCATED: u64 = 0;
static mut ALLOCATION_CLOCK: u64 = 0;

unsafe fn publish() {
    unsafe {
        let words = heap().words();
        (WORDS_VIEW as *mut u32).write(words.as_ptr() as u32);
        ((WORDS_VIEW + 4) as *mut u32).write((words.len() * 8) as u32);
    }
}
pub unsafe fn set_static_end(end: u32) {
    unsafe {
        assert_eq!(*core::ptr::addr_of!(ORIGIN), 0);
        ORIGIN = end;
        HEAP = Some(Heap::new(end, HEAP_LIMIT).expect("heap origin"));
        (WORDS_ORIGIN as *mut u32).write(end);
        publish();
    }
}
pub unsafe fn allocate(bytes: u32) -> u32 {
    unsafe {
        let reference = heap().allocate(bytes, 0).expect("managed heap budget");
        ALLOCATION_CLOCK = ALLOCATION_CLOCK
            .checked_add(u64::from(bytes.max(1)))
            .expect("allocation clock exhausted");
        assert!(
            reference
                .checked_add(bytes.max(1))
                .expect("language heap exceeds wasm32")
                < telora_wasm_shared::frame_scratch::BASE
        );
        publish();
        reference
    }
}

pub(crate) unsafe fn allocation_clock() -> u64 {
    unsafe { *core::ptr::addr_of!(ALLOCATION_CLOCK) }
}

/// Amortize tracing a large live graph while keeping room for allocation slack.
/// The engine separately bounds content, marker worklists and Rust resources.
pub(crate) unsafe fn collection_interval(minimum: u32) -> u64 {
    unsafe {
        let usage = heap().usage();
        let live = BASE_ALLOCATED + usage.allocated;
        let headroom = HEAP_LIMIT.saturating_sub(live.saturating_add(usage.metadata));
        u64::from(minimum).max(u64::from(telora_heap_bytes()) / 2)
            .min((headroom / 4).max(1))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_heap_limit(bytes: u32) -> u32 {
    unsafe {
        if let Some(heap) = (&mut *core::ptr::addr_of_mut!(HEAP)).as_mut() {
            if heap.set_limit(u64::from(bytes)).is_err() { return 1; }
        }
        HEAP_LIMIT = u64::from(bytes);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_allocated_total() -> u64 {
    unsafe { allocation_clock() }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_address(reference: u32) -> u32 {
    unsafe {
        if reference >= telora_wasm_shared::frame_scratch::BASE {
            return crate::frame_scratch::address(reference);
        }
        if reference < ORIGIN {
            return reference;
        }
        let offset = reference - ORIGIN;
        let words = heap().words();
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
    unsafe {
        ptr::<T>(reference).write_unaligned(value);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_copy(to: u32, from: u32, bytes: u32) -> u32 {
    unsafe {
        if bytes != 0 {
            core::ptr::copy(ptr::<u8>(from), ptr::<u8>(to), bytes as usize);
        }
        to
    }
}
pub unsafe fn freeze() {
    unsafe {
        assert!((*core::ptr::addr_of!(WORK_BASE)).is_none());
        BASE_ALLOCATED += heap().usage().allocated;
        WORK_BASE = Some(heap().words().len());
        let words = (&mut *core::ptr::addr_of_mut!(HEAP))
            .take()
            .unwrap()
            .into_words();
        HEAP = Some(Heap::from_words(ORIGIN, HEAP_LIMIT, words).expect("frozen heap budget"));
    }
}

pub(crate) unsafe fn frozen() -> bool {
    unsafe { (*core::ptr::addr_of!(WORK_BASE)).is_some() }
}
pub(crate) unsafe fn reset() {
    unsafe {
        crate::frame_scratch::reset();
        let base = (*core::ptr::addr_of!(WORK_BASE)).expect("heap not frozen");
        let mut words = (&mut *core::ptr::addr_of_mut!(HEAP))
            .take()
            .unwrap()
            .into_words();
        words.truncate(base);
        HEAP = Some(Heap::from_words(ORIGIN, HEAP_LIMIT, words).expect("reset heap budget"));
        publish();
    }
}
pub unsafe fn is_frozen(reference: u32) -> bool {
    unsafe {
        reference < ORIGIN
            || (*core::ptr::addr_of!(WORK_BASE))
                .is_some_and(|base| ((reference - ORIGIN) as usize) < base * 8)
    }
}
pub(crate) struct OldWords {
    words: Vec<u64>,
    origin: u32,
}
impl OldWords {
    pub fn ptr<T>(&self, reference: u32) -> *const T {
        if reference < self.origin {
            return reference as *const T;
        }
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
        let prefix = heap().words()[..base].to_vec();
        let old = OldWords {
            words: replace_words(prefix),
            origin: ORIGIN,
        };
        publish();
        old
    }
}
pub unsafe fn take_initialization() -> OldWords {
    unsafe {
        assert!((*core::ptr::addr_of!(WORK_BASE)).is_none());
        // All immutable tracing/reflection metadata lives in the static image;
        // the fixed words prefix is therefore empty in this implementation.
        let words = replace_words(Vec::new());
        BASE_ALLOCATED = 0;
        publish();
        OldWords {
            words,
            origin: ORIGIN,
        }
    }
}

pub(crate) unsafe fn snapshot() -> (u32, u32, Vec<u8>) {
    unsafe {
        let words = heap().words();
        (
            ORIGIN,
            u32::try_from(BASE_ALLOCATED + heap().usage().allocated).unwrap(),
            core::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 8).to_vec(),
        )
    }
}

pub(crate) unsafe fn restore(origin: u32, allocated: u32, bytes: &[u8]) {
    unsafe {
        restore_data(origin, allocated, bytes);
        WORK_BASE = Some(bytes.len() / 8);
    }
}

pub(crate) unsafe fn restore_data(origin: u32, allocated: u32, bytes: &[u8]) {
    unsafe {
        assert_eq!(
            origin,
            *core::ptr::addr_of!(ORIGIN),
            "snapshot static image mismatch"
        );
        assert_eq!(bytes.len() % 8, 0, "unaligned snapshot heap");
        assert!((*core::ptr::addr_of!(WORK_BASE)).is_none());
        let mut words = vec![0u64; bytes.len() / 8];
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), words.as_mut_ptr().cast(), bytes.len());
        replace_words(words);
        BASE_ALLOCATED = u64::from(allocated);
        publish();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_bytes() -> u32 {
    unsafe {
        ((BASE_ALLOCATED + heap().usage().allocated) as usize)
            .checked_add(crate::content::len())
            .unwrap()
            .try_into()
            .unwrap()
    }
}

pub(crate) unsafe fn sweep(references: impl IntoIterator<Item = u32>) {
    unsafe {
        for reference in references {
            if reference < telora_wasm_shared::frame_scratch::BASE && !heap().is_image(reference) {
                heap()
                    .mark(reference)
                    .expect("root outside live managed allocation");
            }
        }
        heap().sweep(|_| {});
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_committed() -> u32 {
    unsafe { u32::try_from(heap().usage().committed + heap().usage().metadata).unwrap() }
}
