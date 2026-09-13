//! Sequence observations box immutable element references using sealed widths.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn dynamic_array_items(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 2 || self.mir.types[args[0].index()].constructor != T::Dyn {
            return Err("Wasm: Dyn array query signature mismatch".into());
        }
        let result_ty = args[1];
        let result = &self.mir.types[result_ty.index()];
        if result.constructor != T::Result
            || result.arguments.len() != 2
            || self.mir.types[result.arguments[1].index()].constructor != T::String
        {
            return Err("Wasm: Dyn array query requires Result(Array(Dyn), String)".into());
        }
        let output = result.arguments[0];
        let array = &self.mir.types[output.index()];
        if array.constructor != T::Array || array.arguments != [args[0]] {
            return Err("Wasm: Dyn array query element mismatch".into());
        }
        let input = self.parameter(0);
        let concrete = self.read32(input, DATA);
        let (base, row) = self.type_row(concrete);
        let kind = self.read32(row, 0);
        self.extend([
            I::LocalGet(kind),
            I::I32Const(7),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        let message = self.text_as(
            node,
            self.string_type()?,
            b"Dyn sequence access has the wrong type",
        )?;
        let error = self.enum_value(node, result_ty, 0, Some(message))?;
        self.extend([I::LocalGet(error), I::Return, I::End]);
        let children = self.read32(row, 8);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(children),
            I::I32Add,
            I::LocalSet(children),
        ]);
        let child_ty = self.read32(children, 0);
        let (_, child_row) = self.type_row(child_ty);
        let width = self.read32(child_row, 32);
        let value = self.table_data(VALUES, input, 24);
        let data = self.table_data(ARRAYS, value, DATA);
        let start = self.read32(value, 20);
        let count = self.read32(value, 24);
        self.extend([
            I::LocalGet(count),
            I::LocalGet(start),
            I::I32Sub,
            I::LocalSet(count),
            I::LocalGet(data),
            I::LocalGet(start),
            I::LocalGet(width),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(data),
        ]);
        let out_width = self.width(args[0])?;
        let out_data = self.array_storage(count, out_width);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let child = self.local(ValType::I32);
        self.extend([
            I::LocalGet(data),
            I::LocalGet(index),
            I::LocalGet(width),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(child),
        ]);
        let boxed = self.box_dynamic_pointer(args[0], child, child_ty, width)?;
        let destination = self.array_item(out_data, index, out_width);
        self.copy(destination, 0, boxed, out_width);
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let array = self.array_result(output, out_data, count, out_width)?;
        self.enum_value(node, result_ty, 1, Some(array))
    }
}
