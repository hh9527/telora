use crate::{abi::*, tables::telora_table_get};

unsafe fn word(pointer: u32, offset: u64) -> u32 {
    unsafe { ((pointer + offset as u32) as *const u32).read_unaligned() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_invoke(value: u32, args: u32) -> u32 {
    unsafe {
        let environment = word(value, ENVIRONMENT);
        let environment = if environment == 0 {
            0
        } else {
            word(
                telora_table_get(table_address(ENVIRONMENTS), environment - 1),
                0,
            )
        };
        let callback: unsafe extern "C" fn(u32, u32) -> u32 =
            core::mem::transmute(word(value, DATA));
        callback(environment, args)
    }
}

unsafe fn string_span(value: u32) -> (u32, u32) {
    unsafe {
        if *((value + DATA as u32) as *const u8) == 0 {
            (value + 18, *((value + 17) as *const u8) as u32)
        } else {
            let start = word(value, 24);
            let end = word(value, 28);
            let base = word(telora_table_get(table_address(STRINGS), word(value, 20)), 0);
            (base + start, end - start)
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_string_compare(a: u32, b: u32) -> i32 {
    unsafe {
        let (a, a_len) = string_span(a);
        let (b, b_len) = string_span(b);
        for index in 0..a_len.min(b_len) {
            let a = *((a + index) as *const u8);
            let b = *((b + index) as *const u8);
            if a != b {
                return i32::from(a) - i32::from(b);
            }
        }
        i32::from(a_len > b_len) - i32::from(a_len < b_len)
    }
}
