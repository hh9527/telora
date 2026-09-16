//! Fixed writer ABI; no TypeId dispatch or language graph traversal.
use crate::{json_text::Writer, text::text};
use alloc::boxed::Box;

/// op 0 creates a writer (a = indent, u32::MAX means compact).
/// Other operations use a = writer pointer. Finish consumes the writer and
/// returns a raw UTF-8 span. b is an immediate or borrowed descriptor pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_json_write(op: u32, a: u32, b: u32) -> u32 {
    unsafe {
        if op == 0 {
            return Box::into_raw(Box::new(Writer::new((a != u32::MAX).then_some(a)))) as u32;
        }
        if op == 1 {
            let writer = Box::from_raw(a as *mut Writer);
            return crate::format::render_with(|out| out.write_str(&writer.output));
        }
        let writer = &mut *(a as *mut Writer);
        match op {
            10 => writer.output.push_str(text(b)),
            2 => writer.quoted(text(b)),
            3 => writer.integer(crate::heap::read::<i64>(b + crate::abi::DATA as u32)),
            4 => return u32::from(writer.float(crate::heap::read::<f64>(b + crate::abi::DATA as u32))),
            5 => writer.output.push_str(match b {
                0 => "null",
                1 => "true",
                2 => "false",
                _ => core::arch::wasm32::unreachable(),
            }),
            6 => writer.open(b != 0),
            7 => writer.item(b),
            8 => writer.colon(),
            9 => writer.close(b & 1 != 0, b & 2 != 0),
            _ => core::arch::wasm32::unreachable(),
        }
        1
    }
}
