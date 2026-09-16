//! JSON publishes spans directly. Original text remains in the VM arena; all
//! escaped strings share a single decoded allocation with the same lifetime.
use super::{origins::Origins, put, string_bytes};
use alloc::string::String;
use telora_data::{
    DataLimits, SourceDatabase,
    json::{self, JsonKind, text::TextSpan},
};

fn span_bits(span: TextSpan, source: u32, decoded: u32) -> u64 {
    let (base, range) = match span {
        TextSpan::Source(range) => (source, range),
        TextSpan::Decoded(range) => (decoded, range),
    };
    u64::from(base + range.start as u32) | ((range.len() as u64) << 32)
}

pub(super) unsafe fn parse(input: &str, origins: &Origins) -> u32 {
    let mut sources = SourceDatabase::default();
    // Dynamic parser errors carry messages; no source indexing is needed here.
    let source = sources.try_add_data("<json string>", String::new()).expect("empty source");
    let (plan, ctx) = match json::parse_structure(source, input, DataLimits::default())
        .and_then(json::JsonStructure::validate)
    {
        Ok(plan) => plan,
        Err(errors) => {
            return unsafe {
                super::errors::export(errors, input, origins, "<json string>")
            };
        }
    };
    let source_pointer = input.as_ptr() as u32;
    let decoded_pointer = string_bytes(ctx.into_decoded()).0;
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
            put(row, 4, origins.at(node.location));
            let (kind, payload) = match node.kind {
                JsonKind::Null => (0, 0),
                JsonKind::Bool(value) => (if value { 1 } else { 2 }, 0),
                JsonKind::Int(value) => (3, value as u64),
                JsonKind::Float(value) => (4, value.to_bits()),
                JsonKind::String(span) => (5, span_bits(span, source_pointer, decoded_pointer)),
                JsonKind::Array(items) => {
                    let count = items.len() as u32;
                    let entries = crate::telora_alloc(count.checked_mul(4).unwrap());
                    for (index, id) in items.into_iter().enumerate() {
                        put(entries, index as u32 * 4, id.index() as u32);
                    }
                    (6, u64::from(entries) | (u64::from(count) << 32))
                }
                JsonKind::Object(fields) => {
                    let count = fields.len() as u32;
                    let entries = crate::telora_alloc(count.checked_mul(16).unwrap());
                    for (index, (key, field)) in fields.into_iter().enumerate() {
                        let entry = entries + index as u32 * 16;
                        let bits = span_bits(key, source_pointer, decoded_pointer);
                        put(entry, 0, bits as u32);
                        put(entry, 4, (bits >> 32) as u32);
                        put(entry, 8, field.value.index() as u32);
                        put(entry, 12, origins.at(field.key_location));
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
