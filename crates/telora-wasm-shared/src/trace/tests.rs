use super::*;
use alloc::{vec, vec::Vec};

struct Image {
    memory: Vec<u8>,
    edges: Vec<Edge>,
}
impl Image {
    fn new() -> Self {
        Self {
            memory: vec![0; 4096],
            edges: Vec::new(),
        }
    }
    fn put(&mut self, at: u32, value: u32) {
        self.memory[at as usize..at as usize + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn layout(&mut self, ty: u32, kind: layout::Kind, width: u32, details: u32, count: u32) {
        let base = ty * layout::ENTRY_BYTES;
        self.put(base, kind as u32);
        self.put(base + layout::VALUE_BYTES, width);
        self.put(base + layout::DETAIL_OFFSET, details);
        self.put(base + layout::DETAIL_COUNT, count);
    }
}
impl Visitor for Image {
    fn types(&self) -> u32 {
        0
    }
    fn read(&self, reference: u32) -> Result<u32, Error> {
        let at = reference as usize;
        Ok(u32::from_le_bytes(
            self.memory
                .get(at..at + 4)
                .ok_or(Error::Memory)?
                .try_into()
                .unwrap(),
        ))
    }
    fn edge(&mut self, edge: Edge) -> Result<(), Error> {
        self.edges.push(edge);
        Ok(())
    }
}

#[test]
fn inline_fields_use_owner_work_without_object_edges_or_native_recursion() {
    let mut image = Image::new();
    let count = 10_000u32;
    let details = (count + 1) * layout::ENTRY_BYTES;
    let values = details + count * layout::DETAIL_BYTES + 8;
    image
        .memory
        .resize((values + (count + 1) * INLINE_DATA) as usize, 0);
    for ty in 0..count {
        let detail = details + ty * layout::DETAIL_BYTES;
        image.layout(ty, layout::Kind::InlineRecord, 32, detail, 1);
        image.put(detail, ty + 1);
        image.put(values + ty * INLINE_DATA + HEADER_BYTES, INLINE_FIELDS);
    }
    image.layout(count, layout::Kind::Scalar, SCALAR_BYTES, 0, 0);
    let mut pending = vec![(values, 0)];
    let mut sources = 0;
    while let Some((old, ty)) = pending.pop() {
        value(&mut image, ty, old, old).unwrap();
        for edge in image.edges.drain(..) {
            match edge {
                Edge::Inline { old, at, ty } => {
                    assert_eq!(old, at);
                    pending.push((old, ty));
                }
                Edge::Source(_) => sources += 1,
                _ => panic!("plain embedded fields must not become GC objects"),
            }
        }
        assert!(pending.len() <= 1);
    }
    assert_eq!(sources, count + 1);
    image.put(values + HEADER_BYTES, 0);
    assert_eq!(value(&mut image, 0, values, values), Err(Error::Layout));
}

#[test]
fn scalar_bits_are_not_edges_and_dictionary_storage_is_typed_separately() {
    let mut image = Image::new();
    image.layout(0, layout::Kind::Scalar, 16, 0, 0);
    image.layout(1, layout::Kind::String, 24, 0, 0);
    image.layout(2, layout::Kind::Dict, 24, 128, 2);
    image.put(128, 1); // String keys.
    image.put(140, 0); // Scalar values.
    image.put(1032, 2048);
    image.put(1040, 3072);
    object(&mut image, VALUES, 1024, 2048, 16, 0).unwrap();
    assert_eq!(image.edges, [Edge::Source(1024)]);
    image.edges.clear();
    value(&mut image, 2, 1024, 2048).unwrap();
    assert_eq!(
        image.edges,
        [
            Edge::Source(1024),
            Edge::Object {
                table: ARRAYS,
                id: 2048,
                ty: 1,
                at: 2056
            },
            Edge::Object {
                table: ARRAYS,
                id: 3072,
                ty: 0,
                at: 2064
            },
        ]
    );
}

#[test]
fn enum_only_visits_active_payload_and_inline_origins_remain_packed_ranges() {
    let mut image = Image::new();
    image.layout(0, layout::Kind::Enum, 48, 128, 3);
    image.layout(1, layout::Kind::String, 24, 0, 0);
    image.put(128, layout::NO_TYPE);
    image.put(140, 1);
    image.put(152, 1);
    image.put(160, layout::DETAIL_BOXED);
    let origin = crate::source_range::SourceRange::checked(91, 110, 120, 120)
        .unwrap()
        .encode();
    image.memory[1040..1048].copy_from_slice(&origin);
    value(&mut image, 0, 1024, 2048).unwrap(); // Empty variant.
    assert_eq!(image.edges, [Edge::Source(1024)]);
    image.put(1032, 1);
    image.edges.clear();
    value(&mut image, 0, 1024, 2048).unwrap();
    assert_eq!(
        image.edges,
        [
            Edge::Source(1024),
            Edge::Source(1040),
            Edge::Content {
                old: 1048,
                at: 2072
            }
        ]
    );
    let source = match image.edges[1] {
        Edge::Source(p) => {
            crate::source_range::SourceRange::unpack(u64::from_le_bytes(
                image.memory[p as usize..p as usize + 8].try_into().unwrap(),
            ))
            .unwrap()
            .source
        }
        _ => unreachable!(),
    };
    assert_eq!(source, 91);
    image.put(1032, 2);
    image.put(1040, 3000);
    image.edges.clear();
    value(&mut image, 0, 1024, 2048).unwrap();
    assert_eq!(
        image.edges,
        [
            Edge::Source(1024),
            Edge::Object {
                table: VALUES,
                id: 3000,
                ty: 1,
                at: 2064
            }
        ]
    );
    image.put(1032, 3);
    assert_eq!(value(&mut image, 0, 1024, 2048), Err(Error::Layout));
}

#[test]
fn closure_uses_actual_capture_types_and_diagnostics_expose_storage_ownership() {
    let mut image = Image::new();
    image.layout(0, layout::Kind::Function, 16, 0, 0);
    image.put(1032, 71);
    image.put(1036, 3000);
    value(&mut image, 0, 1024, 2048).unwrap();
    assert_eq!(
        image.edges,
        [
            Edge::Source(1024),
            Edge::Function(71),
            Edge::Object {
                table: ENVIRONMENTS,
                id: 3000,
                ty: 0,
                at: 2060
            }
        ]
    );
    image.edges.clear();
    image.put(1024, 2);
    image.put(1032, 3000);
    image.put(1036, 3024);
    image.put(1040, 7);
    image.put(1044, 8);
    object(&mut image, ENVIRONMENTS, 1024, 2048, 24, 0).unwrap();
    assert_eq!(
        image.edges,
        [
            Edge::Value {
                reference: 3000,
                ty: 7,
                at: 2056
            },
            Edge::Value {
                reference: 3024,
                ty: 8,
                at: 2060
            }
        ]
    );
    image.edges.clear();
    image.put(1024 + DIAG_MESSAGE as u32, 3000);
    image.put(1024 + DIAG_MESSAGE_TYPE as u32, 7);
    image.put(1024 + DIAG_SUBJECTS as u32, 3500);
    image.put(1024 + DIAG_COUNT as u32, 2);
    object(&mut image, DIAGNOSTICS, 1024, 2048, DIAGNOSTIC_BYTES, 0).unwrap();
    assert_eq!(
        image.edges,
        [
            Edge::Source(1024),
            Edge::Value {
                reference: 3000,
                ty: 7,
                at: 2064
            },
            Edge::Storage {
                reference: 3500,
                bytes: 16,
                at: 2068
            },
            Edge::Source(3500),
            Edge::Source(3508)
        ]
    );
}

#[test]
fn deep_inline_paths_do_not_recurse_and_bad_sizes_are_rejected() {
    let mut image = Image::new();
    image.memory.resize(1024 + 100_000 * 16, 0);
    image.layout(0, layout::Kind::Enum, 48, 128, 2);
    image.put(128, 0); // Nested inline payload, consumed iteratively.
    image.put(140, layout::NO_TYPE);
    image.put(1024 + 99_999 * 16 + 8, 1);
    value(&mut image, 0, 1024, 1024).unwrap();
    assert_eq!(image.edges.len(), 100_000);
    image.layout(1, layout::Kind::Scalar, 16, 0, 0);
    assert_eq!(
        object(&mut image, ARRAYS, 1024, 1024, 17, 1),
        Err(Error::Layout)
    );
    assert_eq!(
        object(&mut image, ENVIRONMENTS, 1024, 1024, 9, 0),
        Err(Error::Layout)
    );
    assert_eq!(value(&mut image, 0, u32::MAX - 3, 0), Err(Error::Overflow));
}
