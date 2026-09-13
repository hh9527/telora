//! Fixed text formatting into a raw UTF-8 span; no language type construction.
use core::fmt::{self, Write};

struct Counter(usize);
impl Write for Counter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0 = self.0.checked_add(text.len()).ok_or(fmt::Error)?;
        Ok(())
    }
}
struct Output {
    pointer: *mut u8,
    offset: usize,
    capacity: usize,
}
impl Write for Output {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self
            .offset
            .checked_add(text.len())
            .filter(|&end| end <= self.capacity)
            .ok_or(fmt::Error)?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                text.as_ptr(),
                self.pointer.add(self.offset),
                text.len(),
            );
        }
        self.offset = end;
        Ok(())
    }
}
unsafe fn render(arguments: fmt::Arguments<'_>) -> u32 {
    unsafe { render_with(|writer| fmt::write(writer, arguments)) }
}

pub(crate) unsafe fn render_with(mut write: impl FnMut(&mut dyn Write) -> fmt::Result) -> u32 {
    let result = unsafe { try_render_with(&mut write) };
    assert_ne!(result, 0);
    result
}

pub(crate) unsafe fn try_render_with(mut write: impl FnMut(&mut dyn Write) -> fmt::Result) -> u32 {
    let mut size = Counter(0);
    if write(&mut size).is_err() {
        return 0;
    }
    let bytes = u32::try_from(size.0).unwrap();
    unsafe {
        let span = crate::telora_alloc(bytes.checked_add(8).unwrap());
        (span as *mut u32).write(span + 8);
        ((span + 4) as *mut u32).write(bytes);
        write(&mut Output {
            pointer: (span + 8) as *mut u8,
            offset: 0,
            capacity: size.0,
        })
        .unwrap();
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
