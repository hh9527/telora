//! YAML publishes spans directly. Original text remains in the VM arena; all
//! escaped strings share a single decoded allocation with the same lifetime.
use super::{put, string_bytes};
use alloc::{format, string::String};
use telora_data::{
    DataLimits, SourceDatabase,
    json::text::TextSpan,
    yaml::{self, YamlKind},
};

fn span_bits(span: TextSpan, source: u32, decoded: u32) -> u64 {
    let (base, range) = match span {
        TextSpan::Source(range) => (source, range),
        TextSpan::Decoded(range) => (decoded, range),
    };
    u64::from(base + range.start as u32) | ((range.len() as u64) << 32)
}

pub(super) unsafe fn parse(input: &str) -> u32 {
    let mut sources = SourceDatabase::default();
    // Dynamic parser errors carry messages; no source indexing is needed here.
    let source = sources
        .try_add_data("<yaml string>", String::new())
        .expect("empty source");
    let (plan, ctx) = match yaml::parse_structure(source, input, DataLimits::default())
        .and_then(yaml::YamlStructure::validate)
    {
        Ok(plan) => plan,
        Err(errors) => {
            return unsafe {
                super::export_plan(Err(format!(
                    "<yaml string>: {}",
                    errors
                        .into_iter()
                        .map(|error| error.message)
                        .collect::<alloc::vec::Vec<_>>()
                        .join("\n")
                )))
            };
        }
    };
    let source_pointer = input.as_ptr() as u32;
    let (decoded, bytes) = ctx.into_decoded();
    let decoded_pointer = string_bytes(decoded).0;
    let bytes_pointer = alloc::boxed::Box::leak(bytes.into_boxed_slice()).as_ptr() as u32;
    unsafe {
        let result = crate::telora_alloc(16);
        let count = u32::try_from(plan.nodes.len()).unwrap();
        let rows = crate::telora_alloc(count.checked_mul(16).unwrap());
        put(result, 0, rows);
        put(result, 4, count);
        put(result, 8, plan.root.index() as u32);
        put(result, 12, 0);
        for (index, node) in plan.nodes.into_iter().enumerate() {
            let row = rows + index as u32 * 16;
            put(row, 4, 0);
            let (kind, payload) = match node.kind {
                YamlKind::Null => (0, 0),
                YamlKind::Bool(value) => (if value { 1 } else { 2 }, 0),
                YamlKind::Int(value) => (3, value as u64),
                YamlKind::Float(value) => (4, value.to_bits()),
                YamlKind::Bytes(range) => (
                    12,
                    u64::from(bytes_pointer + range.start as u32) | ((range.len() as u64) << 32),
                ),
                YamlKind::String(span) => (5, span_bits(span, source_pointer, decoded_pointer)),
                YamlKind::Array(items) => {
                    let count = items.len() as u32;
                    let entries = crate::telora_alloc(count.checked_mul(4).unwrap());
                    for (index, id) in items.into_iter().enumerate() {
                        put(entries, index as u32 * 4, id.index() as u32);
                    }
                    (6, u64::from(entries) | (u64::from(count) << 32))
                }
                YamlKind::Object(fields) => {
                    let count = fields.len() as u32;
                    let entries = crate::telora_alloc(count.checked_mul(12).unwrap());
                    for (index, (key, field)) in fields.into_iter().enumerate() {
                        let entry = entries + index as u32 * 12;
                        let bits = span_bits(key, source_pointer, decoded_pointer);
                        put(entry, 0, bits as u32);
                        put(entry, 4, (bits >> 32) as u32);
                        put(entry, 8, field.value.index() as u32);
                    }
                    (7, u64::from(entries) | (u64::from(count) << 32))
                }
            };
            put(row, 0, kind);
            ((row + 8) as *mut u64).write_unaligned(payload);
        }
        result
    }
}
