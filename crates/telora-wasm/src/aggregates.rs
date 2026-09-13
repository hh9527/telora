use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::{
    candidate_layout::State,
    mir::{HirId, HirKind, Role, TypeConstructor as T, TypeId},
};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    pub fn effective_ty(&self, node: HirId) -> Result<TypeId, String> {
        self.key.effective_ty(self.mir, node)
    }
    pub fn width(&self, ty: TypeId) -> Result<u32, String> {
        match &self.plan.layouts[ty.index()].layout {
            State::Known { shape } => {
                u32::try_from(shape.value_bytes).map_err(|_| "Wasm: value width overflow".into())
            }
            State::Uninhabited { .. } => Ok(0),
            _ => Err("Wasm: type has no closed value layout".into()),
        }
    }
    pub fn table_push(&mut self, table: u32, payload: u32, bytes: u32) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(table) as i32),
            I::LocalGet(payload),
            I::I32Const(bytes as i32),
            I::Call(TABLE_PUSH),
            I::LocalSet(result),
        ]);
        result
    }
    pub fn table_data(&mut self, table: u32, value: u32, field: u64) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(table) as i32),
            I::LocalGet(value),
            I::I32Load(memory(field, 2)),
            I::Call(TABLE_GET),
            I::I32Load(memory(0, 2)),
            I::LocalSet(result),
        ]);
        result
    }
    pub fn copy(&mut self, destination: u32, offset: u32, source: u32, bytes: u32) {
        self.extend([
            I::LocalGet(destination),
            I::I32Const(offset as i32),
            I::I32Add,
            I::LocalGet(source),
            I::I32Const(bytes as i32),
            I::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
        ]);
    }
    pub fn projection(&mut self, node: HirId, index: usize) -> Result<u32, String> {
        let receiver_node = child(self.mir, node, Role::Receiver)?;
        let ty = self.effective_ty(receiver_node)?;
        let field = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .and_then(|o| o.members.get(index))
            .ok_or("Wasm: missing sealed projection")?;
        let offset = field.offset.ok_or("Wasm: projection has no offset")?;
        let actual =
            self.plan.layouts[field.type_id.ok_or("Wasm: projection has no field type")?].id();
        let receiver = self.expression(receiver_node)?;
        let data = self.table_data(RECORDS, receiver, DATA);
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(data),
            I::I32Const(offset as i32),
            I::I32Add,
            I::LocalSet(result),
        ]);
        self.adapt(node, actual, self.ty(node)?, result)
    }
    pub fn field(&mut self, node: HirId) -> Result<u32, String> {
        if let Some(telora_core::mir::MemberSelection::TraitMember { implementation, .. }) =
            self.mir.member_selections[node.index()]
        {
            let instance = self
                .key
                .instance
                .and_then(|id| self.mir.generic_instances[id.index()].implementation(node))
                .or(self.mir.implementation_instances[node.index()]);
            let key = if let Some(instance) = instance {
                self.plan.instances.get(&instance)
            } else if let Some(symbol) = implementation {
                self.plan.globals.get(&symbol)
            } else {
                None
            }
            .copied()
            .ok_or("Wasm: trait member has no sealed implementation")?;
            let owner = key.ty(self.mir, key.node)?;
            let name = child(self.mir, node, Role::Name)?;
            let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                return Err("Wasm: trait member name missing".into());
            };
            let field = self.plan.layouts[owner.index()]
                .object
                .as_ref()
                .and_then(|object| object.members.iter().find(|field| &field.name == name))
                .ok_or("Wasm: sealed implementation member missing")?;
            let offset = field
                .offset
                .ok_or("Wasm: implementation member offset missing")?;
            if field.type_id != Some(self.ty(node)?.index()) {
                return Err(
                    "Wasm: implementation member type differs from sealed selection".into(),
                );
            }
            let record = self.call_key(key)?;
            let data = self.table_data(RECORDS, record, DATA);
            let result = self.local(ValType::I32);
            self.extend([
                I::LocalGet(data),
                I::I32Const(offset as i32),
                I::I32Add,
                I::LocalSet(result),
            ]);
            return Ok(result);
        }
        if let Some(instance) = self.key.reference(self.mir, node) {
            return self.call_key(
                *self
                    .plan
                    .instances
                    .get(&instance)
                    .ok_or("Wasm: missing sealed member instance")?,
            );
        }
        if let Some(slot) = self.mir.hir[node.index()].resolution
            && let telora_core::mir::ResolveState::Bound(symbol) =
                self.mir.resolve_slots[slot.index()]
        {
            if let Some(&key) = self.plan.globals.get(&symbol) {
                return self.call_key(key);
            }
        }
        let receiver = child(self.mir, node, Role::Receiver)?;
        let ty = self.effective_ty(receiver)?;
        let name = child(self.mir, node, Role::Name)?;
        let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
            return Err("Wasm: missing field name".into());
        };
        if self.mir.types[ty.index()].constructor == T::Dict {
            return self.dictionary_field(node, name);
        }
        let index = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .and_then(|o| o.members.iter().position(|m| &m.name == name))
            .ok_or("Wasm: field is not in sealed layout")?;
        self.projection(node, index)
    }
    pub fn index(&mut self, node: HirId) -> Result<u32, String> {
        let receiver_node = child(self.mir, node, Role::Receiver)?;
        let ty = self.effective_ty(receiver_node)?;
        if self.mir.types[ty.index()].constructor == T::Dict {
            let width = self.width(self.mir.types[ty.index()].arguments[0])?;
            let receiver = self.expression(receiver_node)?;
            let key = self.expression(child(self.mir, node, Role::Index)?)?;
            let result = self.dictionary_lookup(receiver, key, width);
            self.extend([I::LocalGet(result), I::I32Eqz]);
            self.fail_if(node, ERROR_KEY);
            return Ok(result);
        }
        if self.mir.types[ty.index()].constructor != T::Array {
            return Err("Wasm: index receiver is not Array".into());
        }
        let width = self.width(self.mir.types[ty.index()].arguments[0])?;
        let receiver = self.expression(receiver_node)?;
        let index = self.expression(child(self.mir, node, Role::Index)?)?;
        let bits = self.local(ValType::I64);
        self.bits(index);
        self.emit(I::LocalSet(bits));
        self.extend([
            I::LocalGet(bits),
            I::LocalGet(receiver),
            I::I32Load(memory(24, 2)),
            I::LocalGet(receiver),
            I::I32Load(memory(20, 2)),
            I::I32Sub,
            I::I64ExtendI32U,
            I::I64GeU,
        ]);
        self.fail_if(node, ERROR_INDEX);
        let data = self.table_data(ARRAYS, receiver, DATA);
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(data),
            I::LocalGet(bits),
            I::I32WrapI64,
            I::LocalGet(receiver),
            I::I32Load(memory(20, 2)),
            I::I32Add,
            I::I32Const(width as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(result),
        ]);
        Ok(result)
    }
    pub fn text(&mut self, node: HirId, bytes: &[u8]) -> Result<u32, String> {
        self.text_as(node, self.effective_ty(node)?, bytes)
    }
    pub fn bytes_literal(&mut self, node: HirId, bytes: &[u8]) -> Result<u32, String> {
        let length = u32::try_from(bytes.len()).map_err(|_| "Wasm: bytes literal size overflow")?;
        let data = self.alloc(length);
        for (offset, byte) in bytes.iter().enumerate() {
            self.extend([
                I::LocalGet(data),
                I::I32Const(*byte as i32),
                I::I32Store8(memory(offset as u64, 0)),
            ]);
        }
        let id = self.table_push(BYTES, data, length);
        let result = self.value(node, 32)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I32Store(memory(DATA, 2)),
        ]);
        self.store32(result, 20, 0);
        self.store32(result, 24, length);
        self.store32(result, 28, 0);
        Ok(result)
    }
    pub fn text_as(&mut self, node: HirId, ty: TypeId, bytes: &[u8]) -> Result<u32, String> {
        let result = self.value_as(node, ty, 32)?;
        if bytes.len() <= 14 {
            let mut inline = [0u8; 16];
            inline[1] = bytes.len() as u8;
            inline[2..2 + bytes.len()].copy_from_slice(bytes);
            for (index, word) in inline.chunks_exact(8).enumerate() {
                self.extend([
                    I::LocalGet(result),
                    I::I64Const(i64::from_le_bytes(word.try_into().unwrap())),
                    I::I64Store(memory(DATA + index as u64 * 8, 3)),
                ]);
            }
        } else {
            let data = self.alloc(bytes.len() as u32);
            for (index, &byte) in bytes.iter().enumerate() {
                self.extend([
                    I::LocalGet(data),
                    I::I32Const(byte as i32),
                    I::I32Store8(memory(index as u64, 0)),
                ]);
            }
            let id = self.table_push(STRINGS, data, bytes.len() as u32);
            self.store32(result, 16, 1);
            self.extend([
                I::LocalGet(result),
                I::LocalGet(id),
                I::I32Store(memory(20, 2)),
            ]);
            self.store32(result, 24, 0);
            self.store32(result, 28, bytes.len() as u32);
        }
        Ok(result)
    }
}
