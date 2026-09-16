//! Source names are input metadata, not language values or type information.
use crate::telora_alloc;
mod registry;
unsafe fn raw_word(pointer: u32, offset: u32) -> u32 {
    unsafe { ((pointer + offset) as *const u32).read_unaligned() }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Source {
    id: u32,
    pointer: u32,
    length: u32,
    lines: u32,
    line_count: u32,
}

pub(crate) unsafe fn freeze() {
    unsafe {
        registry::freeze();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_source_retained(id: u32) -> u32 {
    unsafe {
        for source in registry::records() {
            if source.id == id {
                return 1;
            }
        }
        0
    }
}

pub(crate) unsafe fn collect(gc: &mut crate::collect::Collector) {
    unsafe {
        registry::collect(&gc.sources);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_register_source(id: u32, pointer: u32, length: u32) -> u32 {
    unsafe {
        assert_ne!(id, 0, "zero is not a source identity");
        registry::register(id, core::slice::from_raw_parts(pointer as *const u8, length as usize))
    }
}

/// Borrow an index during registration; retain our own immutable copy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_source_index(id: u32, pointer: u32, count: u32) {
    unsafe { register_index(id, pointer, count, false); }
}

/// Internal linker bootstrap: the index lives in the artifact's static segment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_static_source_index(id: u32, pointer: u32, count: u32) {
    unsafe { register_index(id, pointer, count, true); }
}

unsafe fn register_index(id: u32, pointer: u32, count: u32, static_storage: bool) {
    unsafe {
        assert_ne!(count, 0);
        let bytes = count.checked_mul(8).unwrap();
        assert!(pointer.checked_add(bytes).unwrap() <= crate::telora_heap_end());
        let mut previous_end = 0;
        for index in 0..count {
            let start = raw_word(pointer + index * 8, 0);
            let end = raw_word(pointer + index * 8, 4);
            assert!(start <= end);
            if index == 0 { assert_eq!(start, 0); }
            else { assert!(start > previous_end && start - previous_end <= 2); }
            previous_end = end;
        }
        registry::index(id, pointer, count, static_storage);
    }
}

/// Expand only when emitting diagnostics; ordinary values carry byte offsets.
pub(crate) unsafe fn position(id: u32, byte: u32) -> (u32, u32) {
    unsafe {
        for source in registry::records() {
            if source.id != id { continue; }
            assert_ne!(source.line_count, 0, "source lacks index");
            assert!(byte <= raw_word(source.lines + (source.line_count - 1) * 8, 4));
            let mut lo = 0;
            let mut hi = source.line_count;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if raw_word(source.lines + mid * 8, 0) <= byte { lo = mid + 1; }
                else { hi = mid; }
            }
            let line = lo - 1;
            let start = raw_word(source.lines + line * 8, 0);
            let end = raw_word(source.lines + line * 8, 4);
            return (line, byte.min(end) - start);
        }
        panic!("unregistered source");
    }
}

/// Diagnostic-only expansion of an inline range into five coordinate words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_source_range(range: u32) -> u32 {
    unsafe {
        let id = crate::values::word(range, 0);
        let start = crate::values::word(range, 4);
        let end = crate::values::word(range, 8);
        if id == 0 {
            assert_eq!((start, end), (0, 0), "invalid empty source range");
            return 0;
        }
        assert!(start <= end);
        let start = position(id, start);
        let end = position(id, end);
        let result = telora_alloc(20);
        for (index, word) in [id, start.0, start.1, end.0, end.1].into_iter().enumerate() {
            crate::heap::write(result + index as u32 * 4, word);
        }
        result
    }
}

/// Returns an address of two u32 words: UTF-8 pointer and byte length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_source_name(id: u32) -> u32 {
    unsafe {
        for item in registry::records() {
            if item.id == id {
                let result = telora_alloc(8 + item.length);
                crate::heap::write(result, result + 8);
                crate::heap::write(result + 4, item.length);
                core::ptr::copy_nonoverlapping(item.pointer as *const u8, crate::heap::ptr::<u8>(result + 8), item.length as usize);
                return result;
            }
        }
        panic!("unregistered source");
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_subject_label(index: u32) -> u32 {
    unsafe { number_text(b"subject ", index, b" originated here") }
}

unsafe fn number_text(prefix: &[u8], mut number: u32, suffix: &[u8]) -> u32 {
    unsafe {
        let mut digits = [0u8; 10];
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (number % 10) as u8;
            number /= 10;
            if number == 0 {
                break;
            }
        }
        let length = (prefix.len() + digits.len() - at + suffix.len()) as u32;
        let result = telora_alloc(8 + length);
        crate::heap::write(result, result + 8);
        crate::heap::write(result + 4, length);
        core::ptr::copy_nonoverlapping(prefix.as_ptr(), crate::heap::ptr::<u8>(result + 8), prefix.len());
        core::ptr::copy_nonoverlapping(
            digits[at..].as_ptr(),
            crate::heap::ptr::<u8>(result + 8 + prefix.len() as u32),
            digits.len() - at,
        );
        core::ptr::copy_nonoverlapping(
            suffix.as_ptr(),
            crate::heap::ptr::<u8>(result + 8 + prefix.len() as u32 + (digits.len() - at) as u32),
            suffix.len(),
        );
        result
    }
}

/// Allocate from the Guest registry, after the deterministic static source list.
pub(crate) unsafe fn next_id() -> u32 {
    unsafe {
        let mut highest = 0;
        for source in registry::records() {
            highest = highest.max(source.id);
        }
        highest.checked_add(1).expect("source identity overflow")
    }
}
