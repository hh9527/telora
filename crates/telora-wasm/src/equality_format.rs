//! Fmt equality compares its fixed operation graph, not rendered text.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeId;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn compare_format(&mut self, ty: TypeId) -> Result<(), String> {
        let left = self.table_data(FORMATS, 0, DATA);
        let right = self.table_data(FORMATS, 1, DATA);
        self.extend([
            I::LocalGet(left),
            I::LocalGet(right),
            I::I32Eq,
            I::If(BlockType::Empty),
            I::I32Const(1),
            I::Return,
            I::End,
        ]);
        let operation = self.read32(left, 0);
        self.extend([
            I::LocalGet(operation),
            I::LocalGet(right),
            I::I32Load(memory(0, 2)),
            I::I32Ne,
        ]);
        self.unequal_if();
        let a = self.read32(left, 4);
        let b = self.read32(right, 4);
        self.extend([
            I::LocalGet(operation),
            I::I32Const(1),
            I::I32Eq,
            I::If(BlockType::Empty),
            I::LocalGet(a),
            I::LocalGet(b),
            I::Call(STRING_COMPARE),
            I::I32Eqz,
            I::Return,
            I::End,
        ]);
        self.extend([
            I::LocalGet(operation),
            I::I32Const(2),
            I::I32Eq,
            I::LocalGet(operation),
            I::I32Const(3),
            I::I32Eq,
            I::I32Or,
            I::If(BlockType::Empty),
        ]);
        // Format Float nodes use bit equality, including the sign of zero.
        self.bits(a);
        self.bits(b);
        self.extend([
            I::I64Eq,
            I::Return,
            I::End,
            I::LocalGet(operation),
            I::I32Const(4),
            I::I32Ne,
            I::If(BlockType::Empty),
            I::Unreachable,
            I::End,
        ]);
        let (strings_a, count_a) = self.array_parts(a, 32);
        let (strings_b, count_b) = self.array_parts(b, 32);
        self.extend([I::LocalGet(count_a), I::LocalGet(count_b), I::I32Ne]);
        self.unequal_if();
        let items_a = self.read32(left, 8);
        let items_b = self.read32(right, 8);
        let (items_a, items_count_a) = self.array_parts(items_a, SCALAR_BYTES);
        let (items_b, items_count_b) = self.array_parts(items_b, SCALAR_BYTES);
        self.extend([
            I::LocalGet(items_count_a),
            I::LocalGet(items_count_b),
            I::I32Ne,
        ]);
        self.unequal_if();
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count_a),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let a = self.array_item(strings_a, index, 32);
        let b = self.array_item(strings_b, index, 32);
        self.extend([I::LocalGet(a), I::LocalGet(b), I::Call(STRING_COMPARE)]);
        self.unequal_if();
        self.extend([
            I::LocalGet(index),
            I::LocalGet(items_count_a),
            I::I32LtU,
            I::If(BlockType::Empty),
        ]);
        let a = self.array_item(items_a, index, SCALAR_BYTES);
        let b = self.array_item(items_b, index, SCALAR_BYTES);
        self.compare_call(ty, a, b)?;
        self.emit(I::I32Eqz);
        self.unequal_if();
        self.extend([
            I::End,
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
            I::I32Const(1),
        ]);
        Ok(())
    }
}
