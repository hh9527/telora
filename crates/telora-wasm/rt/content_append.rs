//! Append behind immutable byte views; only unpublished tail bytes may change.
use telora_wasm_shared::arena::content::{Bytes, Slice};

fn length(value: &Bytes) -> u32 {
    match value {
        Bytes::Inline { len, .. } => u32::from(*len),
        Bytes::Slice(s) => s.end - s.start,
    }
}

unsafe fn copy(value: &Bytes, destination: *mut u8) {
    unsafe {
        let source = match value {
            Bytes::Inline { data, .. } => data.as_ptr(),
            Bytes::Slice(s) => crate::heap::ptr::<u8>(s.raw + 8 + s.start),
        };
        core::ptr::copy(source, destination, length(value) as usize);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_content_append(destination: u32, owner: u32, suffix: u32) -> u32 {
    unsafe {
        let owner = crate::content::read(owner);
        let suffix = crate::content::read(suffix);
        let left = length(&owner);
        let right = length(&suffix);
        let length = left.checked_add(right).unwrap();
        let result = if right == 0 { owner }
        else if length < 16 {
            let mut data = [0; 15];
            copy(&owner, data.as_mut_ptr());
            copy(&suffix, data.as_mut_ptr().add(left as usize));
            Bytes::Inline { data, len: length as u8 }
        } else {
            if let Bytes::Slice(s) = owner {
                let end = s.end.checked_add(right).unwrap();
                if !crate::heap::is_frozen(s.raw)
                    && s.end == crate::heap::read::<u32>(s.raw)
                    && end <= crate::heap::read::<u32>(s.raw + 4)
                {
                    copy(&suffix, crate::heap::ptr::<u8>(s.raw + 8 + s.end));
                    crate::heap::write(s.raw, end);
                    crate::content::write(destination, Bytes::Slice(Slice { end, ..s }));
                    return destination;
                }
            }
            let cap = length.checked_next_power_of_two().unwrap();
            let raw = crate::content::allocate_capacity(cap);
            crate::heap::write(raw, length);
            crate::heap::write(raw + 4, cap);
            copy(&owner, crate::heap::ptr::<u8>(raw + 8));
            copy(&suffix, crate::heap::ptr::<u8>(raw + 8 + left));
            Bytes::Slice(Slice { raw, start: 0, end: length })
        };
        crate::content::write(destination, result);
        destination
    }
}
