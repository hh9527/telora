//! Stable Guest addresses backed by the default allocator. Continuous words are
//! used only while packing or reading an object image, never for dynamic storage.
use crate::abi::{WORDS_ORIGIN, WORDS_VIEW};
use std::vec::Vec;
use telora_wasm_shared::direct_heap::Heap;
static mut HEAP_LIMIT: u32 = 32 * 1024 * 1024;
static mut MEMORY: Option<Memory> = None;
static mut ORIGIN: u32 = 0;
static mut ALLOCATION_CLOCK: u64 = 0;
static mut PACKING: bool = false;
static mut PACK_SOURCE: Option<Memory> = None;
static mut ALLOCATION_FAILURE: [u32; 6] = [0; 6];

unsafe fn allocation_begin(bytes: u32) {
    unsafe {
        let (live, committed) = usage();
        ALLOCATION_FAILURE = [if packing() { 2 } else { 1 }, 0, bytes, HEAP_LIMIT,
            live.min(u32::MAX as u64) as u32, committed.min(u32::MAX as u64) as u32];
        (crate::abi::ALLOCATION_FAILURE as *mut u32)
            .write(core::ptr::addr_of!(ALLOCATION_FAILURE) as u32);
    }
}

unsafe fn allocation_failed(error: telora_wasm_shared::direct_heap::Error) -> ! {
    unsafe {
        ALLOCATION_FAILURE[1] = match error {
            telora_wasm_shared::direct_heap::Error::Overflow => 1,
            telora_wasm_shared::direct_heap::Error::Limit => 2,
            telora_wasm_shared::direct_heap::Error::Allocation => 3,
            telora_wasm_shared::direct_heap::Error::InvalidReference => 4,
        };
    }
    panic!("managed allocation failed")
}

enum Memory {
    Dynamic(Heap),
    Image(Vec<u64>),
}
unsafe fn memory() -> &'static mut Memory {
    unsafe {
        (&mut *core::ptr::addr_of_mut!(MEMORY))
            .as_mut()
            .expect("heap not configured")
    }
}
unsafe fn replace(next: Memory) -> Memory {
    unsafe {
        (&mut *core::ptr::addr_of_mut!(MEMORY))
            .replace(next)
            .expect("heap not configured")
    }
}
unsafe fn usage() -> (u64, u64) {
    unsafe {
        match memory() {
            Memory::Dynamic(heap) => {
                let usage = heap.usage();
                (
                    usage.allocated as u64,
                    (usage.storage + usage.metadata) as u64,
                )
            }
            Memory::Image(words) => ((words.len() * 8) as u64, (words.capacity() * 8) as u64),
        }
    }
}
unsafe fn publish() {
    unsafe {
        // ABI telemetry only: dynamic references themselves are Guest addresses.
        (WORDS_VIEW as *mut u32).write(0);
        ((WORDS_VIEW + 4) as *mut u32).write(usage().0.try_into().unwrap());
    }
}
pub unsafe fn set_static_end(end: u32) {
    unsafe {
        assert_eq!(*core::ptr::addr_of!(ORIGIN), 0);
        ORIGIN = end;
        MEMORY = Some(Memory::Dynamic(Heap::new(HEAP_LIMIT as usize)));
        (WORDS_ORIGIN as *mut u32).write(end);
        publish();
    }
}
pub unsafe fn allocate(bytes: u32) -> u32 {
    unsafe { allocate_tagged(bytes, 0) }
}
pub(crate) unsafe fn allocate_tagged(bytes: u32, descriptor: u32) -> u32 {
    unsafe {
        allocation_begin(bytes);
        let reference = match memory() {
            Memory::Dynamic(heap) => heap
                .allocate(bytes as usize, descriptor)
                .unwrap_or_else(|error| allocation_failed(error)) as u32,
            Memory::Image(words) => {
                let start = words.len();
                let count = (bytes.max(1) as usize).div_ceil(8);
                let end = start.checked_add(count).unwrap_or_else(||
                    allocation_failed(telora_wasm_shared::direct_heap::Error::Overflow));
                if end as u64 * 8 > u64::from(HEAP_LIMIT) {
                    allocation_failed(telora_wasm_shared::direct_heap::Error::Limit);
                }
                words
                    .try_reserve_exact(count)
                    .unwrap_or_else(|_| allocation_failed(telora_wasm_shared::direct_heap::Error::Allocation));
                words.resize(end, 0);
                ORIGIN
                    .checked_add(u32::try_from(start * 8).unwrap())
                    .unwrap()
            }
        };
        // Scratch retains its explicit high-bit namespace in this ABI. Reject a
        // conflicting physical allocation instead of decoding it as scratch.
        if reference.checked_add(bytes.max(1)).is_none_or(|end|
            end >= telora_wasm_shared::frame_scratch::BASE) {
            allocation_failed(telora_wasm_shared::direct_heap::Error::Overflow);
        }
        ALLOCATION_CLOCK = ALLOCATION_CLOCK
            .checked_add(u64::from(bytes.max(1)))
            .unwrap();
        publish();
        ALLOCATION_FAILURE[0] = 0;
        reference
    }
}
pub(crate) unsafe fn allocation_clock() -> u64 {
    unsafe { ALLOCATION_CLOCK }
}
pub(crate) unsafe fn collection_interval(minimum: u32) -> u64 {
    unsafe {
        let (live, committed) = usage();
        let headroom = u64::from(HEAP_LIMIT).saturating_sub(committed);
        // Repeated full collections cannot recover registry capacity or a live
        // graph. Keep a finite allocation interval under pressure rather than
        // scanning that same graph at every tiny allocation until fuel expires.
        let floor = u64::from(minimum).min(u64::from(HEAP_LIMIT) / 16).max(1);
        u64::from(minimum)
            .max(live / 2)
            .min((headroom / 4).max(floor))
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_heap_limit(bytes: u32) -> u32 {
    unsafe {
        if let Some(memory) = (&mut *core::ptr::addr_of_mut!(MEMORY)).as_mut() {
            match memory {
                Memory::Dynamic(heap) => {
                    if heap.set_limit(bytes as usize).is_err() {
                        return 1;
                    }
                }
                Memory::Image(words) if words.capacity() as u64 * 8 > u64::from(bytes) => return 1,
                _ => {}
            }
        }
        HEAP_LIMIT = bytes;
    }
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_gc_allocated_total() -> u64 {
    unsafe { ALLOCATION_CLOCK }
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
        match memory() {
            Memory::Dynamic(_) => reference,
            Memory::Image(words) => {
                let offset = (reference - ORIGIN) as usize;
                assert!(offset <= words.len() * 8, "image reference outside storage");
                (words.as_ptr() as u32).checked_add(offset as u32).unwrap()
            }
        }
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
    }
    to
}
pub(crate) unsafe fn reset() {
    unsafe {
        crate::frame_scratch::reset();
    }
}
pub unsafe fn is_static(reference: u32) -> bool {
    unsafe { reference < ORIGIN }
}

pub(crate) struct SourceHeap {
    memory: Memory,
    origin: u32,
}
impl SourceHeap {
    pub fn ptr<T>(&self, reference: u32) -> *const T {
        self.span(reference, core::mem::size_of::<T>());
        if reference < self.origin {
            return reference as *const T;
        }
        match &self.memory {
            Memory::Dynamic(_) => reference as *const T,
            Memory::Image(words) => {
                let offset = (reference - self.origin) as usize;
                assert!(
                    offset < words.len() * 8,
                    "source image reference outside storage"
                );
                unsafe { words.as_ptr().cast::<u8>().add(offset).cast() }
            }
        }
    }
    pub fn span(&self, reference: u32, bytes: usize) {
        if reference < self.origin {
            assert!(
                (reference as usize).checked_add(bytes).unwrap() <= self.origin as usize,
                "static source extent overflow"
            );
        } else if let Memory::Image(words) = &self.memory {
            let offset = (reference - self.origin) as usize;
            assert!(
                offset.checked_add(bytes).unwrap() <= words.len() * 8,
                "source image extent overflow"
            );
        }
    }
    pub unsafe fn read<T: Copy>(&self, reference: u32) -> T {
        self.span(reference, core::mem::size_of::<T>());
        unsafe { self.ptr::<T>(reference).read_unaligned() }
    }
}
pub unsafe fn take_source() -> SourceHeap {
    unsafe {
        let next = if PACKING {
            Memory::Image(Vec::new())
        } else {
            Memory::Dynamic(Heap::new(HEAP_LIMIT as usize))
        };
        let memory = replace(next);
        publish();
        SourceHeap {
            memory,
            origin: ORIGIN,
        }
    }
}
pub(crate) unsafe fn packing() -> bool {
    unsafe { PACKING }
}
pub(crate) unsafe fn start_packing() {
    unsafe {
        assert!(!PACKING);
        PACKING = true;
    }
}
pub(crate) unsafe fn retain_source(source: SourceHeap) {
    unsafe {
        assert!(PACKING);
        assert!((*core::ptr::addr_of!(PACK_SOURCE)).is_none());
        PACK_SOURCE = Some(source.memory);
    }
}
pub(crate) unsafe fn finish_packing() {
    unsafe {
        assert!(PACKING);
        let source = (&mut *core::ptr::addr_of_mut!(PACK_SOURCE)).take().unwrap();
        drop(replace(source));
        PACKING = false;
        publish();
    }
}
pub(crate) unsafe fn snapshot() -> (u32, u32, Vec<u8>) {
    unsafe {
        let Memory::Image(words) = memory() else {
            panic!("snapshot requires image packing");
        };
        let mut bytes =
            core::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 8).to_vec();
        // Rust Regex pointers are reconstructed from pattern descriptions and
        // must never leak into the portable, deterministic object image.
        let table = crate::tables::snapshot()[crate::abi::REGEXES as usize];
        for id in 0..table.length {
            let offset = (table.buffer - ORIGIN + id * 8) as usize;
            bytes[offset..offset + 4].fill(0);
        }
        (ORIGIN, u32::try_from(words.len() * 8).unwrap(), bytes)
    }
}
pub(crate) unsafe fn restore(origin: u32, allocated: u32, bytes: &[u8]) {
    unsafe {
        restore_data(origin, allocated, bytes);
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
        assert_eq!(allocated as usize, bytes.len(), "snapshot extent mismatch");
        assert!(
            bytes.len() as u64 <= u64::from(HEAP_LIMIT),
            "snapshot budget"
        );
        let mut words = vec![0u64; bytes.len() / 8];
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), words.as_mut_ptr().cast(), bytes.len());
        drop(replace(Memory::Image(words)));
        publish();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_bytes() -> u32 {
    unsafe { usage().0.try_into().unwrap() }
}
pub(crate) unsafe fn sweep(references: impl IntoIterator<Item = u32>) {
    unsafe {
        let Memory::Dynamic(heap) = memory() else {
            panic!("GC during image packing");
        };
        for reference in references {
            if reference >= ORIGIN && reference < telora_wasm_shared::frame_scratch::BASE {
                heap.mark(reference as usize)
                    .expect("root outside live managed allocation");
            }
        }
        heap.sweep(|block| {
            if block.descriptor == crate::abi::CONTENT_VIEW {
                crate::content::release(block.bytes.try_into().unwrap());
            }
        });
        publish();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_heap_committed() -> u32 {
    unsafe { usage().1.try_into().unwrap() }
}
