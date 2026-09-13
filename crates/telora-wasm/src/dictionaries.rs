use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::mir::{HirId, Role, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn string_type(&self) -> Result<TypeId, String> {
        self.plan
            .layouts
            .iter()
            .find(|layout| self.mir.types[layout.type_id].constructor == T::String)
            .map(|layout| layout.id())
            .ok_or_else(|| "Wasm: sealed graph has no String identity".into())
    }
    pub fn dictionary_field(&mut self, node: HirId, name: &str) -> Result<u32, String> {
        let receiver_node = child(self.mir, node, Role::Receiver)?;
        let ty = self.effective_ty(receiver_node)?;
        let width = self.width(self.mir.types[ty.index()].arguments[0])?;
        let receiver = self.expression(receiver_node)?;
        let key = self.text_as(node, self.string_type()?, name.as_bytes())?;
        let result = self.dictionary_lookup(receiver, key, width);
        self.extend([I::LocalGet(result), I::I32Eqz]);
        self.fail_if(node, ERROR_KEY);
        Ok(result)
    }
    pub(crate) fn dictionary_lookup(&mut self, receiver: u32, key: u32, width: u32) -> u32 {
        let stride = self.local(ValType::I32);
        self.extend([I::I32Const(width as i32), I::LocalSet(stride)]);
        self.dictionary_lookup_stride(receiver, key, stride)
    }
    pub(crate) fn dictionary_lookup_stride(&mut self, receiver: u32, key: u32, stride: u32) -> u32 {
        let keys = self.table_data(ARRAYS, receiver, DATA);
        let values = self.table_data(ARRAYS, receiver, 24);
        let low = self.local(ValType::I32);
        let high = self.local(ValType::I32);
        let middle = self.local(ValType::I32);
        let comparison = self.local(ValType::I32);
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(receiver),
            I::I32Load(memory(20, 2)),
            I::LocalSet(high),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(low),
            I::LocalGet(high),
            I::I32GeU,
            I::BrIf(1),
            I::LocalGet(low),
            I::LocalGet(high),
            I::LocalGet(low),
            I::I32Sub,
            I::I32Const(1),
            I::I32ShrU,
            I::I32Add,
            I::LocalSet(middle),
            I::LocalGet(keys),
            I::LocalGet(middle),
            I::I32Const(32),
            I::I32Mul,
            I::I32Add,
            I::LocalGet(key),
            I::Call(STRING_COMPARE),
            I::LocalTee(comparison),
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::LocalGet(values),
            I::LocalGet(middle),
            I::LocalGet(stride),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(result),
            I::Br(2),
            I::End,
            I::LocalGet(comparison),
            I::I32Const(0),
            I::I32LtS,
            I::If(BlockType::Empty),
            I::LocalGet(middle),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(low),
            I::Else,
            I::LocalGet(middle),
            I::LocalSet(high),
            I::End,
            I::Br(0),
            I::End,
            I::End,
        ]);
        result
    }
}
