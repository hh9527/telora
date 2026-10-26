//! Scope-owned UTF-8 metadata. No language values or GC roots.
use alloc::{collections::BTreeMap, string::String, vec::Vec};
static mut OPEN: bool = false;
static mut ENTRIES: BTreeMap<String, String> = BTreeMap::new();
pub(crate) unsafe fn open() { unsafe { OPEN = true; } }
pub(crate) unsafe fn close() { unsafe { OPEN = false; } }
pub(crate) struct Close;
impl Drop for Close { fn drop(&mut self) { unsafe { close(); } } }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_metadata_publish(name: u32, value: u32) -> u32 {
    unsafe {
        if !OPEN { return 0; }
        let name = crate::text::text(name);
        let entries = &mut *core::ptr::addr_of_mut!(ENTRIES);
        if name.is_empty() || entries.contains_key(name) { return 0; }
        entries.insert(String::from(name), String::from(crate::text::text(value)));
        1
    }
}
/// Transfer: entry count followed by UTF-8 name/JSON length-prefixed byte pairs.
/// Result owns (pointer,length), with exact capacity and alignment one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_metadata_export(result: u32) {
    unsafe {
        assert!(!OPEN, "Service metadata initialization is still open");
        crate::host_memory::range(result, 8, 4);
        let entries = core::mem::take(&mut *core::ptr::addr_of_mut!(ENTRIES));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
        for (name, value) in entries {
            for field in [name, value] {
                bytes.extend_from_slice(&u32::try_from(field.len()).unwrap().to_le_bytes());
                bytes.extend_from_slice(field.as_bytes());
            }
        }
        let length = u32::try_from(bytes.len()).unwrap();
        let pointer = crate::host_memory::alloc(length, 1);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len());
        (result as *mut u32).write(pointer);
        (result as *mut u32).add(1).write(length);
    }
}
