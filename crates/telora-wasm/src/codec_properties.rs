//! Fixed metadata slots carried through the generated codec call graph.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{PropertySite, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

pub(crate) const PROPERTY_FIELDS: [&str; 6] = [
    "parse_by",
    "decode_by_parse",
    "encode_by_display",
    "display_by",
    "json_rename_all",
    "json_untagged",
];

impl Emitter<'_> {
    pub(crate) fn codec_property_value(
        &mut self,
        owner: TypeId,
        slot: usize,
    ) -> Result<u32, String> {
        let result = self.local(ValType::I32);
        for (&index, &key) in &self.plan.properties {
            let property = &self.mir.properties[index];
            if property.owner != owner || property.site != PropertySite::Type {
                continue;
            }
            self.extend([
                I::LocalGet(0),
                I::I32Load(memory(slot as u64 * 4, 2)),
                I::I32Const(property.property.index() as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let value = self.call_key(key)?;
            self.extend([I::LocalGet(value), I::LocalSet(result), I::End]);
        }
        Ok(result)
    }
    pub(crate) fn codec_property_present(&mut self, owner: TypeId, slot: usize) -> u32 {
        let result = self.local(ValType::I32);
        for property in &self.mir.properties {
            if property.owner != owner || property.site != PropertySite::Type {
                continue;
            }
            self.extend([
                I::LocalGet(result),
                I::LocalGet(0),
                I::I32Load(memory(slot as u64 * 4, 2)),
                I::I32Const(property.property.index() as i32),
                I::I32Eq,
                I::I32Or,
                I::LocalSet(result),
            ]);
        }
        result
    }
    pub(crate) fn codec_property_context(&mut self, ty: TypeId, value: u32) -> Result<u32, String> {
        let members = &self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("Wasm: codec Properties layout missing")?
            .members;
        let mut offsets = Vec::new();
        for name in PROPERTY_FIELDS {
            let member = members
                .iter()
                .find(|m| m.name == name)
                .ok_or_else(|| format!("Wasm: codec Properties lacks {name}"))?;
            let id = member
                .type_id
                .ok_or("Wasm: codec property field type missing")?;
            if self.mir.types[id].constructor != T::Type {
                return Err("Wasm: codec property field is not Type".into());
            }
            offsets.push(
                member
                    .offset
                    .ok_or("Wasm: codec property field offset missing")?,
            );
        }
        let base = self.table_data(RECORDS, value, DATA);
        let context = self.alloc(24);
        for (index, offset) in offsets.into_iter().enumerate() {
            self.extend([
                I::LocalGet(context),
                I::LocalGet(base),
                I::I32Load(memory(offset + DATA, 2)),
                I::I32Store(memory(index as u64 * 4, 2)),
            ]);
        }
        Ok(context)
    }
}
