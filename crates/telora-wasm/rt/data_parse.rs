//! Fixed parse-plan ABI. Node IDs are postorder indices, never TypeIds.
//! Result: {rows, count, root, error_span}, four u32 words.
//! Row (16 bytes): {kind:u32, reserved:u32, payload:u64}.
//! Text payloads point directly into input or a shared decoded buffer.
use alloc::{boxed::Box, string::String};
mod json;
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
    unsafe {
        let result = crate::telora_alloc(16);
        for offset in [0, 4, 8] {
            put(result, offset, 0);
        }
        let span = crate::format::render_with(|out| out.write_str(&message));
        put(result, 12, span);
        result
    }
}
/// Generated callers supply final language type identities and layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_json_parse(input: u32) -> u32 {
    unsafe { json::parse(crate::text::text(input)) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_yaml_parse(input: u32) -> u32 {
    unsafe { yaml::parse(crate::text::text(input)) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_toml_parse(input: u32) -> u32 {
    unsafe { toml::parse(crate::text::text(input)) }
}
