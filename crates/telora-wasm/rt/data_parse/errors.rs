//! Error descriptors carry text for language Result and structured diagnostics
//! for service ABI consumers. Neither path registers request locations.
use alloc::{string::String, vec::Vec};
use core::fmt::Write;
use telora_data::source::{Diagnostic, LineIndex, Severity};
use super::{Origins, put};

pub(super) unsafe fn export(errors: Vec<Diagnostic>, input: &str, origins: &Origins, name: &str) -> u32 {
    let lines = LineIndex::new(input).expect("parser input coordinates");
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
                let start = lines.point(label.location.start);
                let end = lines.point(label.location.end);
                write!(&mut json, ",\"start\":{{\"line\":{},\"offset\":{}}},\"end\":{{\"line\":{},\"offset\":{}}}}},\"message\":", start >> 32, start as u32, end >> 32, end as u32).unwrap();
                crate::json_text::quoted(&mut json, &label.message).unwrap();
                write!(&mut json, ",\"primary\":{}}}", label.primary).unwrap();
            }
        }
        json.push_str("],\"notes\":[");
        let mut notes = error.notes.clone();
        if matches!(origins, Origins::Inherit(_)) {
            for label in &error.labels {
                let start = lines.point(label.location.start);
                let end = lines.point(label.location.end);
                notes.push(alloc::format!("input range (zero-based line/UTF-8 offset): {}:{}..{}:{}; {}", start >> 32, start as u32, end >> 32, end as u32, label.message));
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
