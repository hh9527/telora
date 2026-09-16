//! Publish retained text spans; external input buffers may be overwritten.
use super::{origins::Origins, put, string_bytes, span_bits};
use alloc::string::String;
use telora_data::{
    DataLimits, SourceDatabase,
    json::TemporalKind,
    toml::{self, TomlKind},
};

pub(super) unsafe fn parse(input: &str, origins: &Origins) -> u32 {
    let mut sources = SourceDatabase::default();
    // Dynamic parser errors carry messages; no source indexing is needed here.
    let source = sources
        .try_add_data("<toml string>", String::new())
        .expect("empty source");
    let (plan, ctx) = match toml::parse_structure(source, input, DataLimits::default())
        .and_then(toml::TomlStructure::validate)
    {
        Ok(plan) => plan,
        Err(errors) => {
            return unsafe {
                super::errors::export(errors, origins, "<toml string>")
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
                TomlKind::Bool(value) => (if value { 1 } else { 2 }, 0),
                TomlKind::Int(value) => (3, value as u64),
                TomlKind::Float(value) => (4, value.to_bits()),
                TomlKind::Temporal { kind, value } => {
                    let kind = match kind {
                        TemporalKind::LocalDate => 8,
                        TemporalKind::LocalTime => 9,
                        TemporalKind::LocalDateTime => 10,
                        TemporalKind::OffsetDateTime => 11,
                    };
                    (kind, span_bits(value, source_pointer, decoded_pointer))
                }
                TomlKind::String(span) => (5, span_bits(span, source_pointer, decoded_pointer)),
                TomlKind::Array(items) => {
                    let count = items.len() as u32;
                    let entries = crate::telora_alloc(count.checked_mul(4).unwrap());
                    for (index, id) in items.into_iter().enumerate() {
                        put(entries, index as u32 * 4, id.index() as u32);
                    }
                    (6, u64::from(entries) | (u64::from(count) << 32))
                }
                TomlKind::Object(fields) => {
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
            ((row + 16) as *mut u64).write_unaligned(payload);
        }
        result
    }
}
