//! Fixed text formatting into a raw UTF-8 span; no language type construction.
use core::fmt::{self, Write};

pub(crate) unsafe fn render(arguments: fmt::Arguments<'_>) -> u32 {
    unsafe { render_with(|writer| fmt::write(writer, arguments)) }
}

pub(crate) unsafe fn render_with(mut write: impl FnMut(&mut dyn Write) -> fmt::Result) -> u32 {
    let result = unsafe { try_render_with(&mut write) };
    assert_ne!(result, 0);
    result
}

pub(crate) unsafe fn try_render_with(mut write: impl FnMut(&mut dyn Write) -> fmt::Result) -> u32 {
    // Finish reading all language values before appending to the word arena.
    let mut text = alloc::string::String::new();
    if write(&mut text).is_err() {
        return 0;
    }
    let bytes = u32::try_from(text.len()).unwrap();
    unsafe {
        let span = crate::telora_alloc(bytes.checked_add(8).unwrap());
        crate::heap::write(span, span + 8);
        crate::heap::write(span + 4, bytes);
        core::ptr::copy_nonoverlapping(text.as_ptr(), crate::heap::ptr::<u8>(span + 8), text.len());
        span
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_duplicate_key_message(value: u32) -> u32 {
    unsafe {
        let (pointer, length) = crate::values::string_span(value);
        let text = core::str::from_utf8(core::slice::from_raw_parts(
            pointer as *const u8,
            length as usize,
        ))
        .unwrap();
        render(format_args!(
            "std/dict.from_pairs contains duplicate field {text:?}"
        ))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_member_message(operation: u32, a: u32, b: u32) -> u32 {
    unsafe {
        match operation {
            0 => render(format_args!("field index {a} is out of range")),
            1 => render(format_args!("Dyn variant index is {a}, not {b}")),
            2 => render(format_args!("Dyn record has no field {:?}", crate::text::text(a))),
            _ => core::arch::wasm32::unreachable(),
        }
    }
}
