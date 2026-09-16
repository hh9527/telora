//! Source names are input metadata, not language values or type information.
use crate::telora_alloc;

#[repr(C)]
#[derive(Clone, Copy)]
struct Source {
    id: u32,
    pointer: u32,
    length: u32,
    lines: u32,
    line_count: u32,
}
const SOURCE_BYTES: u32 = core::mem::size_of::<Source>() as u32;
static mut BUFFER: u32 = 0;
static mut LENGTH: u32 = 0;
static mut CAPACITY: u32 = 0;
static mut FROZEN: u32 = 0;

pub(crate) unsafe fn freeze() {
    unsafe {
        FROZEN = LENGTH;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_source_retained(id: u32) -> u32 {
    unsafe {
        for index in 0..LENGTH {
            if (BUFFER as *const Source).add(index as usize).read().id == id {
                return 1;
            }
        }
        0
    }
}

pub(crate) unsafe fn collect(gc: &mut crate::collect::Collector) {
    unsafe {
        let retained = (0..LENGTH)
            .filter(|&index| {
                index < FROZEN
                    || gc
                        .sources
                        .contains(&(BUFFER as *const Source).add(index as usize).read().id)
            })
            .count() as u32;
        let at = gc.reserve(retained * SOURCE_BYTES);
        let mut next = 0;
        for index in 0..LENGTH {
            let source = (BUFFER as *const Source).add(index as usize).read();
            if index >= FROZEN && !gc.sources.contains(&source.id) {
                continue;
            }
            let pointer = if source.pointer < gc.base {
                source.pointer
            } else {
                let offset = gc.copy_bytes(source.pointer, source.length);
                gc.base + offset
            };
            let lines = if source.lines < gc.base { source.lines } else {
                gc.base + gc.copy_bytes(source.lines, source.line_count.checked_mul(8).unwrap())
            };
            gc.put(at + next * SOURCE_BYTES, source.id);
            gc.put(at + next * SOURCE_BYTES + 4, pointer);
            gc.put(at + next * SOURCE_BYTES + 8, source.length);
            gc.put(at + next * SOURCE_BYTES + 12, lines);
            gc.put(at + next * SOURCE_BYTES + 16, source.line_count);
            next += 1;
        }
        BUFFER = gc.base + at;
        LENGTH = retained;
        CAPACITY = retained;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_register_source(id: u32, pointer: u32, length: u32) -> u32 {
    unsafe {
        for index in 0..LENGTH {
            let old = (BUFFER as *const Source).add(index as usize).read();
            if old.id == id {
                if old.length != length {
                    return 0;
                }
                for offset in 0..length {
                    if *((old.pointer + offset) as *const u8) != *((pointer + offset) as *const u8)
                    {
                        return 0;
                    }
                }
                return 1;
            }
        }
        if LENGTH == CAPACITY {
            let capacity = CAPACITY.checked_mul(2).unwrap().max(8);
            let buffer = telora_alloc(capacity.checked_mul(SOURCE_BYTES).unwrap());
            if LENGTH != 0 {
                core::ptr::copy_nonoverlapping(
                    BUFFER as *const Source,
                    buffer as *mut Source,
                    LENGTH as usize,
                );
            }
            BUFFER = buffer;
            CAPACITY = capacity;
        }
        (BUFFER as *mut Source).add(LENGTH as usize).write(Source {
            id,
            pointer,
            length,
            lines: 0,
            line_count: 0,
        });
        LENGTH += 1;
        1
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
            let start = crate::values::word(pointer + index * 8, 0);
            let end = crate::values::word(pointer + index * 8, 4);
            assert!(start <= end);
            if index == 0 { assert_eq!(start, 0); }
            else { assert!(start > previous_end && start - previous_end <= 2); }
            previous_end = end;
        }
        for index in 0..LENGTH {
            let source = &mut *(BUFFER as *mut Source).add(index as usize);
            if source.id != id { continue; }
            if source.line_count != 0 {
                assert_eq!(source.line_count, count, "source index changed");
                let old = core::slice::from_raw_parts(source.lines as *const u8, bytes as usize);
                let new = core::slice::from_raw_parts(pointer as *const u8, bytes as usize);
                assert_eq!(old, new, "source index changed");
                return;
            }
            source.lines = if static_storage { pointer } else {
                let owned = telora_alloc(bytes);
                core::ptr::copy_nonoverlapping(pointer as *const u8, owned as *mut u8, bytes as usize);
                owned
            };
            source.line_count = count;
            return;
        }
        panic!("unregistered source index");
    }
}

/// Expand only when emitting diagnostics; ordinary values carry byte offsets.
pub(crate) unsafe fn position(id: u32, byte: u32) -> (u32, u32) {
    unsafe {
        for index in 0..LENGTH {
            let source = (BUFFER as *const Source).add(index as usize).read();
            if source.id != id { continue; }
            assert_ne!(source.line_count, 0, "source lacks index");
            assert!(byte <= crate::values::word(source.lines + (source.line_count - 1) * 8, 4));
            let mut lo = 0;
            let mut hi = source.line_count;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if crate::values::word(source.lines + mid * 8, 0) <= byte { lo = mid + 1; }
                else { hi = mid; }
            }
            let line = lo - 1;
            let start = crate::values::word(source.lines + line * 8, 0);
            let end = crate::values::word(source.lines + line * 8, 4);
            return (line, byte.min(end) - start);
        }
        panic!("unregistered source");
    }
}

/// Returns an address of two u32 words: UTF-8 pointer and byte length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_source_name(id: u32) -> u32 {
    unsafe {
        for index in 0..LENGTH {
            let item = (BUFFER as *const Source).add(index as usize);
            if (*item).id == id {
                return item as u32 + 4;
            }
        }
        number_text(b"source:", id, b"")
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
        (result as *mut u32).write(result + 8);
        ((result + 4) as *mut u32).write(length);
        core::ptr::copy_nonoverlapping(prefix.as_ptr(), (result + 8) as *mut u8, prefix.len());
        core::ptr::copy_nonoverlapping(
            digits[at..].as_ptr(),
            (result + 8 + prefix.len() as u32) as *mut u8,
            digits.len() - at,
        );
        core::ptr::copy_nonoverlapping(
            suffix.as_ptr(),
            (result + 8 + prefix.len() as u32 + (digits.len() - at) as u32) as *mut u8,
            suffix.len(),
        );
        result
    }
}

/// Allocate from the Guest registry, after the deterministic static source list.
pub(crate) unsafe fn next_id() -> u32 {
    unsafe {
        let mut highest = 0;
        for index in 0..LENGTH {
            highest = highest.max((BUFFER as *const Source).add(index as usize).read().id);
        }
        highest.checked_add(1).expect("source identity overflow")
    }
}
