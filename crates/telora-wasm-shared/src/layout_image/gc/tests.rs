use super::*;
use crate::layout_image::*;

type Fields<'a> = &'a [(Option<u32>, u32)];

fn image(types: &[(Kind, Fields<'_>)]) -> Vec<u8> {
    let mut image = vec![0; types.len() * ENTRY_BYTES as usize];
    for (id, &(kind, fields)) in types.iter().enumerate() {
        let start = image.len() as u32;
        let words = [
            kind as u32,
            16,
            8,
            8,
            NO_RESOURCE,
            start,
            fields.len() as u32,
            0,
        ];
        for (index, word) in words.into_iter().enumerate() {
            let at = id * ENTRY_BYTES as usize + index * 4;
            image[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
        for &(ty, flags) in fields {
            for word in [ty.unwrap_or(NO_TYPE), 0, flags] {
                image.extend_from_slice(&word.to_le_bytes());
            }
        }
    }
    image
}

#[test]
fn scalar_aggregates_and_container_storage_have_distinct_classifications() {
    let bytes = image(&[
        (Kind::Scalar, &[]),
        (Kind::Record, &[(Some(0), 0)]),
        (Kind::String, &[]),
        (Kind::Record, &[(Some(1), 0), (Some(2), 0)]),
        (Kind::Array, &[(Some(0), 0)]),
        (Kind::Enum, &[(None, 0), (Some(3), 0)]),
        (Kind::Newtype, &[(Some(5), 0)]),
    ]);
    assert_eq!(classify(&bytes, 7).unwrap(), [0, 0, 1, 1, 1, 1, 1]);
}

#[test]
fn recursive_graph_reaches_fixed_point_without_inventing_container_fields() {
    let bytes = image(&[
        (Kind::Record, &[(Some(1), 0)]),
        (Kind::Enum, &[(Some(0), DETAIL_BOXED), (Some(2), 0)]),
        (Kind::Bytes, &[]),
        (Kind::Enum, &[(Some(3), DETAIL_BOXED)]),
        (Kind::Function, &[]),
        (Kind::Dyn, &[]),
        (Kind::Resource, &[]),
    ]);
    let flags = classify(&bytes, 7).unwrap();
    assert_eq!(flags[0], GC_REFERENCES | INTERNAL_REFERENCES);
    assert_eq!(flags[1], flags[0]);
    assert_eq!(flags[2], GC_REFERENCES);
    assert_eq!(flags[3], INTERNAL_REFERENCES);
    assert_eq!(flags[4], INTERNAL_REFERENCES | DYNAMIC_REFERENCES);
    assert_eq!(flags[5], flags[4]);
    assert_eq!(flags[6], INTERNAL_REFERENCES);
}

#[test]
fn rejects_dangling_type_edges_and_truncated_images() {
    let bytes = image(&[(Kind::Record, &[(Some(99), 0)])]);
    assert_eq!(classify(&bytes, 1), None);
    assert_eq!(classify(&bytes[..31], 1), None);
}
