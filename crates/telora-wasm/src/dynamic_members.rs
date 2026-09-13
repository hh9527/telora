//! Explicit Dyn member access consumes offsets from the sealed type image.
use crate::{
    abi::*,
    emit::Emitter,
    reflection_data::{MEMBER, ROW},
};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn dynamic_field_value(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 3
            || self.mir.types[args[0].index()].constructor != T::Dyn
            || self.mir.types[args[1].index()].constructor != T::Int
            || args[2] != args[0]
        {
            return Err("Wasm: Dyn field access signature mismatch".into());
        }
        let input = self.parameter(0);
        let index = self.parameter(1);
        self.bits(index);
        self.extend([
            I::I64Const(u32::MAX as i64),
            I::I64GtU,
            I::If(BlockType::Empty),
        ]);
        self.reflection_failure(index, "Dyn member index must be a non-negative u32")?;
        self.emit(I::End);
        let id = self.read32(input, DATA);
        let (base, row) = self.type_row(id);
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
            I::I32Const(10),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        self.reflection_failure(input, "Dyn field access expects Struct")?;
        self.emit(I::End);
        let index = self.read32(index, DATA);
        self.extend([
            I::LocalGet(index),
            I::LocalGet(row),
            I::I32Load(memory(20, 2)),
            I::I32GeU,
            I::If(BlockType::Empty),
        ]);
        let message = self.local(ValType::I32);
        self.extend([
            I::I32Const(0),
            I::LocalGet(index),
            I::I32Const(0),
            I::Call(MEMBER_MESSAGE),
            I::LocalSet(message),
        ]);
        let message = self.text_span_value(self.string_type()?, message)?;
        let one = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(one)]);
        self.report(node, message, input, one, false);
        self.emit(I::End);
        let members = self.read32(row, 16);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(members),
            I::I32Add,
            I::LocalSet(members),
        ]);
        let member = self.array_item(members, index, MEMBER);
        let concrete = self.read32(member, 8);
        let offset = self.read32(member, 12);
        let width = self.read32(member, 16);
        let record = self.table_data(VALUES, input, 24);
        let data = self.table_data(RECORDS, record, DATA);
        self.extend([
            I::LocalGet(data),
            I::LocalGet(offset),
            I::I32Add,
            I::LocalSet(data),
        ]);
        let id = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(VALUES) as i32),
            I::LocalGet(data),
            I::LocalGet(width),
            I::Call(TABLE_PUSH),
            I::LocalSet(id),
        ]);
        let result = self.value_as(node, args[2], 40)?;
        self.copy(result, 0, data, 12);
        self.extend([
            I::LocalGet(result),
            I::LocalGet(concrete),
            I::I32Store(memory(DATA, 2)),
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(24, 3)),
        ]);
        self.store32(result, 20, 1);
        Ok(result)
    }
}
