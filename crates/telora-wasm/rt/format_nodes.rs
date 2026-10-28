//! Fixed format-node ABI: operation, first value pointer, second value pointer.
//! Arguments have statically prescribed physical layouts, never inferred types.
use crate::{abi::*, tables::telora_table_get, text::text, values::word};
use core::fmt::{self, Write};

unsafe fn array_item(value: u32, index: u32, width: u32) -> u32 {
    unsafe {
        let base = crate::values::array_base(value);
        base + (word(value, DATA + 4) + index) * width
    }
}

// Append chains do not add nesting depth; concat and embedded Fmt do.
// The work stack lives outside managed memory and holds borrowed pointers only.
unsafe fn render(value: u32, output: &mut dyn Write, depth: u32) -> fmt::Result {
    enum Task { Format(u32, u32), Text(u32), Int(u32), Float(u32), Quoted(u32) }
    let mut tasks = alloc::vec![Task::Format(value, depth)];
    unsafe {
        while let Some(task) = tasks.pop() {
            match task {
                Task::Text(v) => output.write_str(text(v))?,
                Task::Int(v) => write!(output, "{}", crate::heap::read::<i64>(v + DATA as u32))?,
                Task::Float(v) => write!(output, "{}", crate::heap::read::<f64>(v + DATA as u32))?,
                Task::Quoted(v) => crate::json_text::quoted(output, text(v))?,
                Task::Format(value, depth) => {
                    if depth >= 128 { return Err(fmt::Error); }
                    let node = word(telora_table_get(table_address(FORMATS), word(value, DATA)), 0);
                    let first = word(node, 4);
                    let second = word(node, 8);
                    match word(node, 0) {
                        0 => {},
                        1 => tasks.push(Task::Text(first)),
                        2 => tasks.push(Task::Int(first)),
                        3 => tasks.push(Task::Float(first)),
                        4 => {
                            let count = word(second, DATA + 8) - word(second, DATA + 4);
                            tasks.push(Task::Text(array_item(first, count, STRING_BYTES)));
                            for index in (0..count).rev() {
                                tasks.push(Task::Format(array_item(second, index, SCALAR_BYTES), depth + 1));
                                tasks.push(Task::Text(array_item(first, index, STRING_BYTES)));
                            }
                        },
                        op @ 6..=11 => {
                            tasks.push(match op {
                                6 => Task::Text(second), 7 => Task::Int(second),
                                8 | 11 => Task::Float(second), 9 => Task::Quoted(second),
                                10 => Task::Format(second, depth + 1), _ => unreachable!(),
                            });
                            tasks.push(Task::Format(first, depth));
                        },
                        _ => core::arch::wasm32::unreachable(),
                    }
                },
            }
        }
        Ok(())
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
                    3 => write!(output, "{}", crate::heap::read::<i64>(word(part, 4) + DATA as u32))?,
                    4 => write!(output, "{}", crate::heap::read::<f64>(word(part, 4) + DATA as u32))?,
                    5 => crate::json_text::quoted(output, text(word(part, 4)))?,
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
