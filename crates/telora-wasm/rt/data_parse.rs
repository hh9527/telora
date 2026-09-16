//! Fixed parse-plan ABI. Node IDs are postorder indices, never TypeIds.
//! Result: {rows, count, root, error_descriptor}, four u32 words.
//! Error descriptor: {text_ptr, text_len, diagnostics_ptr, diagnostics_len}.
//! Diagnostics are comma-separated JSON records; text serves language Result.
//! Row (16 bytes): {kind:u32, loc:LocId, payload:u64}.
//! Text payloads point directly into input or a shared decoded buffer.
use alloc::{boxed::Box, string::String};
mod json;
mod errors;
mod origins;
use origins::Origins;
mod toml;
mod yaml;

unsafe fn put(pointer: u32, offset: u32, value: u32) {
    unsafe {
        ((pointer + offset) as *mut u32).write_unaligned(value);
    }
}
fn string_bytes(value: String) -> (u32, u32) {
    let bytes = Box::leak(value.into_bytes().into_boxed_slice());
    (bytes.as_mut_ptr() as u32, bytes.len() as u32)
}
unsafe fn export_error(message: String) -> u32 {
    let diagnostic = telora_data::source::Diagnostic {
        severity: telora_data::source::Severity::Error, message,
        labels: alloc::vec::Vec::new(), notes: alloc::vec::Vec::new(),
    };
    unsafe { errors::export(alloc::vec![diagnostic], "", &Origins::Inherit(0), "") }
}

pub(crate) unsafe fn error_text(span: u32) -> &'static str {
    unsafe { core::str::from_utf8(core::slice::from_raw_parts(
        crate::values::word(span, 0) as *const u8,
        crate::values::word(span, 4) as usize)).unwrap() }
}
/// Generated callers supply final language type identities and layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_json_parse(input: u32) -> u32 {
    unsafe { json::parse(crate::text::text(input), &Origins::Inherit(crate::values::word(input, crate::abi::SOURCE))) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_yaml_parse(input: u32) -> u32 {
    unsafe { yaml::parse(crate::text::text(input), &Origins::Inherit(crate::values::word(input, crate::abi::SOURCE))) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_toml_parse(input: u32) -> u32 {
    unsafe { toml::parse(crate::text::text(input), &Origins::Inherit(crate::values::word(input, crate::abi::SOURCE))) }
}

/// Internal plan producer used by generated input glue. Retain one Guest-owned
/// input allocation for spans; no Host tree or location interning.
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
        let (owned, length) = string_bytes(String::from(input));
        let input = core::str::from_utf8_unchecked(core::slice::from_raw_parts(owned as *const u8, length as usize));
        let origins = Origins::new(source, input);
        match format {
            1 => json::parse(input, &origins),
            2 => yaml::parse(input, &origins),
            3 => toml::parse(input, &origins),
            _ => core::arch::wasm32::unreachable(),
        }
    }
}
