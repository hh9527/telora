//! Nominal decoding consumes sealed metadata identities and checker signatures.
use crate::{abi::*, emit::Emitter};
use telora_core::{
    candidate_layout::State,
    mir::{PropertySite, TypeId},
};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_decode_nominal(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let decode = self.codec_property_present(target, 1);
        let encode = self.codec_property_present(target, 2);
        self.extend([
            I::LocalGet(decode),
            I::LocalGet(encode),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        self.codec_error(
            input,
            "std/string.decode_by_parse and std/string.encode_by_display must be used together",
        )?;
        self.emit(I::End);
        self.extend([I::LocalGet(decode), I::If(BlockType::Empty)]);
        self.codec_property_value(target, 1)?;
        self.codec_property_value(target, 2)?;
        let parsed = self.codec_decode_text(source, target, input)?;
        self.extend([I::LocalGet(parsed), I::Return, I::End]);
        // Validate rename metadata even for a newtype, then decode its payload.
        let rename = self.codec_decode_rename(target, input)?;
        let untagged = self.codec_property_value(target, 5)?;
        let layout = &self.plan.layouts[target.index()];
        if !layout.variants.is_empty() {
            self.extend([I::LocalGet(untagged), I::If(BlockType::Empty)]);
            self.extend([I::LocalGet(rename), I::If(BlockType::Empty)]);
            self.codec_error(input, "rename_all is not meaningful on an untagged Enum")?;
            self.emit(I::End);
            let value = self.codec_decode_untagged(source, target, input)?;
            self.extend([I::LocalGet(value), I::Return, I::End]);
            self.extend([I::LocalGet(rename), I::If(BlockType::Empty)]);
            let value = self.codec_decode_enum(source, target, input, true)?;
            self.extend([I::LocalGet(value), I::Return, I::End]);
            return self.codec_decode_enum(source, target, input, false);
        }
        if matches!(&layout.layout, State::Known {shape} if shape.table == Some("RecordTable")) {
            self.extend([I::LocalGet(rename), I::If(BlockType::Empty)]);
            let renamed = self.codec_decode_record(source, target, input, true)?;
            self.extend([I::LocalGet(renamed), I::Return, I::End]);
            return self.codec_decode_record(source, target, input, false);
        }
        if !matches!(&layout.layout, State::Known {shape} if shape.table == Some("NewtypeTable")) {
            return Err("Wasm: codec nominal decode layout not yet implemented".into());
        }
        let members = &layout
            .object
            .as_ref()
            .ok_or("Wasm: newtype decode layout missing")?
            .members;
        if members.len() != 1 {
            return Err("Wasm: newtype decode must have one payload".into());
        }
        let inner = self.plan.layouts[members[0]
            .type_id
            .ok_or("Wasm: newtype payload type missing")?]
        .id();
        let decoded = self.codec_decode_call(source, inner, input, 0)?;
        self.parse_propagate(decoded);
        let error = self.read32(0, 28);
        self.construction_check_with_rejection(
            self.key.node,
            target,
            PropertySite::Type,
            decoded,
            Some(error),
        )?;
        let id = self.table_push(NEWTYPES, decoded, self.width(inner)?);
        let value = self.value_as(self.key.node, target, self.width(target)?)?;
        self.copy(value, 0, input, 12);
        self.extend([
            I::LocalGet(value),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(value)
    }

    pub(crate) fn codec_decode_rename(
        &mut self,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let selected = self.codec_property_value(target, 4)?;
        let rename = self.local(ValType::I32);
        self.extend([I::LocalGet(selected), I::If(BlockType::Empty)]);
        for property in &self.mir.properties {
            if property.owner != target || property.site != PropertySite::Type {
                continue;
            }
            let Some(object) = &self.plan.layouts[property.property.index()].object else {
                continue;
            };
            let Some(case) = object.members.iter().find(|m| m.name == "case") else {
                continue;
            };
            let ty = case.type_id.ok_or("Wasm: rename case type missing")?;
            let Some(camel) = self.plan.layouts[ty]
                .variants
                .iter()
                .position(|v| v.name == "CamelCase")
            else {
                continue;
            };
            let offset = case.offset.ok_or("Wasm: rename case offset missing")?;
            self.extend([
                I::LocalGet(0),
                I::I32Load(memory(16, 2)),
                I::I32Const(property.property.index() as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let data = self.table_data(RECORDS, selected, DATA);
            self.extend([
                I::LocalGet(data),
                I::I32Load(memory(offset + DATA, 2)),
                I::I32Const(camel as i32),
                I::I32Eq,
                I::LocalSet(rename),
                I::End,
            ]);
        }
        self.extend([I::LocalGet(rename), I::I32Eqz, I::If(BlockType::Empty)]);
        self.codec_error(input, "rename_all requires CamelCase")?;
        self.extend([I::End, I::End]);
        Ok(rename)
    }

    fn codec_decode_text(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let index = self.plan.layouts[source.index()]
            .variants
            .iter()
            .position(|v| v.name == "String")
            .ok_or("Wasm: codec Value lacks String")?;
        self.extend([
            I::LocalGet(input),
            I::I32Load(memory(DATA, 2)),
            I::I32Const(index as i32),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        self.codec_decode_reject("expected String text representation", input)?;
        self.emit(I::End);
        let text = self.enum_payload(source, index as u32, input)?;
        let property = self.value_as(
            self.key.node,
            self.mir
                .types
                .iter()
                .enumerate()
                .find(|(_, t)| t.constructor == telora_core::mir::TypeConstructor::Type)
                .map(|(id, _)| self.plan.layouts[id].id())
                .ok_or("Wasm: codec property Type identity missing")?,
            24,
        )?;
        self.extend([
            I::LocalGet(property),
            I::LocalGet(0),
            I::I32Load(memory(0, 2)),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        let path = self.read32(0, 24);
        let error = self.read32(0, 28);
        let context = self.alloc(24);
        for (offset, value) in [
            (0, property),
            (4, path),
            (8, error),
            (16, input),
            (20, error),
        ] {
            self.extend([
                I::LocalGet(context),
                I::LocalGet(value),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        self.store32(context, 12, 0);
        self.extend([
            I::LocalGet(error),
            I::LocalGet(input),
            I::I32Store(memory(4, 2)),
        ]);
        self.parse_call(target, context, text)
    }
}
