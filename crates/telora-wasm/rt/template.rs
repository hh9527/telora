//! UTF-8 template parsing. Produces two raw span lists, no language values.
use crate::{format::render_with, telora_alloc, text::text};

enum Piece<'a> {
    Text(&'a str),
    Field(&'a str),
}
enum Error<'a> {
    Nested,
    Unclosed,
    Unmatched,
    Field(&'a str),
}

fn scan<'a>(source: &'a str, mut emit: impl FnMut(Piece<'a>)) -> Result<(), Error<'a>> {
    let mut chars = source.char_indices().peekable();
    let mut start = 0;
    while let Some((index, ch)) = chars.next() {
        if !matches!(ch, '{' | '}') {
            continue;
        }
        emit(Piece::Text(&source[start..index]));
        if chars.peek().is_some_and(|&(_, next)| next == ch) {
            chars.next();
            emit(Piece::Text(&source[index..index + 1]));
            start = index + 2;
        } else if ch == '{' {
            let end = loop {
                match chars.next() {
                    Some((end, '}')) => break end,
                    Some((_, '{')) => return Err(Error::Nested),
                    Some(_) => {}
                    None => return Err(Error::Unclosed),
                }
            };
            let field = &source[index + 1..end];
            if field.is_empty()
                || field.starts_with(|ch: char| ch.is_ascii_digit())
                || !field
                    .chars()
                    .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
            {
                return Err(Error::Field(field));
            }
            emit(Piece::Field(field));
            start = end + 1;
        } else {
            return Err(Error::Unmatched);
        }
    }
    emit(Piece::Text(&source[start..]));
    Ok(())
}

unsafe fn span(destination: u32, pointer: u32, length: u32) {
    unsafe {
        (destination as *mut u32).write(pointer);
        ((destination + 4) as *mut u32).write(length);
    }
}

/// Result: strings pointer/count, fields pointer/count, error-message span or 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_template_prepare(value: u32) -> u32 {
    unsafe {
        let source = text(value);
        let result = telora_alloc(20);
        core::ptr::write_bytes(result as *mut u8, 0, 20);
        let mut count = 0u32;
        if let Err(error) = scan(source, |piece| {
            if matches!(piece, Piece::Field(_)) {
                count = count.checked_add(1).unwrap();
            }
        }) {
            let message = render_with(|output| match error {
                Error::Nested => output.write_str("nested '{' in Display template field"),
                Error::Unclosed => output.write_str("unclosed Display template field"),
                Error::Unmatched => output.write_str("unmatched '}' in Display template"),
                Error::Field(field) => write!(output, "invalid Display template field {field:?}"),
            });
            ((result + 16) as *mut u32).write(message);
            return result;
        }
        let strings = telora_alloc(count.checked_add(1).unwrap().checked_mul(8).unwrap());
        let fields = telora_alloc(count.checked_mul(8).unwrap());
        let buffer = telora_alloc(u32::try_from(source.len()).unwrap());
        let mut offset = 0u32;
        let mut start = 0u32;
        let mut index = 0u32;
        let scanned = scan(source, |piece| match piece {
            Piece::Text(text) => {
                core::ptr::copy_nonoverlapping(
                    text.as_ptr(),
                    (buffer + offset) as *mut u8,
                    text.len(),
                );
                offset += text.len() as u32;
            }
            Piece::Field(field) => {
                span(strings + index * 8, buffer + start, offset - start);
                span(
                    fields + index * 8,
                    field.as_ptr() as u32,
                    field.len() as u32,
                );
                start = offset;
                index += 1;
            }
        });
        assert!(scanned.is_ok());
        span(strings + count * 8, buffer + start, offset - start);
        span(result, strings, count + 1);
        span(result + 8, fields, count);
        result
    }
}
