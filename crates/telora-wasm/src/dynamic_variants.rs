//! Variant identities and payload storage come from the sealed metadata image.
use crate::{
    abi::*,
    emit::Emitter,
    reflection_data::{MEMBER, ROW},
};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn box_dynamic_pointer(
        &mut self,
        ty: TypeId,
        data: u32,
        concrete: u32,
        width: u32,
    ) -> Result<u32, String> {
        let id = self.local(ValType::I32);
        self.extend([
            I::LocalGet(width),
            I::I32Const(8),
            I::I32Add,
            I::Call(ALLOC),
            I::LocalSet(id),
            I::LocalGet(id),
            I::LocalGet(concrete),
            I::I32Store(memory(0, 2)),
            I::LocalGet(id),
            I::LocalGet(width),
            I::I32Store(memory(4, 2)),
            I::LocalGet(id),
            I::I32Const(8),
            I::I32Add,
            I::LocalGet(data),
            I::LocalGet(width),
            I::MemoryCopy { src_mem: 0, dst_mem: 0 },
        ]);
        let result = self.value_as(self.key.node, ty, DYN_BYTES)?;
        self.copy(result, 0, data, LOC_BYTES);
        self.extend([
            I::LocalGet(result),
            I::LocalGet(concrete),
            I::I32Store(memory(DATA, 2)),
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA + 8, 3)),
        ]);
        self.store32(result, DATA + 4, 1);
        Ok(result)
    }
    pub fn dynamic_variant(&mut self, name: &str) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        let checked = matches!(name, "tag_raw" | "payload_raw");
        let payload = matches!(name, "get_variant_payload" | "payload_raw");
        let indexed = name == "get_variant_payload";
        let arity = if indexed { 2 } else { 1 };
        if args.len() != arity + 1
            || self.mir.types[args[0].index()].constructor != T::Dyn
            || (indexed && self.mir.types[args[1].index()].constructor != T::Int)
        {
            return Err("Wasm: Dyn variant argument mismatch".into());
        }
        let result_type = args[arity];
        let output = if checked {
            let shape = &self.mir.types[result_type.index()];
            if shape.constructor != T::Result
                || shape.arguments.len() != 2
                || self.mir.types[shape.arguments[1].index()].constructor != T::String
            {
                return Err("Wasm: Dyn query result mismatch".into());
            }
            shape.arguments[0]
        } else {
            result_type
        };
        let shape = &self.mir.types[output.index()];
        if if payload {
            shape.constructor != T::Option || shape.arguments != [args[0]]
        } else if checked {
            shape.constructor != T::String
        } else {
            shape.constructor != T::Int
        } {
            return Err("Wasm: Dyn variant result mismatch".into());
        }
        let input = self.parameter(0);
        let expected = if indexed {
            let value = self.parameter(1);
            self.bits(value);
            self.extend([
                I::I64Const(u32::MAX as i64),
                I::I64GtU,
                I::If(BlockType::Empty),
            ]);
            self.reflection_failure(value, "Dyn member index must be a non-negative u32")?;
            self.emit(I::End);
            Some(self.read32(value, DATA))
        } else {
            None
        };
        let concrete = self.read32(input, DATA);
        let (base, row) = self.type_row(concrete);
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
            I::I32Const(12),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        if checked {
            let message = self.text_as(
                node,
                self.string_type()?,
                b"Dyn variant access expects Enum",
            )?;
            let error = self.enum_value(node, result_type, 0, Some(message))?;
            self.extend([I::LocalGet(error), I::Return]);
        } else {
            self.reflection_failure(input, "Dyn variant access expects Enum")?;
        }
        self.emit(I::End);
        let value = self.table_data(VALUES, input, DATA + 8);
        let tag = self.read32(value, DATA);
        if !payload && !checked {
            return self.reflected_scalar(output, tag, value);
        }
        if let Some(expected) = expected {
            self.extend([
                I::LocalGet(tag),
                I::LocalGet(expected),
                I::I32Ne,
                I::If(BlockType::Empty),
            ]);
            let message = self.local(ValType::I32);
            self.extend([
                I::I32Const(1),
                I::LocalGet(tag),
                I::LocalGet(expected),
                I::Call(MEMBER_MESSAGE),
                I::LocalSet(message),
            ]);
            let message = self.text_span_value(self.string_type()?, message)?;
            let one = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(one)]);
            self.report(node, message, input, one, false);
            self.emit(I::End);
        }
        let members = self.read32(row, 16);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(members),
            I::I32Add,
            I::LocalSet(members),
        ]);
        let member = self.array_item(members, tag, MEMBER);
        if name == "tag_raw" {
            let text = self.reflected_text(output, base, member, 0, input)?;
            return self.enum_value(node, result_type, 1, Some(text));
        }
        let concrete = self.read32(member, 8);
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(concrete),
            I::I32Const(-1),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let none = self.enum_value(node, output, 0, None)?;
        self.extend([I::LocalGet(none), I::LocalSet(result), I::Else]);
        let storage = self.read32(member, 16);
        let child = self.local(ValType::I32);
        self.extend([
            I::LocalGet(storage),
            I::I32Const(2),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let boxed = self.table_data(VALUES, value, DATA + 8);
        self.extend([
            I::LocalGet(boxed),
            I::LocalSet(child),
            I::Else,
            I::LocalGet(storage),
            I::I32Const(3),
            I::I32Ne,
            I::If(BlockType::Empty),
            I::Unreachable,
            I::End,
            I::LocalGet(value),
            I::I32Const((DATA + 8) as i32),
            I::I32Add,
            I::LocalSet(child),
            I::End,
        ]);
        let (_, child_row) = self.type_row(concrete);
        let width = self.read32(child_row, 32);
        let boxed = self.box_dynamic_pointer(args[0], child, concrete, width)?;
        let some = self.enum_value(node, output, 1, Some(boxed))?;
        self.extend([I::LocalGet(some), I::LocalSet(result), I::End]);
        if checked {
            self.enum_value(node, result_type, 1, Some(result))
        } else {
            Ok(result)
        }
    }
}
