//! Single-test process for requested-allocation observations, not RSS accounting.
//! Realloc records net requested growth, not transient allocator copying overhead.
use regex_automata::{Input, nfa::thompson::pikevm::PikeVM};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::SeqCst};

struct ObservedAllocator;
static ENABLED: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
fn change(bytes: isize) {
    if ENABLED.load(SeqCst) {
        let live = LIVE.fetch_add(bytes, SeqCst) + bytes;
        PEAK.fetch_max(live, SeqCst);
    }
}
unsafe impl GlobalAlloc for ObservedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            change(layout.size() as isize);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            change(layout.size() as isize);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        change(-(layout.size() as isize));
        unsafe {
            System.dealloc(pointer, layout);
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(pointer, layout, size) };
        if !next.is_null() {
            change(size as isize - layout.size() as isize);
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: ObservedAllocator = ObservedAllocator;

#[test]
fn observe_pikevm_retained_and_requested_live_allocations() {
    for pattern in [r"(?P<word>\w+)", r"(?:a?){32}a{32}", r"(?P<a>a*)(?P<b>b*)"] {
        let regex = PikeVM::new(pattern).unwrap();
        let mut captures = regex.create_captures();
        let input = "a".repeat(65_536);
        LIVE.store(0, SeqCst);
        PEAK.store(0, SeqCst);
        ENABLED.store(true, SeqCst);
        let mut cache = regex.create_cache();
        let initial = LIVE.load(SeqCst);
        regex.search(&mut cache, &Input::new(&input), &mut captures);
        let retained = LIVE.load(SeqCst);
        let peak = PEAK.load(SeqCst);
        let reported = cache.memory_usage();
        drop(cache);
        let after_drop = LIVE.load(SeqCst);
        ENABLED.store(false, SeqCst);
        eprintln!(
            "pattern={pattern:?} initial={initial} retained={retained} requested_live_high_water={peak} reported={reported} after_drop={after_drop}"
        );
        assert!(peak >= retained);
        assert!(
            retained > reported as isize,
            "epsilon stack capacity should be observable for this pattern"
        );
        assert_eq!(
            after_drop, 0,
            "measurement window must release all its allocations"
        );
    }
}
