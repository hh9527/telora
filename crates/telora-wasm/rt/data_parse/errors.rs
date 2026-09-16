//! Error descriptors carry text for language Result and structured diagnostics
//! for service ABI consumers. Neither path registers request locations.
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
        write!(&mut json, "{{\"severity\":\"{severity}\",\"message\":").unwrap();
        crate::json_text::quoted(&mut json, &error.message).unwrap();
        json.push_str(",\"labels\":[");
        if let Origins::Source { id, .. } = origins {
            for (index, label) in error.labels.iter().enumerate() {
                if index != 0 { json.push(','); }
                let source = unsafe {
                    let span = crate::sources::telora_source_name(*id);
                    super::error_text(span)
                };
                json.push_str("{\"location\":{\"source\":");
                crate::json_text::quoted(&mut json, source).unwrap();
                let start = unsafe { crate::sources::position(*id, label.location.start) };
                let end = unsafe { crate::sources::position(*id, label.location.end) };
                write!(&mut json, ",\"start\":{{\"line\":{},\"offset\":{}}},\"end\":{{\"line\":{},\"offset\":{}}}}},\"message\":", start.0, start.1, end.0, end.1).unwrap();
                crate::json_text::quoted(&mut json, &label.message).unwrap();
                write!(&mut json, ",\"primary\":{}}}", label.primary).unwrap();
            }
        }
        json.push_str("],\"notes\":[");
        let mut notes = error.notes.clone();
        if matches!(origins, Origins::Inherit(_)) {
            for label in &error.labels {
                notes.push(alloc::format!("input range (UTF-8 bytes): {}..{}; {}", label.location.start, label.location.end, label.message));
            }
        }
        for (index, note) in notes.iter().enumerate() {
            if index != 0 { json.push(','); }
            crate::json_text::quoted(&mut json, note).unwrap();
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
