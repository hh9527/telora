//! Sort opaque key/value pointer pairs. Value types are a codegen concern.
#[repr(C)]
struct Pair {
    key: u32,
    value: u32,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_sort_pairs(pointer: u32, count: u32) -> u32 {
    unsafe {
        let pairs = core::slice::from_raw_parts_mut(crate::heap::ptr::<Pair>(pointer), count as usize);
        pairs.sort_unstable_by(|a, b| crate::values::telora_string_compare(a.key, b.key).cmp(&0));
        for pair in pairs.windows(2) {
            if crate::values::telora_string_compare(pair[0].key, pair[1].key) == 0 {
                return pair[1].key;
            }
        }
        0
    }
}

/// Private builder index hash; collisions are resolved by full key comparison.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_dict_key_hash(value: u32) -> u32 {
    unsafe {
        let (pointer, length) = crate::values::string_span(value);
        let mut hash = 2166136261u32;
        for index in 0..length {
            hash = (hash ^ u32::from(*((pointer + index) as *const u8))).wrapping_mul(16777619);
        }
        hash
    }
}
