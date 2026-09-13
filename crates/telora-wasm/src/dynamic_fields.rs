//! Named observations share record fields and sorted dictionary columns.
use crate::{
    abi::*,
    emit::Emitter,
    reflection_data::{MEMBER, ROW},
};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn dynamic_fields(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 2 || self.mir.types[args[0].index()].constructor != T::Dyn {
            return Err("Wasm: Dyn fields signature mismatch".into());
        }
        let result_ty = args[1];
        let result = &self.mir.types[result_ty.index()];
        if result.constructor != T::Result
            || result.arguments.len() != 2
            || self.mir.types[result.arguments[1].index()].constructor != T::String
        {
            return Err("Wasm: Dyn fields requires Result".into());
        }
        let output = result.arguments[0];
        let array = &self.mir.types[output.index()];
        if array.constructor != T::Array || array.arguments.len() != 1 {
            return Err("Wasm: Dyn fields requires Array".into());
        }
        let pair = array.arguments[0];
        let string = self.string_type()?;
        if self.mir.types[pair.index()].constructor != T::Tuple
            || self.mir.types[pair.index()].arguments != [string, args[0]]
        {
            return Err("Wasm: Dyn fields pair mismatch".into());
        }
        let input = self.parameter(0);
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
        let error = self.enum_value(node, result_ty, 0, Some(message))?;
        self.extend([I::LocalGet(error), I::Return, I::End]);
        let value = self.table_data(VALUES, input, 24);
        let count = self.local(ValType::I32);
        let data = self.local(ValType::I32);
        let keys = self.local(ValType::I32);
        let child_ty = self.local(ValType::I32);
        let width = self.local(ValType::I32);
        self.extend([
            I::LocalGet(kind),
            I::I32Const(8),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let n = self.read32(value, 20);
        let k = self.table_data(ARRAYS, value, DATA);
        let d = self.table_data(ARRAYS, value, 24);
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
        self.extend([
            I::LocalGet(n),
            I::LocalSet(count),
            I::LocalGet(k),
            I::LocalSet(keys),
            I::LocalGet(d),
            I::LocalSet(data),
            I::LocalGet(ty),
            I::LocalSet(child_ty),
            I::LocalGet(w),
            I::LocalSet(width),
            I::Else,
        ]);
        let n = self.read32(row, 20);
        let members = self.read32(row, 16);
        self.extend([
            I::LocalGet(n),
            I::LocalSet(count),
            I::LocalGet(base),
            I::LocalGet(members),
            I::I32Add,
            I::LocalSet(keys),
            I::LocalGet(n),
            I::If(BlockType::Empty),
        ]);
        let d = self.table_data(RECORDS, value, DATA);
        self.extend([I::LocalGet(d), I::LocalSet(data), I::End, I::End]);
        let out_width = self.width(pair)?;
        let out = self.array_storage(count, out_width);
        let index = self.local(ValType::I32);
        let key = self.local(ValType::I32);
        let child = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
            I::LocalGet(kind),
            I::I32Const(8),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let k = self.array_item(keys, index, 32);
        self.extend([
            I::LocalGet(k),
            I::LocalSet(key),
            I::LocalGet(data),
            I::LocalGet(index),
            I::LocalGet(width),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(child),
            I::Else,
        ]);
        let member = self.array_item(keys, index, MEMBER);
        let k = self.reflected_text(string, base, member, 0, value)?;
        let ty = self.read32(member, 8);
        let offset = self.read32(member, 12);
        let w = self.read32(member, 16);
        self.extend([
            I::LocalGet(k),
            I::LocalSet(key),
            I::LocalGet(ty),
            I::LocalSet(child_ty),
            I::LocalGet(w),
            I::LocalSet(width),
            I::LocalGet(data),
            I::LocalGet(offset),
            I::I32Add,
            I::LocalSet(child),
            I::End,
        ]);
        let boxed = self.box_dynamic_pointer(args[0], child, child_ty, width)?;
        let item = self.packed_tuple(pair, &[key, boxed])?;
        let dest = self.array_item(out, index, out_width);
        self.copy(dest, 0, item, out_width);
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let array = self.array_result(output, out, count, out_width)?;
        self.enum_value(node, result_ty, 1, Some(array))
    }
}
