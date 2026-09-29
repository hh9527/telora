//! Initialization diagnostics expand inline byte ranges inside Guest.
use alloc::string::String;
use core::fmt::Write;
use crate::{abi::*, json_text::quoted, tables::telora_table_get, values::word};

unsafe fn span(pointer: u32) -> &'static str {
    unsafe { core::str::from_utf8(core::slice::from_raw_parts(
        crate::heap::ptr::<u8>(word(pointer, 0)), word(pointer, 4) as usize)).unwrap() }
}

unsafe fn location(output: &mut String, id: u32) {
    unsafe {
        let record = crate::sources::telora_source_range(id);
        output.push_str("{\"source\":");
        quoted(output, span(crate::sources::telora_source_name(word(record, 0)))).unwrap();
        write!(output, ",\"start\":{{\"line\":{},\"offset\":{}}},\"end\":{{\"line\":{},\"offset\":{}}}}}",
            word(record, 4), word(record, 8), word(record, 12), word(record, 16)).unwrap();
    }
}

unsafe fn event(output: &mut String, pointer: u32) {
    unsafe {
        let severity = match word(pointer, DIAG_SEVERITY) {
            0 => "Error",
            1 => "Warning",
            2 => "Info",
            _ => "Error",
        };
        output.push_str("{\"severity\":");
        quoted(output, severity).unwrap();
        output.push_str(",\"message\":");
        let code = word(pointer, DIAG_CODE);
        let message = String::from(if code == 9 { crate::text::text(word(pointer, DIAG_MESSAGE)) }
            else { telora_wasm_shared::diagnostics::error_message(code) });
        quoted(output, &message).unwrap();
        output.push_str(",\"locs\":[");
        let origin: u64 = crate::heap::read(pointer);
        if origin != 0 {
            location(output, pointer);
        }
        let subjects = word(pointer, DIAG_SUBJECTS);
        for index in 0..word(pointer, DIAG_COUNT) {
            let pointer = subjects + index * LOC_BYTES;
            let id: u64 = crate::heap::read(pointer);
            if id == 0 || origin == 0 { continue; }
            output.push(',');
            location(output, pointer);
        }
        output.push_str("]}");
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
            json.push_str(",\"locs\":[]}");
        }
        let has_errors = count != 0 || !super::service().errors.is_empty();
        for (index, diagnostics) in super::service().parse_errors.iter().enumerate() {
            if has_errors || index != 0 { json.push(','); }
            json.push_str(diagnostics);
        }
        json.push(']');
        buffer.extend_from_slice(json.as_bytes());
        super::buffers::give(buffer, result);
    }
}
