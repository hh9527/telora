//! Fixed parse-plan ABI. Node IDs are postorder indices, never TypeIds.
//! Result: {rows, count, root, error_descriptor}, four u32 words.
//! Error descriptor: {text_ptr, text_len, diagnostics_ptr, diagnostics_len}.
//! Diagnostics are comma-separated JSON records; text serves language Result.
//! Row (24 bytes): {kind:u32, src:u32, start:u32, end:u32, payload:u64}.
//! Text payloads point directly into input or a shared decoded buffer.
use alloc::string::String;
mod json;
mod errors;
mod origins;
use origins::Origins;
mod toml;
mod yaml;

// External input is borrowed. Only published source spans need persistent bytes;
// decoded spans already belong to the Guest. Language String input stays owned.
fn span_bits(span: telora_data::json::text::TextSpan, source: u32, decoded: u32, borrowed: bool) -> u64 {
    use telora_data::json::text::TextSpan;
    let (base, range, copy) = match span {
        TextSpan::Source(range) => (source, range, borrowed),
        TextSpan::Decoded(range) => (decoded, range, false),
    };
    let length = u32::try_from(range.len()).unwrap();
    let mut pointer = base.checked_add(u32::try_from(range.start).unwrap()).unwrap();
    if copy && length != 0 {
        unsafe {
            let owned = crate::telora_alloc(length);
            core::ptr::copy_nonoverlapping(pointer as *const u8, owned as *mut u8, length as usize);
            pointer = owned;
        }
    } else if length == 0 {
        pointer = 1;
    }
    u64::from(pointer) | (u64::from(length) << 32)
}

unsafe fn put(pointer: u32, offset: u32, value: u32) {
    unsafe {
        ((pointer + offset) as *mut u32).write_unaligned(value);
    }
}
fn string_bytes(value: String) -> (u32, u32) {
    retained_bytes(value.as_bytes())
}

fn retained_bytes(bytes: &[u8]) -> (u32, u32) {
    let length = u32::try_from(bytes.len()).unwrap();
    if length == 0 { return (1, 0); }
    unsafe {
        let pointer = crate::telora_alloc(length);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len());
        (pointer, length)
    }
}
unsafe fn export_error(message: String) -> u32 {
    let diagnostic = telora_data::source::Diagnostic {
        severity: telora_data::source::Severity::Error, message,
        labels: alloc::vec::Vec::new(), notes: alloc::vec::Vec::new(),
    };
    unsafe { errors::export(alloc::vec![diagnostic], &Origins::Inherit(telora_wasm_shared::source_range::SourceRange::NONE), "") }
}

pub(crate) unsafe fn error_text(span: u32) -> &'static str {
    unsafe { core::str::from_utf8(core::slice::from_raw_parts(
        crate::values::word(span, 0) as *const u8,
        crate::values::word(span, 4) as usize)).unwrap() }
}
/// Generated callers supply final language type identities and layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_json_parse(input: u32) -> u32 {
    unsafe { json::parse(crate::text::text(input), &Origins::inherit(input), false) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_yaml_parse(input: u32) -> u32 {
    unsafe { yaml::parse(crate::text::text(input), &Origins::inherit(input), false) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_toml_parse(input: u32) -> u32 {
    unsafe { toml::parse(crate::text::text(input), &Origins::inherit(input), false) }
}

/// Internal plan producer. Retain published text spans and the source index,
/// never the borrowed transfer buffer or a complete copy of the source.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_parse_data(pointer: u32, length: u32, format: u32, source: u32) -> u32 {
    unsafe {
        assert!((1..=3).contains(&format), "unknown data format");
        assert_ne!(pointer, 0);
        let end = pointer.checked_add(length).expect("input range overflow");
        assert!(end <= crate::telora_heap_end());
        let bytes = core::slice::from_raw_parts(pointer as *const u8, length as usize);
        let input = match core::str::from_utf8(bytes) {
            Ok(input) => input,
            Err(_) => return export_error(String::from("input is not UTF-8")),
        };
        let origins = Origins::new(source, input);
        match format {
            1 => json::parse(input, &origins, true),
            2 => yaml::parse(input, &origins, true),
            3 => toml::parse(input, &origins, true),
            _ => core::arch::wasm32::unreachable(),
        }
    }
}
