use crate::{abi::*, emit::Emitter, plan::child};
use std::collections::BTreeMap;
use telora_core::mir::{HirId, HirKind, Role, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    fn literal_dict_part(
        &mut self,
        node: HirId,
        ty: TypeId,
        fields: BTreeMap<String, (HirId, u32)>,
    ) -> Result<u32, String> {
        let width = self.width(self.mir.types[ty.index()].arguments[0])?;
        let count = self.local(ValType::I32);
        self.extend([
            I::I32Const(
                u32::try_from(fields.len()).map_err(|_| "Wasm: dict size overflow")? as i32,
            ),
            I::LocalSet(count),
        ]);
        let keys = self.array_storage(count, STRING_BYTES);
        let values = self.array_storage(count, width);
        for (index, (name, (field, value))) in fields.into_iter().enumerate() {
            let key = self.text_as(field, self.string_type()?, name.as_bytes())?;
            self.copy(keys, index as u32 * STRING_BYTES, key, STRING_BYTES);
            self.copy(values, index as u32 * width, value, width);
        }
        self.dict_result_at(node, ty, keys, values, count, width)
    }

    fn dict_reheader(&mut self, node: HirId, ty: TypeId, value: u32) -> Result<u32, String> {
        let result = self.value_as(node, ty, STRING_BYTES)?;
        self.extend([
            I::LocalGet(result),
            I::I32Const(DATA as i32),
            I::I32Add,
            I::LocalGet(value),
            I::I32Const(DATA as i32),
            I::I32Add,
            I::I32Const(16),
            I::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
        ]);
        Ok(result)
    }

    fn dict_spread_value(&mut self, node: HirId, target: TypeId) -> Result<u32, String> {
        let actual = self.effective_ty(node)?;
        let value = self.expression(node)?;
        if actual == target {
            return Ok(value);
        }
        let source = &self.mir.types[actual.index()];
        if source.constructor != T::Dict || source.arguments.len() != 1 {
            return Err("Wasm: dictionary spread source is not sealed Dict".into());
        }
        let from = source.arguments[0];
        let into = self.mir.types[target.index()].arguments[0];
        let count = self.local(ValType::I32);
        self.extend([
            I::LocalGet(value),
            I::I32Load(memory(DATA + 4, 2)),
            I::LocalSet(count),
        ]);
        if self.width(from)? == 0 {
            self.extend([
                I::LocalGet(count),
                I::If(BlockType::Empty),
                I::Unreachable,
                I::End,
            ]);
            return self.dict_reheader(node, target, value);
        }
        let source = self.table_data(ARRAYS, value, DATA + 8);
        let width = self.width(into)?;
        let values = self.array_storage(count, width);
        let keys = self.table_data(ARRAYS, value, DATA);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let item = self.array_item(source, index, self.width(from)?);
        let item = self.adapt(node, from, into, item)?;
        let destination = self.array_item(values, index, width);
        self.copy(destination, 0, item, width);
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        self.dict_result_at(node, target, keys, values, count, width)
    }

    pub fn dictionary(&mut self, node: HirId) -> Result<u32, String> {
        let ty = self.effective_ty(node)?;
        let element = self.mir.types[ty.index()].arguments[0];
        let contributions = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|edge| edge.role == Role::Field)
            .map(|edge| edge.node)
            .collect::<Vec<_>>();
        let mut fields = BTreeMap::new();
        let mut parts = Vec::new();
        let spread_only = contributions.len() == 1
            && !self.mir.hir[contributions[0].index()]
                .children
                .iter()
                .any(|edge| edge.role == Role::Name);
        for field in contributions {
            let item = child(self.mir, field, Role::Value)?;
            if let Ok(name) = child(self.mir, field, Role::Name) {
                let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                    return Err("Wasm: dictionary key missing".into());
                };
                let name = name.clone();
                let value = self.expression(item)?;
                let value = self.adapt(item, self.effective_ty(item)?, element, value)?;
                fields.insert(name, (field, value));
            } else {
                if !fields.is_empty() {
                    parts.push(self.literal_dict_part(node, ty, std::mem::take(&mut fields))?);
                }
                parts.push(self.dict_spread_value(child(self.mir, item, Role::Operand)?, ty)?);
            }
        }
        if !fields.is_empty() || parts.is_empty() {
            parts.push(self.literal_dict_part(node, ty, fields)?);
        }
        let mut parts = parts.into_iter();
        let mut result = parts.next().ok_or("Wasm: dictionary has no contribution")?;
        for part in parts {
            result = self.dict_merge_values(node, ty, result, part)?;
        }
        // A spread-only literal shares columns but is a new container origin.
        if spread_only {
            self.dict_reheader(node, ty, result)
        } else {
            Ok(result)
        }
    }
}
