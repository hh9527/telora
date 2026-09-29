//! Error descriptors carry text for language Result and structured diagnostics
//! for service ABI consumers. Dynamic input uses byte offsets, not an index.
use alloc::{string::String, vec::Vec};
use core::fmt::Write;
use telora_data::source::{Diagnostic, Severity};
use super::{Origins, put};

pub(super) unsafe fn export(errors: Vec<Diagnostic>, origins: &Origins, name: &str) -> u32 {
    let mut json = String::new();
    let mut summary = String::from(name);
    if !name.is_empty() { summary.push_str(": "); }
    for (index, error) in errors.iter().enumerate() {
        if index != 0 { json.push(','); summary.push('\n'); }
        summary.push_str(&error.message);
        let severity = match error.severity { Severity::Error => "Error", Severity::Warning => "Warning", Severity::Info => "Info" };
        let mut message = error.message.clone();
        if matches!(origins, Origins::Inherit(_)) {
            for loc in &error.locs {
                message.push_str(&alloc::format!("; input range (UTF-8 bytes): {}..{}", loc.start, loc.end));
            }
        }
        write!(&mut json, "{{\"severity\":\"{severity}\",\"message\":").unwrap();
        crate::json_text::quoted(&mut json, &message).unwrap();
        json.push_str(",\"locs\":[");
        if !matches!(origins, Origins::Inherit(_)) {
            let id = match origins { Origins::Source { id } => *id, Origins::Dynamic => 0, _ => unreachable!() };
            for (index, loc) in error.locs.iter().enumerate() {
                if index != 0 { json.push(','); }
                let source = unsafe {
                    let span = crate::sources::telora_source_name(id);
                    super::error_text(span)
                };
                json.push_str("{\"source\":");
                crate::json_text::quoted(&mut json, source).unwrap();
                let start = unsafe { crate::sources::position(id, loc.start) };
                let end = unsafe { crate::sources::position(id, loc.end) };
                write!(&mut json, ",\"start\":{{\"line\":{},\"offset\":{}}},\"end\":{{\"line\":{},\"offset\":{}}}}}", start.0, start.1, end.0, end.1).unwrap();
            }
        }
        json.push_str("]}");
    }
    unsafe {
        let result = crate::telora_alloc(16);
        let descriptor = crate::telora_alloc(16);
        let (text, length) = super::string_bytes(summary);
        put(descriptor, 0, text);
        put(descriptor, 4, length);
        let (text, length) = super::string_bytes(json);
        put(descriptor, 8, text);
        put(descriptor, 12, length);
        put(result, 12, descriptor);
        result
    }
}
