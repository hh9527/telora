//! Platform-independent lexical paths. Results are raw UTF-8 spans, not Options.
use crate::{abi::*, tables::telora_table_get, text::text, values::word};

unsafe fn join(array: u32) -> alloc::string::String {
    unsafe {
        let base = word(
            telora_table_get(table_address(ARRAYS), word(array, DATA)),
            0,
        );
        let end = word(array, DATA + 8);
        let mut start = word(array, DATA + 4);
        for index in start..end {
            if text(base + index * STRING_BYTES).starts_with('/') {
                start = index;
            }
        }
        let span = crate::format::render_with(|output| {
            let mut empty = true;
            let mut slash = false;
            for index in start..end {
                let part = text(base + index * STRING_BYTES);
                if !empty && !slash {
                    output.write_char('/')?;
                    slash = true;
                }
                output.write_str(part)?;
                if !part.is_empty() {
                    empty = false;
                    slash = part.ends_with('/');
                }
            }
            Ok(())
        });
        core::str::from_utf8(core::slice::from_raw_parts(
            crate::heap::ptr::<u8>(word(span, 0)),
            word(span, 4) as usize,
        ))
        .unwrap().into()
    }
}

unsafe fn normalize(input: &str) -> alloc::string::String {
    unsafe {
        let capacity = input.len().checked_add(1).unwrap();
        let mut output = alloc::vec![0; capacity];
        let absolute = input.starts_with('/');
        let mut length = usize::from(absolute);
        if absolute {
            output[0] = b'/';
        }
        for component in input.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let start = output[..length]
                    .iter()
                    .rposition(|&byte| byte == b'/')
                    .map_or(0, |index| index + 1);
                if start < length && &output[start..length] != b".." {
                    length = start.saturating_sub(1).max(usize::from(absolute));
                    continue;
                }
                if absolute {
                    continue;
                }
            }
            if length != 0 && output[length - 1] != b'/' {
                output[length] = b'/';
                length += 1;
            }
            let end = length.checked_add(component.len()).unwrap();
            output[length..end].copy_from_slice(component.as_bytes());
            length = end;
        }
        if length == 0 {
            output[0] = b'.';
            length = 1;
        }
        output.truncate(length);
        alloc::string::String::from_utf8(output).unwrap()
    }
}

/// 0 join, 1 normalize, 2 parent, 3 file_name. Zero denotes no path component;
/// the generated caller constructs the sealed Option(String) representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_path(operation: u32, input: u32) -> u32 {
    unsafe {
        let value = if operation == 0 { normalize(&join(input)) } else { normalize(text(input)) };
        let value = value.as_str();
        let result = match operation {
            0 | 1 => Some(value),
            2 => match value {
                "." | "/" => None,
                value => Some(match value.rfind('/') {
                    Some(0) => "/",
                    Some(index) => &value[..index],
                    None => ".",
                }),
            },
            3 => match value {
                "." | "/" | ".." => None,
                value => value.rsplit('/').next(),
            },
            _ => core::arch::wasm32::unreachable(),
        };
        let Some(result) = result else { return 0 };
        let span = crate::telora_alloc(8 + result.len() as u32);
        crate::heap::write(span, span + 8);
        crate::heap::write(span + 4, result.len() as u32);
        core::ptr::copy_nonoverlapping(result.as_ptr(), crate::heap::ptr::<u8>(span + 8), result.len());
        span
    }
}
