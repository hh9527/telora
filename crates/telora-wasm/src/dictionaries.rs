use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::mir::{HirId, HirKind, Role, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    fn string_type(&self) -> Result<TypeId, String> {
        self.plan
            .layouts
            .iter()
            .find(|layout| self.mir.types[layout.type_id].constructor == T::String)
            .map(|layout| layout.id())
            .ok_or_else(|| "Wasm: sealed graph has no String identity".into())
    }
    pub fn dictionary(&mut self, node: HirId) -> Result<u32, String> {
        let ty = self.effective_ty(node)?;
        let element = self.mir.types[ty.index()].arguments[0];
        let width = self.width(element)?;
        let string = self.string_type()?;
        let mut fields = std::collections::BTreeMap::new();
        for edge in &self.mir.hir[node.index()].children {
            if edge.role != Role::Field {
                continue;
            }
            let name = child(self.mir, edge.node, Role::Name)?;
            let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                return Err("Wasm: missing dictionary key".into());
            };
            let expression = child(self.mir, edge.node, Role::Value)?;
            if self.effective_ty(expression)? != element {
                return Err("Wasm: dictionary value needs sealed adaptation".into());
            }
            let value = self.expression(expression)?;
            fields.insert(name.clone(), value);
        }
        let length = fields.len() as u32;
        let key_bytes = length
            .checked_mul(32)
            .ok_or("Wasm: dictionary size overflow")?;
        let value_bytes = length
            .checked_mul(width)
            .ok_or("Wasm: dictionary size overflow")?;
        let keys = self.alloc(key_bytes);
        let values = self.alloc(value_bytes);
        for (index, (name, value)) in fields.into_iter().enumerate() {
            let key = self.text_as(node, string, name.as_bytes())?;
            self.copy(keys, index as u32 * 32, key, 32);
            self.copy(values, index as u32 * width, value, width);
        }
        let keys_id = self.table_push(ARRAYS, keys, key_bytes);
        let values_id = self.table_push(ARRAYS, values, value_bytes);
        let result = self.value(node, 32)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(keys_id),
            I::I32Store(memory(DATA, 2)),
        ]);
        self.store32(result, 20, length);
        self.extend([
            I::LocalGet(result),
            I::LocalGet(values_id),
            I::I32Store(memory(24, 2)),
        ]);
        self.store32(result, 28, 0);
        Ok(result)
    }
    pub fn dictionary_field(&mut self, node: HirId, name: &str) -> Result<u32, String> {
        let receiver_node = child(self.mir, node, Role::Receiver)?;
        let ty = self.effective_ty(receiver_node)?;
        let width = self.width(self.mir.types[ty.index()].arguments[0])?;
        let receiver = self.expression(receiver_node)?;
        let key = self.text_as(node, self.string_type()?, name.as_bytes())?;
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
            I::I32Const(width as i32),
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
            I::LocalGet(result),
            I::I32Eqz,
        ]);
        self.fail_if(node, ERROR_KEY);
        Ok(result)
    }
}
