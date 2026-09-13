//! Persistent hash updates return new slots; finished digests leave inputs intact.
use crate::{
    abi::*,
    tables::{telora_table_get, telora_table_push},
    values::word,
};
use alloc::boxed::Box;
use telora_sha256::Context;

unsafe fn state(id: u32) -> &'static Context {
    unsafe { &*(word(telora_table_get(table_address(HASHES), id), 0) as *const Context) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_hash(operation: u32, a: u32, b: u32) -> u32 {
    unsafe {
        if operation == 0 {
            let digest = telora_sha256::hex(crate::text::text(a).as_bytes());
            return crate::format::render(format_args!("{digest}"));
        }
        if operation == 6 {
            return (state(a) == state(b)) as u32;
        }
        let mut next = if operation == 1 {
            let mut initial = Context::default();
            initial.update(b"telora.hash\0\x01");
            initial
        } else {
            state(a).clone()
        };
        match operation {
            1 => {}
            2 => {
                let base = word(telora_table_get(table_address(BYTES), word(b, DATA)), 0);
                let start = word(b, 20);
                let length = word(b, 24) - start;
                let bytes =
                    core::slice::from_raw_parts((base + start) as *const u8, length as usize);
                next.update(&[1]);
                next.update(&(length as u64).to_be_bytes());
                next.update(bytes);
            }
            3 => {
                let bytes = crate::text::text(b).as_bytes();
                next.update(&[2]);
                next.update(&(bytes.len() as u64).to_be_bytes());
                next.update(bytes);
            }
            4 => {
                let bits = ((b + DATA as u32) as *const u64).read_unaligned();
                next.update(&[3]);
                next.update(&bits.to_be_bytes());
            }
            5 => {
                let data = crate::telora_alloc(32);
                core::ptr::copy_nonoverlapping(next.finish().as_ptr(), data as *mut u8, 32);
                return data;
            }
            _ => core::arch::wasm32::unreachable(),
        }
        let pointer = Box::into_raw(Box::new(next)) as u32;
        telora_table_push(
            table_address(HASHES),
            pointer,
            core::mem::size_of::<Context>() as u32,
        )
    }
}
