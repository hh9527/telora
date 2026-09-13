//! Fixed UTF-8 operations. Type-bound headers are assembled by codegen.
use crate::{
    abi::*,
    tables::telora_table_get,
    values::{string_span, word},
};

pub(crate) unsafe fn text(value: u32) -> &'static str {
    unsafe {
        let (pointer, length) = string_span(value);
        core::str::from_utf8(core::slice::from_raw_parts(
            pointer as *const u8,
            length as usize,
        ))
        .unwrap()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_text_query(operation: u32, a: u32, b: u32) -> u32 {
    unsafe {
        let a = text(a);
        match operation {
            0 => a.chars().count() as u32,
            1 => u32::from(a.starts_with(text(b))),
            2 => u32::from(a.ends_with(text(b))),
            3 => u32::from(a.contains(text(b))),
            _ => core::arch::wasm32::unreachable(),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_text_build(operation: u32, a: u32, b: u32, c: u32) -> u32 {
    unsafe {
        crate::format::render_with(|output| {
            match operation {
                0 | 1 => {
                    let separator = if operation == 0 { text(b) } else { "\n" };
                    let base = word(telora_table_get(table_address(ARRAYS), word(a, DATA)), 0);
                    let start = word(a, 20);
                    for index in start..word(a, 24) {
                        if index != start {
                            output.write_str(separator)?;
                        }
                        output.write_str(text(base + index * 32))?;
                    }
                }
                2 => {
                    let source = text(a);
                    let replacement = text(c);
                    let mut start = 0;
                    for (index, matched) in source.match_indices(text(b)) {
                        output.write_str(&source[start..index])?;
                        output.write_str(replacement)?;
                        start = index + matched.len();
                    }
                    output.write_str(&source[start..])?;
                }
                3 => {
                    for line in text(a).split_inclusive('\n') {
                        if !line.trim_matches(['\r', '\n']).is_empty() {
                            for _ in 0..b {
                                output.write_char(' ')?;
                            }
                        }
                        output.write_str(line)?;
                    }
                }
                4 => {
                    let source = text(a);
                    output.write_str(source)?;
                    if !source.ends_with('\n') {
                        output.write_char('\n')?;
                    }
                }
                5 => {
                    let margin = text(b);
                    for line in text(a).split_inclusive('\n') {
                        let end = line.trim_end_matches(['\r', '\n']).len();
                        let content = &line[..end];
                        let marker = content
                            .bytes()
                            .take_while(|byte| matches!(byte, b' ' | b'\t'))
                            .count();
                        output
                            .write_str(content[marker..].strip_prefix(margin).unwrap_or(content))?;
                        output.write_str(&line[end..])?;
                    }
                }
                _ => core::arch::wasm32::unreachable(),
            }
            Ok(())
        })
    }
}

unsafe fn spans(pieces: impl Iterator<Item = &'static str> + Clone) -> u32 {
    unsafe {
        let count = pieces.clone().count();
        let bytes = count.checked_mul(8).and_then(|n| n.checked_add(8)).unwrap();
        let result = crate::telora_alloc(u32::try_from(bytes).unwrap());
        (result as *mut u32).write(result + 8);
        ((result + 4) as *mut u32).write(count as u32);
        for (index, piece) in pieces.enumerate() {
            let row = (result + 8 + index as u32 * 8) as *mut u32;
            row.write(piece.as_ptr() as u32);
            row.add(1).write(piece.len() as u32);
        }
        result
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_text_split(operation: u32, a: u32, b: u32) -> u32 {
    unsafe {
        match operation {
            0 => spans(text(a).split(text(b))),
            1 => spans(
                text(a)
                    .split('\n')
                    .map(|line| line.strip_suffix('\r').unwrap_or(line)),
            ),
            _ => core::arch::wasm32::unreachable(),
        }
    }
}
