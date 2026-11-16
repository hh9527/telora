//! Import the public pointer-free packet into the existing internal Value plan.
use alloc::vec::Vec;
use telora_wasm_shared::{
    located_input::{InputValue, LocatedValue, Span},
    source_range::SourceRange,
};

fn range(span: Option<Span>) -> [u8; 8] {
    span.map(|s| SourceRange {
        source: 0,
        start: s.start,
        end: s.end,
    })
    .unwrap_or(SourceRange::NONE)
    .encode()
}
unsafe fn bytes(data: &[u8]) -> u32 {
    unsafe {
        let p = crate::telora_alloc(data.len().max(1) as u32);
        core::ptr::copy_nonoverlapping(data.as_ptr(), crate::heap::ptr::<u8>(p), data.len());
        p
    }
}
unsafe fn node(input: &LocatedValue, rows: &mut Vec<[u8; 24]>) -> u32 {
    unsafe {
        let (kind, payload): (u32, u64) = match &input.value {
            InputValue::Null => (0, 0),
            InputValue::Bool(v) => (if *v { 1 } else { 2 }, 0),
            InputValue::Int(n) => (3, *n as u64),
            InputValue::Float(n) => (4, n.to_bits()),
            InputValue::String(s) => (5, u64::from(bytes(s.as_bytes())) | ((s.len() as u64) << 32)),
            InputValue::Array(items) => {
                let mut entries = Vec::new();
                for item in items {
                    entries.extend_from_slice(&node(item, rows).to_le_bytes());
                }
                (6, u64::from(bytes(&entries)) | ((items.len() as u64) << 32))
            }
            InputValue::Object(fields) => {
                let mut entries = Vec::new();
                let mut ordered: Vec<_> = fields.iter().collect();
                ordered.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
                for field in ordered {
                    let id = node(&field.value, rows);
                    entries.extend_from_slice(&bytes(field.name.as_bytes()).to_le_bytes());
                    entries.extend_from_slice(&(field.name.len() as u32).to_le_bytes());
                    entries.extend_from_slice(&id.to_le_bytes());
                    entries.extend_from_slice(&range(field.span));
                    entries.extend_from_slice(&0u32.to_le_bytes());
                }
                (
                    7,
                    u64::from(bytes(&entries)) | ((fields.len() as u64) << 32),
                )
            }
        };
        let mut row = [0u8; 24];
        row[..4].copy_from_slice(&kind.to_le_bytes());
        row[4..12].copy_from_slice(&range(input.span));
        row[16..].copy_from_slice(&payload.to_le_bytes());
        let id = rows.len() as u32;
        rows.push(row);
        id
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_import_located(pointer: u32, length: u32) -> u32 {
    unsafe {
        let end = pointer
            .checked_add(length)
            .expect("located input pointer overflow");
        assert!(
            pointer != 0 && end <= crate::telora_heap_end(),
            "invalid located input buffer"
        );
        let data = core::slice::from_raw_parts(pointer as *const u8, length as usize);
        let input = match telora_wasm_shared::located_input::decode(data) {
            Ok(value) => value,
            Err(error) => return super::export_error(alloc::string::String::from(error)),
        };
        let mut rows = Vec::new();
        let root = node(&input, &mut rows);
        let data: Vec<u8> = rows.iter().flatten().copied().collect();
        let packet = crate::telora_alloc(16);
        super::put(packet, 0, bytes(&data));
        super::put(packet, 4, rows.len() as u32);
        super::put(packet, 8, root);
        super::put(packet, 12, 0);
        packet
    }
}
