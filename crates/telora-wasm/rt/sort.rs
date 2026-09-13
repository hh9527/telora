//! Sort opaque key/value pointer pairs. Value types are a codegen concern.
#[repr(C)]
struct Pair {
    key: u32,
    value: u32,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_sort_pairs(pointer: u32, count: u32) -> u32 {
    unsafe {
        let pairs = core::slice::from_raw_parts_mut(pointer as *mut Pair, count as usize);
        pairs.sort_unstable_by(|a, b| crate::values::telora_string_compare(a.key, b.key).cmp(&0));
        for pair in pairs.windows(2) {
            if crate::values::telora_string_compare(pair[0].key, pair[1].key) == 0 {
                return pair[1].key;
            }
        }
        0
    }
}
