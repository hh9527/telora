//! Source names are input metadata, not language values or type information.
use crate::telora_alloc;

#[repr(C)]
#[derive(Clone, Copy)]
struct Source {
    id: u32,
    pointer: u32,
    length: u32,
}
static mut BUFFER: u32 = 0;
static mut LENGTH: u32 = 0;
static mut CAPACITY: u32 = 0;

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
            let buffer = telora_alloc(capacity.checked_mul(12).unwrap());
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
        });
        LENGTH += 1;
        1
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
