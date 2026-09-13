use crate::{abi::*, emit::Emitter};
use telora_core::{candidate_layout::State, mir::TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_encode_record(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let is_record = matches!(&self.plan.layouts[source.index()].layout, State::Known { shape } if shape.table == Some("RecordTable"));
        let is_enum = !self.plan.layouts[source.index()].variants.is_empty();
        let decode = self.codec_property_present(source, 1);
        let encode = self.codec_property_present(source, 2);
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
        self.extend([I::LocalGet(encode), I::If(BlockType::Empty)]);
        self.codec_property_value(source, 1)?;
        self.codec_property_value(source, 2)?;
        let displayed = self.codec_display(source, target, input)?;
        self.extend([I::LocalGet(displayed), I::Return, I::End]);
        for slot in [4] {
            if slot == 4 && (is_record || is_enum) {
                continue;
            }
            let present = self.codec_property_present(source, slot);
            self.extend([I::LocalGet(present), I::If(BlockType::Empty)]);
            let message = self.text_as(
                self.key.node,
                self.string_type()?,
                b"Wasm: codec property rule is not yet implemented",
            )?;
            let count = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(count)]);
            self.report(self.key.node, message, input, count, false);
            self.emit(I::End);
        }
        let untagged = self.codec_property_value(source, 5)?;
        if is_enum {
            self.extend([I::LocalGet(untagged), I::If(BlockType::Empty)]);
            let rename = self.codec_property_present(source, 4);
            self.extend([I::LocalGet(rename), I::If(BlockType::Empty)]);
            self.codec_error(input, "rename_all is not meaningful on an untagged Enum")?;
            self.emit(I::End);
            let result = self.codec_encode_untagged(source, target, input)?;
            self.extend([I::LocalGet(result), I::Return, I::End]);
        }
        let layout = &self.plan.layouts[source.index()];
        if matches!(&layout.layout, State::Known { shape } if shape.table == Some("NewtypeTable")) {
            let members = &layout
                .object
                .as_ref()
                .ok_or("Wasm: codec newtype layout missing")?
                .members;
            if members.len() != 1 {
                return Err("Wasm: codec newtype must have one payload".into());
            }
            let ty = self.plan.layouts[members[0]
                .type_id
                .ok_or("Wasm: codec newtype payload type missing")?]
            .id();
            let payload = self.table_data(NEWTYPES, input, DATA);
            return self.codec_encode_scalar(ty, target, payload);
        }
        if !is_record && !is_enum {
            return Err("Wasm: codec nominal type is not yet supported".into());
        }
        for (&index, &key) in &self.plan.properties {
            let property = &self.mir.properties[index];
            if property.owner != source || property.site != telora_core::mir::PropertySite::Type {
                continue;
            }
            let Some(object) = &self.plan.layouts[property.property.index()].object else {
                continue;
            };
            let Some(case) = object.members.iter().find(|m| m.name == "case") else {
                continue;
            };
            let case_ty = case.type_id.ok_or("Wasm: rename case type missing")?;
            let Some(camel) = self.plan.layouts[case_ty]
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
            let capability = self.call_key(key)?;
            let data = self.table_data(RECORDS, capability, DATA);
            self.extend([
                I::LocalGet(data),
                I::I32Load(memory(offset + DATA, 2)),
                I::I32Const(camel as i32),
                I::I32Ne,
            ]);
            self.fail_if(self.key.node, ERROR_DATA);
            let value = if is_enum {
                self.codec_encode_enum_names(source, target, input, true)?
            } else {
                self.codec_record_fields(source, target, input, true)?
            };
            self.extend([I::LocalGet(value), I::Return, I::End]);
        }
        if is_enum {
            self.codec_encode_enum(source, target, input)
        } else {
            self.codec_record_fields(source, target, input, false)
        }
    }

    fn codec_record_fields(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
        rename: bool,
    ) -> Result<u32, String> {
        let layout = &self.plan.layouts[source.index()];
        let mut members: Vec<_> = layout
            .object
            .as_ref()
            .ok_or("Wasm: codec Record layout missing")?
            .members
            .iter()
            .map(|m| (m.name.clone(), m.type_id, m.offset))
            .collect();
        let names =
            match crate::codec_names::external_names(members.iter().map(|m| m.0.clone()), rename) {
                Ok(names) => names,
                Err(_) => {
                    let message = self.text_as(
                        self.key.node,
                        self.string_type()?,
                        b"duplicate external field name",
                    )?;
                    let count = self.local(ValType::I32);
                    self.extend([I::I32Const(1), I::LocalSet(count)]);
                    self.report(self.key.node, message, input, count, false);
                    return Ok(self.local(ValType::I32));
                }
            };
        for (member, name) in members.iter_mut().zip(names) {
            member.0 = name;
        }
        members.sort_by(|a, b| a.0.cmp(&b.0));
        let width = self.width(target)?;
        let count =
            u32::try_from(members.len()).map_err(|_| "Wasm: codec Record field count overflow")?;
        let keys = self.alloc(
            count
                .checked_mul(32)
                .ok_or("Wasm: codec key size overflow")?,
        );
        let values = self.alloc(
            count
                .checked_mul(width)
                .ok_or("Wasm: codec value size overflow")?,
        );
        let base = self.table_data(RECORDS, input, DATA);
        for (index, (name, ty, offset)) in members.iter().enumerate() {
            let key = self.text_as(self.key.node, self.string_type()?, name.as_bytes())?;
            self.copy(key, 0, input, 12);
            self.copy(keys, index as u32 * 32, key, 32);
            let field = self.local(ValType::I32);
            self.extend([
                I::LocalGet(base),
                I::I32Const(offset.ok_or("Wasm: codec field offset missing")? as i32),
                I::I32Add,
                I::LocalSet(field),
            ]);
            let ty = self.plan.layouts[ty.ok_or("Wasm: codec field type missing")?].id();
            let value = self.codec_encode_scalar(ty, target, field)?;
            self.copy(values, index as u32 * width, value, width);
        }
        let payload_ty = self.plan.layouts[target.index()]
            .variants
            .iter()
            .find(|v| v.name == "Object")
            .and_then(|v| v.type_id)
            .ok_or("Wasm: codec Object payload missing")?;
        let len = self.local(ValType::I32);
        self.extend([I::I32Const(count as i32), I::LocalSet(len)]);
        let payload =
            self.dict_result(self.plan.layouts[payload_ty].id(), keys, values, len, width)?;
        self.copy(payload, 0, input, 12);
        self.codec_variant(target, "Object", Some(payload), input)
    }
}
