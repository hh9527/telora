//! Flat, relocatable metadata. Every internal reference is image-relative.
use telora_core::{candidate_layout::Entry, mir::TypeConstructor as T, type_image::TypeImage};

pub const KINDS: [&str; 18] = [
    "Never", "Type", "TypeOf", "Int", "Float", "String", "Bytes", "Array", "Dict", "Tuple",
    "Struct", "Newtype", "Enum", "Func", "Opaque", "Bound", "Dyn", "Ref",
];
pub const ROW: u32 = 32;
pub const MEMBER: u32 = 20;

fn kind(ty: &T) -> Option<&'static str> {
    Some(match ty {
        T::Never => "Never",
        T::Type | T::Meta => "Type",
        T::TypeOf => "TypeOf",
        T::Int => "Int",
        T::Float => "Float",
        T::String => "String",
        T::Bytes => "Bytes",
        T::Array => "Array",
        T::Dict => "Dict",
        T::Tuple => "Tuple",
        T::Record(_) => "Struct",
        T::Newtype => "Newtype",
        T::Enum(_) | T::Bool | T::Option | T::Result | T::FoldControl | T::PropertyTarget => "Enum",
        T::Function => "Func",
        T::Native(_) => "Opaque",
        T::Parameter(_) | T::PropertyBound => "Bound",
        T::Dyn => "Dyn",
        T::Nominal(_) | T::Unchecked => "Ref",
        _ => return None,
    })
}
fn put(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn append(data: &mut Vec<u8>, bytes: &[u8]) -> Result<u32, String> {
    let offset = u32::try_from(data.len()).map_err(|_| "Wasm: type image too large")?;
    let end = data
        .len()
        .checked_add(bytes.len())
        .ok_or("Wasm: type image overflow")?;
    u32::try_from(end).map_err(|_| "Wasm: type image too large")?;
    data.extend_from_slice(bytes);
    while data.len() % 4 != 0 {
        data.push(0);
    }
    Ok(offset)
}
pub fn build(image: &TypeImage, layouts: &[Entry]) -> Result<Vec<u8>, String> {
    let bytes = image
        .types
        .len()
        .checked_mul(ROW as usize)
        .ok_or("Wasm: type image overflow")?;
    u32::try_from(bytes).map_err(|_| "Wasm: type image too large")?;
    let mut data = vec![0; bytes];
    for (index, ty) in image.types.iter().enumerate() {
        let row = index * ROW as usize;
        let tag = kind(&ty.constructor).and_then(|kind| KINDS.iter().position(|&k| k == kind));
        put(&mut data, row, tag.map_or(u32::MAX, |n| n as u32));
        put(
            &mut data,
            row + 4,
            image
                .layout(layouts[index].id())
                .map_or(u32::MAX, |l| l.body.index() as u32),
        );
        if matches!(
            ty.constructor,
            T::TypeOf
                | T::Array
                | T::Dict
                | T::Tuple
                | T::Record(_)
                | T::Newtype
                | T::Enum(_)
                | T::Option
                | T::Result
                | T::FoldControl
        ) {
            let children = ty
                .arguments
                .iter()
                .flat_map(|id| (id.index() as u32).to_le_bytes())
                .collect::<Vec<_>>();
            let offset = append(&mut data, &children)?;
            put(&mut data, row + 8, offset);
            put(&mut data, row + 12, ty.arguments.len() as u32);
        }
        let bool_members = ["False", "True"].map(|name| telora_core::candidate_layout::Member {
            name: name.into(),
            type_id: None,
            offset: None,
            storage: "none",
            table: None,
        });
        let members: &[_] = if ty.constructor == T::Bool {
            &bool_members
        } else if tag == Some(10) {
            layouts[index]
                .object
                .as_ref()
                .map(|o| o.members.as_slice())
                .unwrap_or(&[])
        } else if tag == Some(12) {
            &layouts[index].variants
        } else {
            &[]
        };
        let mut encoded = vec![0; members.len() * MEMBER as usize];
        for (i, member) in members.iter().enumerate() {
            let offset = append(&mut data, member.name.as_bytes())?;
            let row = i * MEMBER as usize;
            put(&mut encoded, row, offset);
            put(&mut encoded, row + 4, member.name.len() as u32);
            put(
                &mut encoded,
                row + 8,
                member.type_id.map_or(u32::MAX, |id| id as u32),
            );
            put(
                &mut encoded,
                row + 12,
                member.offset.map_or(u32::MAX, |offset| offset as u32),
            );
            put(
                &mut encoded,
                row + 16,
                match member.storage {
                    _ if tag == Some(10) => member
                        .type_id
                        .and_then(|id| match &layouts[id].layout {
                            telora_core::candidate_layout::State::Known { shape } => {
                                Some(shape.value_bytes as u32)
                            }
                            _ => None,
                        })
                        .unwrap_or(0),
                    "heap_id" => 2,
                    "full_value" => 3,
                    _ => 0,
                },
            );
        }
        let offset = append(&mut data, &encoded)?;
        put(&mut data, row + 16, offset);
        put(&mut data, row + 20, members.len() as u32);
        if let T::Native(id) = ty.constructor
            && let Some(name) = image.native_name(id)
        {
            let offset = append(&mut data, name.as_bytes())?;
            put(&mut data, row + 24, offset);
            put(&mut data, row + 28, name.len() as u32);
        }
    }
    Ok(data)
}
