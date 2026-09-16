//! Initialization diagnostics are serialized in Guest, including LocId expansion.
use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;
use crate::{abi::*, json_text::quoted, tables::telora_table_get, values::word};

unsafe fn span(pointer: u32) -> &'static str {
    unsafe { core::str::from_utf8(core::slice::from_raw_parts(
        word(pointer, 0) as *const u8, word(pointer, 4) as usize)).unwrap() }
}

unsafe fn label(output: &mut String, id: u32, message: &str, primary: bool) {
    unsafe {
        let record = crate::locations::telora_location_get(id);
        output.push_str("{\"location\":{\"source\":");
        quoted(output, span(crate::sources::telora_source_name(word(record, 0)))).unwrap();
        write!(output, ",\"start\":{{\"line\":{},\"offset\":{}}},\"end\":{{\"line\":{},\"offset\":{}}}}},\"message\":",
            word(record, 4), word(record, 8), word(record, 12), word(record, 16)).unwrap();
        quoted(output, message).unwrap();
        write!(output, ",\"primary\":{primary}}}").unwrap();
    }
}

unsafe fn event(output: &mut String, pointer: u32) {
    unsafe {
        let warning = word(pointer, DIAG_WARNING) != 0;
        output.push_str(if warning { "{\"severity\":\"Warning\",\"message\":" }
            else { "{\"severity\":\"Error\",\"message\":" });
        let code = word(pointer, DIAG_CODE);
        let message = if code == 9 { crate::text::text(word(pointer, DIAG_MESSAGE)) }
            else { telora_wasm_shared::diagnostics::error_message(code) };
        quoted(output, message).unwrap();
        output.push_str(",\"labels\":[");
        let origin = word(pointer, 0);
        let mut seen = Vec::new();
        if origin != 0 {
            label(output, origin, message, true);
            seen.push(origin);
        }
        let subjects = word(pointer, DIAG_SUBJECTS);
        for index in 0..word(pointer, DIAG_COUNT) {
            let id = word(subjects, u64::from(index) * u64::from(LOC_BYTES));
            if id == 0 || seen.contains(&id) { continue; }
            if !seen.is_empty() { output.push(','); }
            label(output, id, &format!("subject {} originated here", index + 1), false);
            seen.push(id);
        }
        output.push_str("],\"notes\":[]}");
    }
}

#[unsafe(export_name = "get-service-diagnostics")]
pub unsafe extern "C" fn get(output: u32, cap: u32, result: u32) {
    unsafe {
        let mut buffer = super::buffers::take(1, 0, output, cap, result);
        let mut json = String::from("[");
        let count = word(table_address(DIAGNOSTICS), 4);
        for index in 0..count {
            if index != 0 { json.push(','); }
            let slot = telora_table_get(table_address(DIAGNOSTICS), index);
            assert_eq!(word(slot, 4), DIAGNOSTIC_BYTES);
            event(&mut json, word(slot, 0));
        }
        for (index, message) in super::service().errors.iter().enumerate() {
            if count != 0 || index != 0 { json.push(','); }
            json.push_str("{\"severity\":\"Error\",\"message\":");
            quoted(&mut json, message).unwrap();
            json.push_str(",\"labels\":[],\"notes\":[]}");
        }
        json.push(']');
        buffer.extend_from_slice(json.as_bytes());
        super::buffers::give(buffer, result);
    }
}

pub(super) unsafe fn source_error(source: u32, error: u32) -> String {
    unsafe { format!("{}: {}", span(crate::sources::telora_source_name(source)), span(error)) }
}
