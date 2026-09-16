//! Immutable tracing metadata, built before any language value is materialized.
use crate::artifact::{Kind, TypeDesc};

pub(super) fn encode(types: &[TypeDesc]) -> Result<Vec<u8>, String> {
    let mut image = vec![0; types.len().checked_mul(20).ok_or("Wasm: trace image overflow")?];
    for (index, ty) in types.iter().enumerate() {
        let kind = match ty.kind {
            Kind::Int | Kind::Float | Kind::Bool | Kind::Unit | Kind::Metadata => 0,
            Kind::String => 1,
            Kind::Bytes => 2,
            Kind::Record | Kind::Tuple => 3,
            Kind::Array => 4,
            Kind::Dict => 5,
            Kind::Enum | Kind::Option | Kind::Value => 6,
            Kind::Dyn => 7,
            Kind::Function => 8,
            Kind::Unsupported if ty.resource_table.is_some() => 9,
            Kind::Newtype => 10,
            Kind::Unsupported => u32::MAX,
        };
        let variants = u32::try_from(image.len()).map_err(|_| "Wasm: trace image overflow")?;
        let count = u32::try_from(ty.variants.len()).map_err(|_| "Wasm: trace variants overflow")?;
        for (offset, word) in [kind, ty.bytes, ty.resource_table.unwrap_or(u32::MAX), count, variants]
            .into_iter().enumerate()
        {
            image[index * 20 + offset * 4..index * 20 + offset * 4 + 4]
                .copy_from_slice(&word.to_le_bytes());
        }
        for variant in &ty.variants {
            image.extend_from_slice(&variant.ty.unwrap_or(u32::MAX).to_le_bytes());
            image.extend_from_slice(&u32::from(variant.boxed).to_le_bytes());
        }
    }
    Ok(image)
}
