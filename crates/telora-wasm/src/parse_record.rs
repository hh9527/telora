use crate::{abi::*, emit::Emitter};
use telora_core::{
    candidate_layout::State,
    mir::{PropertySite, TypeConstructor as T, TypeId},
};
use wasm_encoder::{BlockType, Instruction as I, ValType};
impl Emitter<'_> {
    pub(crate) fn parse_record(&mut self, target: TypeId) -> Result<u32, String> {
        let node = self.key.node;
        let layout = &self.plan.layouts[target.index()];
        if !matches!(&layout.layout, State::Known {shape} if shape.table == Some("RecordTable"))
            || !matches!(
                self.mir.types[target.index()].constructor,
                T::Nominal(_) | T::Record(_)
            )
        {
            self.parse_reject("type has no std/string.parse capability")?;
            return Ok(1);
        }
        let members = &layout
            .object
            .as_ref()
            .ok_or("Wasm: parse record layout missing")?
            .members;
        let property = self.read32(0, 0);
        for (&index, &key) in &self.plan.properties {
            let record = &self.mir.properties[index];
            if record.owner != target || record.site != PropertySite::Type {
                continue;
            }
            let Some(property_layout) = &self.plan.layouts[record.property.index()].object else {
                continue;
            };
            let Some(regex_field) = property_layout.members.iter().find(|field| field.name == "regex"
                && field.type_id.is_some_and(|ty| matches!(self.mir.types[ty].constructor, T::Native(id) if (id.module,id.slot)==(19,0)))) else { continue; };
            self.bits(property);
            self.extend([
                I::I64Const(record.property.index() as i64),
                I::I64Eq,
                I::If(BlockType::Empty),
            ]);
            let capability = self.call_key(key)?;
            let object = self.table_data(RECORDS, capability, DATA);
            let regex = self.local(ValType::I32);
            self.extend([
                I::LocalGet(object),
                I::I32Const(
                    regex_field
                        .offset
                        .ok_or("Wasm: regex field offset missing")? as i32,
                ),
                I::I32Add,
                I::LocalSet(regex),
            ]);
            let regex_id = self.read32(regex, DATA);
            let contract = self.regex_contract_packet(target, property)?;
            let error = self.local(ValType::I32);
            self.extend([
                I::I32Const(3),
                I::LocalGet(regex_id),
                I::LocalGet(contract),
                I::Call(REGEX),
                I::LocalSet(error),
                I::LocalGet(error),
                I::If(BlockType::Empty),
            ]);
            let message = self.text_span_value(self.string_type()?, error)?;
            self.parse_reject_value(message)?;
            self.emit(I::End);
            let packet = self.alloc(8 + members.len() as u32 * 4);
            self.extend([
                I::LocalGet(packet),
                I::LocalGet(1),
                I::I32Store(memory(0, 2)),
            ]);
            self.store32(packet, 4, members.len() as u32);
            let mut names = Vec::new();
            for (index, member) in members.iter().enumerate() {
                let name = self.text_as(node, self.string_type()?, member.name.as_bytes())?;
                self.extend([
                    I::LocalGet(packet),
                    I::LocalGet(name),
                    I::I32Store(memory(8 + index as u64 * 4, 2)),
                ]);
                names.push(name);
            }
            let captures = self.local(ValType::I32);
            self.extend([
                I::I32Const(4),
                I::LocalGet(regex_id),
                I::LocalGet(packet),
                I::Call(REGEX),
                I::LocalSet(captures),
                I::LocalGet(captures),
                I::I32Load(memory(0, 2)),
                I::I32Eqz,
                I::If(BlockType::Empty),
            ]);
            self.parse_reject("input does not match regular expression")?;
            self.emit(I::End);
            let path = self.read32(0, 4);
            let mut fields = Vec::new();
            for (index, member) in members.iter().enumerate() {
                let span = self.local(ValType::I32);
                self.extend([
                    I::LocalGet(captures),
                    I::I32Const(4 + index as i32 * 12),
                    I::I32Add,
                    I::LocalSet(span),
                ]);
                let input = self.local(ValType::I32);
                let present = self.read32(span, 8);
                self.extend([I::LocalGet(present), I::If(BlockType::Empty)]);
                let start = self.read32(span, 0);
                let length = self.read32(span, 4);
                let text = self.byte_slice_value(self.string_type()?, 1, start, length)?;
                self.copy(text, 0, 1, LOC_BYTES);
                self.extend([I::LocalGet(text), I::LocalSet(input), I::End]);
                let path = self.parse_text(7, path, names[index])?;
                let context = self.parse_context(path);
                let ty = self.plan.layouts
                    [member.type_id.ok_or("Wasm: parse field identity missing")?]
                .id();
                let value = self.parse_call(ty, context, input)?;
                self.parse_propagate(value);
                fields.push(value);
            }
            let value = self.packed_tuple(target, &fields)?;
            self.copy(value, 0, 1, LOC_BYTES);
            let rejection = self.read32(0, 20);
            self.construction_check_with_rejection(
                node,
                target,
                PropertySite::Type,
                value,
                Some(rejection),
            )?;
            self.extend([I::LocalGet(value), I::Return, I::End]);
        }
        self.parse_reject("type has no std/string.parse capability")?;
        Ok(1)
    }
}
