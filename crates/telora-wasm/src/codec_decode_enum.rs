//! Enum branch selection and checks use closed variant indices.
use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Special},
};
use telora_core::mir::{PropertySite, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_decode_variant(
        &mut self,
        source: TypeId,
        target: TypeId,
        index: u32,
    ) -> Result<u32, String> {
        let ty = self.plan.layouts[target.index()].variants[index as usize]
            .type_id
            .ok_or("Wasm: decode variant payload missing")?;
        let decoded = self.codec_decode_call(source, self.plan.layouts[ty].id(), 1, 0)?;
        self.parse_propagate(decoded);
        let error = self.read32(0, 28);
        self.construction_check_with_rejection(
            self.key.node,
            target,
            PropertySite::Variant(index),
            decoded,
            Some(error),
        )?;
        let value = self.enum_value_unchecked(self.key.node, target, index, Some(decoded))?;
        self.copy(value, 0, 1, LOC_BYTES);
        Ok(value)
    }

    pub(crate) fn codec_decode_variant_call(
        &mut self,
        source: TypeId,
        target: TypeId,
        index: u32,
        input: u32,
    ) -> Result<u32, String> {
        let key = Key {
            special: Special::DecodeVariant(source, target, index),
            callable: true,
            ..self.plan.root
        };
        let function = *self
            .plan
            .functions
            .get(&key)
            .ok_or("Wasm: decode variant was not planned")?;
        let value = self.local(ValType::I32);
        self.extend([
            I::LocalGet(0),
            I::LocalGet(input),
            I::Call(function),
            I::LocalSet(value),
        ]);
        Ok(value)
    }

    pub(crate) fn codec_decode_enum(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
        rename: bool,
    ) -> Result<u32, String> {
        let variants: Vec<_> = self.plan.layouts[target.index()]
            .variants
            .iter()
            .map(|v| (v.name.clone(), v.type_id))
            .collect();
        let names = match crate::codec_names::external_names(
            variants.iter().map(|v| v.0.clone()),
            rename,
        ) {
            Ok(names) => names,
            Err(_) => {
                self.codec_error(input, "duplicate external member name")?;
                return Ok(self.local(ValType::I32));
            }
        };
        let sources: Vec<_> = self.plan.layouts[source.index()]
            .variants
            .iter()
            .enumerate()
            .map(|(i, v)| (i, v.name.clone()))
            .collect();
        for (source_index, kind) in sources {
            if !matches!(
                kind.as_str(),
                "String"
                    | "Object"
                    | "LocalDate"
                    | "LocalTime"
                    | "LocalDateTime"
                    | "OffsetDateTime"
            ) {
                continue;
            }
            self.extend([
                I::LocalGet(input),
                I::I32Load(memory(DATA, 2)),
                I::I32Const(source_index as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let payload = self.enum_payload(source, source_index as u32, input)?;
            if kind == "Object" {
                self.extend([
                    I::LocalGet(payload),
                    I::I32Load(memory(DATA + 4, 2)),
                    I::I32Const(1),
                    I::I32Ne,
                    I::If(BlockType::Empty),
                ]);
                self.codec_decode_reject("expected a declared enum variant", input)?;
                self.emit(I::End);
            }
            for (index, (_, ty)) in variants.iter().enumerate() {
                if kind == "String" && ty.is_some() || kind != "String" && ty.is_none() {
                    continue;
                }
                let child = if kind == "Object" {
                    let key =
                        self.text_as(self.key.node, self.string_type()?, names[index].as_bytes())?;
                    let child = self.dictionary_lookup(payload, key, self.width(source)?);
                    self.extend([I::LocalGet(child), I::If(BlockType::Empty)]);
                    Some(child)
                } else if kind == "String" {
                    let key =
                        self.text_as(self.key.node, self.string_type()?, names[index].as_bytes())?;
                    self.extend([
                        I::LocalGet(payload),
                        I::LocalGet(key),
                        I::Call(STRING_COMPARE),
                        I::I32Eqz,
                        I::If(BlockType::Empty),
                    ]);
                    None
                } else {
                    if names[index] != kind {
                        continue;
                    }
                    self.extend([I::I32Const(1), I::If(BlockType::Empty)]);
                    Some(self.codec_variant(source, "String", Some(payload), input)?)
                };
                let value = if let Some(child) = child {
                    let value =
                        self.codec_decode_variant_call(source, target, index as u32, child)?;
                    self.parse_propagate(value);
                    value
                } else {
                    self.enum_value(self.key.node, target, index as u32, None)?
                };
                self.copy(value, 0, input, LOC_BYTES);
                self.extend([I::LocalGet(value), I::Return, I::End]);
            }
            self.emit(I::End);
        }
        self.codec_decode_reject("expected a declared enum variant", input)?;
        Ok(self.local(ValType::I32))
    }
}
