//! Immutable tracing metadata, built before any language value is materialized.
use crate::artifact::{Kind, TypeDesc};
use telora_wasm_shared::layout_image as layout;

pub(super) fn encode(types: &[TypeDesc]) -> Result<Vec<u8>, String> {
    let string = types
        .iter()
        .position(|ty| ty.kind == Kind::String)
        .and_then(|index| u32::try_from(index).ok())
        .ok_or("Wasm: closed layouts lack String")?;
    let mut image = vec![
        0;
        types
            .len()
            .checked_mul(layout::ENTRY_BYTES as usize)
            .ok_or("Wasm: trace image overflow")?
    ];
    for (index, ty) in types.iter().enumerate() {
        let kind = match ty.kind {
            Kind::Int | Kind::Float | Kind::Bool | Kind::Unit | Kind::Metadata => {
                layout::Kind::Scalar
            }
            Kind::String => layout::Kind::String,
            Kind::Bytes => layout::Kind::Bytes,
            Kind::Record | Kind::Tuple => layout::Kind::Record,
            Kind::Array => layout::Kind::Array,
            Kind::Dict => layout::Kind::Dict,
            Kind::Enum | Kind::Option | Kind::Value => layout::Kind::Enum,
            Kind::Dyn => layout::Kind::Dyn,
            Kind::Function => layout::Kind::Function,
            Kind::Unsupported if ty.resource_table.is_some() => layout::Kind::Resource,
            Kind::Newtype => layout::Kind::Newtype,
            Kind::Unsupported => layout::Kind::CompileTime,
        };
        let details = u32::try_from(image.len()).map_err(|_| "Wasm: layout image overflow")?;
        let count = match kind {
            layout::Kind::Record => ty.fields.len(),
            layout::Kind::Enum => ty.variants.len(),
            layout::Kind::Array => ty.arguments.len(),
            layout::Kind::Dict => ty.arguments.len() + 1,
            layout::Kind::Newtype => {
                if ty.fields.is_empty() {
                    ty.arguments.len()
                } else {
                    ty.fields.len()
                }
            }
            _ => 0,
        };
        let count = u32::try_from(count).map_err(|_| "Wasm: layout details overflow")?;
        for (offset, word) in [
            kind as u32,
            ty.bytes,
            ty.bytes.saturating_sub(crate::abi::HEADER_BYTES),
            8,
            ty.resource_table.unwrap_or(layout::NO_RESOURCE),
            details,
            count,
            0,
        ]
        .into_iter()
        .enumerate()
        {
            let base = index * layout::ENTRY_BYTES as usize;
            image[base + offset * 4..base + offset * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        match kind {
            layout::Kind::Record => {
                for field in &ty.fields {
                    detail(&mut image, Some(field.ty), field.offset, 0);
                }
            }
            layout::Kind::Enum => {
                for (tag, variant) in ty.variants.iter().enumerate() {
                    detail(
                        &mut image,
                        variant.ty,
                        tag as u32,
                        u32::from(variant.boxed) * layout::DETAIL_BOXED,
                    );
                }
            }
            layout::Kind::Array => {
                for (index, ty) in ty.arguments.iter().copied().enumerate() {
                    detail(&mut image, Some(ty), index as u32, 0);
                }
            }
            layout::Kind::Dict => {
                detail(&mut image, Some(string), 0, 0);
                for (index, ty) in ty.arguments.iter().copied().enumerate() {
                    detail(&mut image, Some(ty), index as u32 + 1, 0);
                }
            }
            layout::Kind::Newtype => {
                if ty.fields.is_empty() {
                    for (index, ty) in ty.arguments.iter().copied().enumerate() {
                        detail(&mut image, Some(ty), index as u32, 0);
                    }
                } else {
                    for field in &ty.fields {
                        detail(&mut image, Some(field.ty), field.offset, 0);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(image)
}

fn detail(image: &mut Vec<u8>, ty: Option<u32>, offset_or_tag: u32, flags: u32) {
    image.extend_from_slice(&ty.unwrap_or(layout::NO_TYPE).to_le_bytes());
    image.extend_from_slice(&offset_or_tag.to_le_bytes());
    image.extend_from_slice(&flags.to_le_bytes());
}
