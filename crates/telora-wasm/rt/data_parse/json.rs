//! Publish retained text spans; external input buffers may be overwritten.
use super::{origins::Origins, put, string_bytes, span_bits};
use alloc::string::String;
use telora_data::{
    DataLimits, SourceDatabase,
    json::{self, JsonKind},
};

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
                super::errors::export(errors, origins, "<json string>")
            };
        }
    };
    let source_pointer = input.as_ptr() as u32;
    let decoded_pointer = string_bytes(ctx.into_decoded()).0;
    unsafe {
        let result = crate::telora_alloc(16);
        let count = u32::try_from(plan.nodes.len()).unwrap();
        let rows = crate::telora_alloc(count.checked_mul(24).unwrap());
        put(result, 0, rows);
        put(result, 4, count);
        put(result, 8, plan.root.index() as u32);
        put(result, 12, 0);
        for (index, node) in plan.nodes.into_iter().enumerate() {
            let row = rows + index as u32 * 24;
            origins.write(row + 4, node.location);
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
                    let entries = crate::telora_alloc(count.checked_mul(24).unwrap());
                    for (index, (key, field)) in fields.into_iter().enumerate() {
                        let entry = entries + index as u32 * 24;
                        let bits = span_bits(key, source_pointer, decoded_pointer);
                        put(entry, 0, bits as u32);
                        put(entry, 4, (bits >> 32) as u32);
                        put(entry, 8, field.value.index() as u32);
                        origins.write(entry + 12, field.key_location);
                    }
                    (7, u64::from(entries) | (u64::from(count) << 32))
                }
            };
            put(row, 0, kind);
            crate::heap::write(row + 16, payload);
        }
        result
    }
}
