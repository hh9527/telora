//! Exact field lookup: binary search for Dict, closed member list for Record.
use crate::{
    abi::*,
    emit::Emitter,
    reflection_data::{MEMBER, ROW},
};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn dynamic_named_field(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        let string = self.string_type()?;
        if args.len() != 3
            || self.mir.types[args[0].index()].constructor != T::Dyn
            || args[1] != string
            || self.mir.types[args[2].index()].constructor != T::Result
            || self.mir.types[args[2].index()].arguments != [args[0], string]
        {
            return Err("Wasm: Dyn field signature mismatch".into());
        }
        let input = self.parameter(0);
        let key = self.parameter(1);
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
        ]);
        let kind = self.read32(row, 0);
        self.extend([
            I::LocalGet(kind),
            I::I32Const(8),
            I::I32Ne,
            I::LocalGet(kind),
            I::I32Const(10),
            I::I32Ne,
            I::I32And,
            I::If(BlockType::Empty),
        ]);
        let message = self.text_as(node, string, b"Dyn field access expects Struct")?;
        let error = self.enum_value(node, args[2], 0, Some(message))?;
        self.extend([I::LocalGet(error), I::Return, I::End]);
        let value = self.table_data(VALUES, input, 24);
        let child = self.local(ValType::I32);
        let child_ty = self.local(ValType::I32);
        let width = self.local(ValType::I32);
        self.extend([
            I::LocalGet(kind),
            I::I32Const(8),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let children = self.read32(row, 8);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(children),
            I::I32Add,
            I::LocalSet(children),
        ]);
        let ty = self.read32(children, 0);
        let (_, child_row) = self.type_row(ty);
        let w = self.read32(child_row, 32);
        let found = self.dictionary_lookup_stride(value, key, w);
        self.extend([
            I::LocalGet(ty),
            I::LocalSet(child_ty),
            I::LocalGet(w),
            I::LocalSet(width),
            I::LocalGet(found),
            I::LocalSet(child),
            I::Else,
        ]);
        let count = self.read32(row, 20);
        let members = self.read32(row, 16);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(members),
            I::I32Add,
            I::LocalSet(members),
        ]);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let member = self.array_item(members, index, MEMBER);
        let name = self.reflected_text(string, base, member, 0, value)?;
        self.extend([
            I::LocalGet(name),
            I::LocalGet(key),
            I::Call(STRING_COMPARE),
            I::I32Eqz,
            I::If(BlockType::Empty),
        ]);
        let ty = self.read32(member, 8);
        let w = self.read32(member, 16);
        let offset = self.read32(member, 12);
        let data = self.table_data(RECORDS, value, DATA);
        self.extend([
            I::LocalGet(ty),
            I::LocalSet(child_ty),
            I::LocalGet(w),
            I::LocalSet(width),
            I::LocalGet(data),
            I::LocalGet(offset),
            I::I32Add,
            I::LocalSet(child),
            I::Br(2),
            I::End,
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
            I::End,
            I::LocalGet(child),
            I::I32Eqz,
            I::If(BlockType::Empty),
        ]);
        let message = self.local(ValType::I32);
        self.extend([
            I::I32Const(2),
            I::LocalGet(key),
            I::I32Const(0),
            I::Call(MEMBER_MESSAGE),
            I::LocalSet(message),
        ]);
        let message = self.text_span_value(string, message)?;
        let error = self.enum_value(node, args[2], 0, Some(message))?;
        self.extend([I::LocalGet(error), I::Return, I::End]);
        let boxed = self.box_dynamic_pointer(args[0], child, child_ty, width)?;
        self.enum_value(node, args[2], 1, Some(boxed))
    }
}
