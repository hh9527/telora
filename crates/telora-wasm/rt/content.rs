//! Immutable byte views over nonmoving, managed RawString allocations.
use crate::abi::CONTENT_VIEW;
use telora_wasm_shared::arena::content::{Bytes, Slice};
static mut CURRENT: u32 = 0;
static mut FROZEN: u32 = 0;

pub(crate) unsafe fn allocate_capacity(length: u32) -> u32 {
    unsafe {
        let bytes = length.checked_add(8).unwrap();
        let raw = crate::heap::allocate_tagged(bytes, CONTENT_VIEW);
        publish_usage(CURRENT.checked_add(bytes).unwrap());
        raw
    }
}
pub(crate) unsafe fn release(bytes: u32) {
    unsafe { publish_usage(CURRENT.checked_sub(bytes).unwrap()); }
}
pub(crate) unsafe fn len() -> u32 { unsafe { CURRENT } }
pub(crate) unsafe fn freeze() { unsafe { FROZEN = CURRENT; } }
pub(crate) unsafe fn reset() { unsafe { publish_usage(FROZEN); } }
pub(crate) unsafe fn begin(initialization: bool) {
    unsafe { publish_usage(if initialization { 0 } else { FROZEN }); }
}
pub(crate) unsafe fn restore(bytes: u32, frozen: bool) {
    unsafe { FROZEN = if frozen { bytes } else { 0 }; publish_usage(bytes); }
}

pub(crate) unsafe fn read(payload: u32) -> Bytes {
    unsafe { Bytes::decode(crate::heap::read(payload)).unwrap() }
}
pub(crate) unsafe fn write(payload: u32, bytes: Bytes) {
    unsafe { crate::heap::write(payload, bytes.encode().unwrap()); }
}

/// Physical spans are borrowed only until the next managed allocation.
pub(crate) unsafe fn span(payload: u32) -> (u32, u32) {
    unsafe {
        match read(payload) {
            Bytes::Inline { len, .. } => (crate::heap::telora_heap_address(payload), u32::from(len)),
            Bytes::Slice(s) => {
                let len: u32 = crate::heap::read(s.raw);
                let cap: u32 = crate::heap::read(s.raw + 4);
                assert!(s.end <= len && len <= cap);
                (crate::heap::telora_heap_address(s.raw + 8 + s.start), s.end - s.start)
            }
        }
    }
}

/// The source is a logical reference: resolve it again after allocating the raw.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_content_write(destination: u32, source: u32, length: u32) -> u32 {
    unsafe {
        let value = if length < 16 {
            let mut data = [0; 15];
            core::ptr::copy_nonoverlapping(crate::heap::ptr::<u8>(source), data.as_mut_ptr(), length as usize);
            Bytes::Inline { data, len: length as u8 }
        } else {
            let raw = allocate_capacity(length);
            crate::heap::write(raw, length);
            crate::heap::write(raw + 4, length);
            core::ptr::copy_nonoverlapping(crate::heap::ptr::<u8>(source), crate::heap::ptr::<u8>(raw + 8), length as usize);
            Bytes::Slice(Slice { raw, start: 0, end: length })
        };
        write(destination, value);
        destination
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_content_slice(destination: u32, owner: u32, start: u32, length: u32) -> u32 {
    unsafe {
        let parent = read(owner);
        let (_, parent_len) = span(owner);
        let end = start.checked_add(length).unwrap();
        assert!(end <= parent_len);
        let value = if length < 16 {
            let (pointer, _) = span(owner);
            let mut data = [0; 15];
            core::ptr::copy_nonoverlapping((pointer + start) as *const u8, data.as_mut_ptr(), length as usize);
            Bytes::Inline { data, len: length as u8 }
        } else {
            let Bytes::Slice(s) = parent else { unreachable!() };
            Bytes::Slice(Slice { raw: s.raw, start: s.start.checked_add(start).unwrap(), end: s.start.checked_add(end).unwrap() })
        };
        write(destination, value);
        destination
    }
}

/// Content is owned by the managed word heap; no separate mirror is serialized.
pub(crate) unsafe fn publish_usage(bytes: u32) {
    unsafe {
        CURRENT = bytes;
        (CONTENT_VIEW as *mut u32).write(0);
        ((CONTENT_VIEW + 4) as *mut u32).write(bytes);
    }
}
