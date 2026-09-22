//! Fixed metadata slots carried through the generated codec call graph.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{PropertySite, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_rename(&mut self, target: TypeId, input: u32) -> Result<u32, String> {
        let rename = self.local(ValType::I32);
        if let Some((selected, field, option)) = self.codec_property_field(target, "rename_all")? {
            if self.mir.types[option.index()].constructor != T::Option {
                return Err("Wasm: CodecProp.rename_all must be Option".into());
            }
            let case = self.mir.types[option.index()].arguments[0];
            let some = self.plan.layouts[option.index()]
                .variants
                .iter()
                .position(|variant| variant.name == "Some")
                .ok_or("Wasm: CodecProp.rename_all Option lacks Some")?;
            let camel = self.plan.layouts[case.index()]
                .variants
                .iter()
                .position(|variant| variant.name == "CamelCase")
                .ok_or("Wasm: RenameCase lacks CamelCase")?;
            self.extend([
                I::LocalGet(selected),
                I::If(BlockType::Empty),
                I::LocalGet(field),
                I::I32Load(memory(DATA, 2)),
                I::I32Const(some as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let value = self.enum_payload(option, some as u32, field)?;
            self.extend([
                I::LocalGet(value),
                I::I32Load(memory(DATA, 2)),
                I::I32Const(camel as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
                I::I32Const(1),
                I::LocalSet(rename),
                I::Else,
            ]);
            self.codec_error(input, "rename_all requires CamelCase")?;
            self.extend([I::End, I::End, I::End]);
        }
        Ok(rename)
    }

    pub(crate) fn codec_untagged(&mut self, target: TypeId) -> Result<u32, String> {
        let enabled = self.local(ValType::I32);
        if let Some((selected, field, ty)) = self.codec_property_field(target, "untagged")? {
            if self.mir.types[ty.index()].constructor != T::Bool {
                return Err("Wasm: CodecProp.untagged must be Bool".into());
            }
            self.extend([
                I::LocalGet(selected),
                I::If(BlockType::Empty),
                I::LocalGet(field),
                I::I32Load(memory(DATA, 2)),
                I::LocalSet(enabled),
                I::End,
            ]);
        }
        Ok(enabled)
    }

    fn codec_property_field(
        &mut self,
        owner: TypeId,
        name: &str,
    ) -> Result<Option<(u32, u32, TypeId)>, String> {
        let property_id = self
            .plan
            .codec_property_type
            .ok_or("Wasm: codec property identity missing")?;
        let Some((offset, ty)) = self.mir.properties.iter().find_map(|property| {
            if property.owner != owner
                || property.property != property_id
                || property.site != PropertySite::Type
            {
                return None;
            }
            self.plan.layouts[property.property.index()]
                .object
                .as_ref()?
                .members
                .iter()
                .find(|member| member.name == name)
                .and_then(|member| Some((member.offset?, self.plan.layouts[member.type_id?].id())))
        }) else {
            return Ok(None);
        };
        let selected = self.codec_property_value_exact(owner, property_id)?;
        let field = self.local(ValType::I32);
        self.extend([I::LocalGet(selected), I::If(BlockType::Empty)]);
        let data = self.table_data(RECORDS, selected, DATA);
        self.extend([
            I::LocalGet(data),
            I::I32Const(offset as i32),
            I::I32Add,
            I::LocalSet(field),
            I::End,
        ]);
        Ok(Some((selected, field, ty)))
    }

    pub(crate) fn codec_property_value_exact(
        &mut self,
        owner: TypeId,
        selected: TypeId,
    ) -> Result<u32, String> {
        let key = self.plan.properties.iter().find_map(|(&index, &key)| {
            let property = &self.mir.properties[index];
            (property.owner == owner
                && property.property == selected
                && property.site == PropertySite::Type)
                .then_some(key)
        });
        self.call_key(key.ok_or("Wasm: sealed codec property slot is missing")?)
    }
}
