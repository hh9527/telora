use crate::{
    abi::*,
    emit::Emitter,
    reflection_data::{MEMBER, ROW},
};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn reflect_collection(
        &mut self,
        name: &str,
        output: TypeId,
        input: u32,
        base: u32,
        row: u32,
    ) -> Result<u32, String> {
        let shape = &self.mir.types[output.index()];
        if shape.constructor != T::Array || shape.arguments.len() != 1 {
            return Err("Wasm: reflection collection is not Array".into());
        }
        let element = shape.arguments[0];
        let children = name == "children";
        if children && self.mir.types[element.index()].constructor != T::Type {
            return Err("Wasm: reflection children are not Type values".into());
        }
        if !children {
            let body = self.read32(row, 4);
            self.extend([
                I::LocalGet(body),
                I::I32Const(-1),
                I::I32Ne,
                I::If(BlockType::Empty),
                I::LocalGet(base),
                I::LocalGet(body),
                I::I32Const(ROW as i32),
                I::I32Mul,
                I::I32Add,
                I::LocalSet(row),
                I::End,
                I::LocalGet(row),
                I::I32Load(memory(0, 2)),
                I::I32Const(if name == "fields" { 10 } else { 12 }),
                I::I32Ne,
                I::If(BlockType::Empty),
            ]);
            self.reflection_failure(
                input,
                if name == "fields" {
                    "std/type-desc.fields expects Struct"
                } else {
                    "std/type-desc.variants expects Enum"
                },
            )?;
            self.emit(I::End);
        }
        let members = self.read32(row, if children { 8 } else { 16 });
        self.extend([
            I::LocalGet(base),
            I::LocalGet(members),
            I::I32Add,
            I::LocalSet(members),
        ]);
        let count = self.read32(row, if children { 12 } else { 20 });
        let width = self.width(element)?;
        let data = self.array_storage(count, width);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let item = self.array_item(members, index, if children { 4 } else { MEMBER });
        let value = if children {
            let ty = self.read32(item, 0);
            self.reflected_scalar(element, ty, input)?
        } else {
            self.reflected_member(name, element, input, base, item, index)?
        };
        let destination = self.array_item(data, index, width);
        self.copy(destination, 0, value, width);
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let result = self.array_result(output, data, count, width)?;
        self.copy(result, 0, input, LOC_BYTES);
        Ok(result)
    }
    fn reflected_member(
        &mut self,
        operation: &str,
        ty: TypeId,
        input: u32,
        base: u32,
        member: u32,
        index: u32,
    ) -> Result<u32, String> {
        let fields = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("Wasm: reflection descriptor has no record layout")?;
        let expected = if operation == "fields" {
            ["index", "name", "ty"]
        } else {
            ["index", "name", "payload"]
        };
        if fields.members.len() != 3
            || fields
                .members
                .iter()
                .zip(expected)
                .any(|(m, name)| m.name != name)
        {
            return Err("Wasm: reflection descriptor fields differ from admitted ABI".into());
        }
        let types = fields
            .members
            .iter()
            .map(|m| {
                m.type_id
                    .map(|id| self.plan.layouts[id].id())
                    .ok_or("Wasm: descriptor type missing")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if self.mir.types[types[0].index()].constructor != T::Int
            || self.mir.types[types[1].index()].constructor != T::String
        {
            return Err("Wasm: reflection descriptor scalar types mismatch".into());
        }
        let number = self.reflected_scalar(types[0], index, input)?;
        let name = self.reflected_text(types[1], base, member, 0, input)?;
        let payload = self.read32(member, 8);
        let value = if operation == "fields" {
            if self.mir.types[types[2].index()].constructor != T::Type {
                return Err("Wasm: field descriptor lacks Type".into());
            }
            self.reflected_scalar(types[2], payload, input)?
        } else {
            let option = &self.mir.types[types[2].index()];
            if option.constructor != T::Option
                || option.arguments.len() != 1
                || self.mir.types[option.arguments[0].index()].constructor != T::Type
            {
                return Err("Wasm: variant descriptor lacks Option(Type)".into());
            }
            let metadata = option.arguments[0];
            let result = self.local(ValType::I32);
            self.extend([
                I::LocalGet(payload),
                I::I32Const(-1),
                I::I32Ne,
                I::If(BlockType::Empty),
            ]);
            let value = self.reflected_scalar(metadata, payload, input)?;
            let some = self.enum_value(self.key.node, types[2], 1, Some(value))?;
            self.extend([I::LocalGet(some), I::LocalSet(result), I::Else]);
            let none = self.enum_value(self.key.node, types[2], 0, None)?;
            self.extend([I::LocalGet(none), I::LocalSet(result), I::End]);
            self.copy(result, 0, input, LOC_BYTES);
            result
        };
        let value = self.packed_tuple(ty, &[number, name, value])?;
        self.copy(value, 0, input, LOC_BYTES);
        Ok(value)
    }
}
