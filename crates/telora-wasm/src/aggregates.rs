use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::{
    candidate_layout::State,
    mir::{HirId, HirKind, Role, TypeConstructor as T, TypeId, TypeState},
};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    pub fn effective_ty(&self, node: HirId) -> Result<TypeId, String> {
        if let Some(slot) = self.mir.value_adjustments[node.index()] {
            return match self.key.instance {
                Some(id) => self.mir.generic_instances[id.index()]
                    .adjustment(node)
                    .ok_or_else(|| "Wasm: missing sealed adjustment".into()),
                None => match self.mir.ty_slots[slot.index()] {
                    TypeState::Known(ty) => Ok(ty),
                    _ => Err("Wasm: unsealed adjustment".into()),
                },
            };
        }
        self.ty(node)
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
    pub fn array(&mut self, node: HirId) -> Result<u32, String> {
        let ty = self.effective_ty(node)?;
        if self.mir.types[ty.index()].constructor != T::Array {
            return Err("Wasm: array needs sealed Array type".into());
        }
        let element = self.mir.types[ty.index()].arguments[0];
        let width = self.width(element)?;
        let items = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|e| e.role == Role::Item)
            .map(|e| e.node)
            .collect::<Vec<_>>();
        let bytes = (items.len() as u32)
            .checked_mul(width)
            .ok_or("Wasm: array size overflow")?;
        let data = self.alloc(bytes);
        for (index, &item) in items.iter().enumerate() {
            if self.effective_ty(item)? != element {
                return Err("Wasm: array element requires sealed adaptation".into());
            }
            let value = self.expression(item)?;
            self.copy(data, index as u32 * width, value, width);
        }
        let id = self.table_push(ARRAYS, data, bytes);
        let result = self.value(node, 32)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I32Store(memory(DATA, 2)),
        ]);
        self.store32(result, 20, 0);
        self.store32(result, 24, items.len() as u32);
        self.store32(result, 28, 0);
        Ok(result)
    }
    pub fn record(&mut self, node: HirId) -> Result<u32, String> {
        let ty = self.effective_ty(node)?;
        let object = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("Wasm: aggregate has no sealed fields")?;
        let size = u32::try_from(object.bytes.ok_or("Wasm: aggregate has no fixed layout")?)
            .map_err(|_| "Wasm: record size overflow")?;
        let members = object
            .members
            .iter()
            .map(|m| (m.name.clone(), m.type_id.unwrap(), m.offset.unwrap() as u32))
            .collect::<Vec<_>>();
        let data = self.alloc(size);
        let mut values = std::collections::BTreeMap::new();
        if matches!(self.mir.hir[node.index()].kind, HirKind::Tuple) {
            let items = self.mir.hir[node.index()]
                .children
                .iter()
                .filter(|e| e.role == Role::Item)
                .map(|e| e.node)
                .collect::<Vec<_>>();
            if items.len() != members.len() {
                return Err("Wasm: tuple arity does not match its sealed layout".into());
            }
            for ((name, _, _), item) in members.iter().zip(items) {
                let value = self.expression(item)?;
                values.insert(name.clone(), (value, self.effective_ty(item)?.index()));
            }
        } else {
            for edge in &self.mir.hir[node.index()].children {
                if edge.role != Role::Field {
                    continue;
                }
                let name_node = child(self.mir, edge.node, Role::Name)?;
                let HirKind::Name(name) = &self.mir.hir[name_node.index()].kind else {
                    return Err("Wasm: missing field name".into());
                };
                let item = child(self.mir, edge.node, Role::Value)?;
                let value = self.expression(item)?;
                values.insert(name.clone(), (value, self.effective_ty(item)?.index()));
            }
        }
        for (name, member, offset) in &members {
            let (value, actual) = values
                .remove(name)
                .ok_or("Wasm: missing sealed record field")?;
            if actual != *member {
                return Err("Wasm: record field requires sealed adaptation".into());
            }
            let State::Known { shape } = &self.plan.layouts[*member].layout else {
                return Err("Wasm: field has no value layout".into());
            };
            self.copy(data, *offset, value, shape.value_bytes as u32);
        }
        if !values.is_empty() {
            return Err("Wasm: extra record fields".into());
        }
        let id = self.table_push(RECORDS, data, size);
        let result = self.value(node, 24)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        if self.mir.value_adjustments[node.index()].is_none() {
            self.construction_check(node, ty, telora_core::mir::PropertySite::Type, result)?;
        }
        Ok(result)
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
        let receiver = self.expression(receiver_node)?;
        let data = self.table_data(RECORDS, receiver, DATA);
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(data),
            I::I32Const(offset as i32),
            I::I32Add,
            I::LocalSet(result),
        ]);
        Ok(result)
    }
    pub fn field(&mut self, node: HirId) -> Result<u32, String> {
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
