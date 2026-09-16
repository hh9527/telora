//! Fixed format-node ABI: operation, first value pointer, second value pointer.
//! Arguments have statically prescribed physical layouts, never inferred types.
use crate::{abi::*, tables::telora_table_get, text::text, values::word};
use core::fmt::{self, Write};

unsafe fn array_item(value: u32, index: u32, width: u32) -> u32 {
    unsafe {
        let base = word(
            telora_table_get(table_address(ARRAYS), word(value, DATA)),
            0,
        );
        base + (word(value, DATA + 4) + index) * width
    }
}

unsafe fn render(value: u32, output: &mut dyn Write, depth: u32) -> fmt::Result {
    if depth >= 128 {
        return Err(fmt::Error);
    }
    unsafe {
        let node = word(
            telora_table_get(table_address(FORMATS), word(value, DATA)),
            0,
        );
        let first = word(node, 4);
        match word(node, 0) {
            1 => output.write_str(text(first)),
            2 => write!(
                output,
                "{}",
                crate::heap::read::<i64>(first + DATA as u32)
            ),
            3 => write!(
                output,
                "{}",
                crate::heap::read::<f64>(first + DATA as u32)
            ),
            4 => {
                let items = word(node, 8);
                let count = word(items, DATA + 8) - word(items, DATA + 4);
                // The generated constructor validates lengths before publishing.
                for index in 0..count {
                    output.write_str(text(array_item(first, index, STRING_BYTES)))?;
                    render(array_item(items, index, SCALAR_BYTES), output, depth + 1)?;
                }
                output.write_str(text(array_item(first, count, STRING_BYTES)))
            }
            _ => core::arch::wasm32::unreachable(),
        }
    }
}

/// Returns a raw UTF-8 span, or zero for the language's render-depth failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_format_render(value: u32) -> u32 {
    unsafe { crate::format::try_render_with(|output| render(value, output, 0)) }
}

/// Codegen supplies (operation, value pointer) pairs after evaluating all parts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_format_join(parts: u32, count: u32) -> u32 {
    unsafe {
        crate::format::try_render_with(|output| {
            for index in 0..count {
                let part = parts + index * 8;
                match word(part, 0) {
                    1 => output.write_str(text(word(part, 4)))?,
                    2 => render(word(part, 4), output, 0)?,
                    _ => core::arch::wasm32::unreachable(),
                }
            }
            Ok(())
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_format_message(strings: u32, items: u32) -> u32 {
    unsafe {
        crate::format::render_with(|output| {
            write!(
                output,
                "std/fmt.concat requires strings.len == items.len + 1, got {strings} and {items}"
            )
        })
    }
}
