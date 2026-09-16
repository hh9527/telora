//! One immutable byte arena. Payloads retain offsets, never its moving address.
use crate::abi::CONTENT_VIEW;
use telora_wasm_shared::arena::content::{Bytes, Content, LiveRanges};

static mut CONTENT: Content = Content::new();

pub(crate) unsafe fn read(payload: u32) -> Bytes {
    unsafe { Bytes::decode(crate::heap::read(payload)).unwrap() }
}

pub(crate) unsafe fn write(payload: u32, bytes: Bytes) {
    unsafe { crate::heap::write(payload, bytes.encode().unwrap()); }
}

unsafe fn publish() {
    unsafe {
        let bytes = (&*core::ptr::addr_of!(CONTENT)).bytes();
        (CONTENT_VIEW as *mut u32).write(bytes.as_ptr() as u32);
        ((CONTENT_VIEW + 4) as *mut u32).write(bytes.len() as u32);
    }
}

/// Only borrow until the next content append or collection.
pub(crate) unsafe fn span(payload: u32) -> (u32, u32) {
    unsafe {
        match read(payload) {
            Bytes::Inline { len, .. } => (crate::heap::telora_heap_address(payload), u32::from(len)),
            value @ Bytes::Slice(_) => {
                let view = (&*core::ptr::addr_of!(CONTENT)).view(&value).unwrap();
                (view.as_ptr() as u32, view.len() as u32)
            }
        }
    }
}

/// Import external storage. Shared language content uses relative slicing instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_content_write(destination: u32, pointer: u32, length: u32) -> u32 {
    unsafe {
        let pointer = crate::heap::telora_heap_address(pointer);
        // Appending must not invalidate the input borrow.
        let bytes = (&*core::ptr::addr_of!(CONTENT)).bytes();
        let base = bytes.as_ptr() as usize;
        assert!(length == 0 || (pointer as usize) < base || pointer as usize >= base + bytes.len());
        let input = core::slice::from_raw_parts(pointer as *const u8, length as usize);
        let value = (&mut *core::ptr::addr_of_mut!(CONTENT)).insert(input).unwrap();
        write(destination, value);
        publish();
        destination
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_content_slice(destination: u32, owner: u32, start: u32, length: u32) -> u32 {
    unsafe {
        let value = (&*core::ptr::addr_of!(CONTENT)).slice(
            &read(owner), start, start.checked_add(length).unwrap()).unwrap();
        write(destination, value);
        destination
    }
}

pub(crate) unsafe fn freeze() {
    unsafe { (&mut *core::ptr::addr_of_mut!(CONTENT)).seal_work().unwrap(); }
}
pub(crate) unsafe fn reset() {
    unsafe {
        (&mut *core::ptr::addr_of_mut!(CONTENT)).reset().unwrap();
        publish();
    }
}

/// Discovery is completed before any copying or payload updates.
pub(crate) unsafe fn collect(patches: &[u32], initialization: bool) {
    unsafe {
        let source = &*core::ptr::addr_of!(CONTENT);
        let mut ranges = LiveRanges::default();
        for &payload in patches { ranges.observe(source, &read(payload)).unwrap(); }
        let (next, relocation) = if initialization { ranges.copy(source) }
            else { ranges.copy_work(source) }.unwrap();
        for &payload in patches { write(payload, relocation.apply(read(payload)).unwrap()); }
        *core::ptr::addr_of_mut!(CONTENT) = next;
        publish();
    }
}

pub(crate) unsafe fn len() -> usize {
    unsafe { (&*core::ptr::addr_of!(CONTENT)).len() }
}
