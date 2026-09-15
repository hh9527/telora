//! Field contributions are compile-time evidence, never anonymous runtime values.
use crate::{abi::*, emit::Emitter, plan::child};
use std::collections::BTreeMap;
use telora_core::mir::{HirId, HirKind, PropertySite, Role, TypeConstructor as T, TypeId};
use wasm_encoder::{Instruction as I, ValType};

// Source node, sealed type and local holding a full-value pointer.
type Fields = BTreeMap<String, (HirId, TypeId, u32)>;

impl Emitter<'_> {
    fn record_inputs(&mut self, node: HirId, depth: usize) -> Result<Fields, String> {
        if depth > 512 {
            return Err("Wasm: record contribution nesting limit".into());
        }
        if matches!(
            self.mir.types[self.effective_ty(node)?.index()].constructor,
            T::Record(_)
        ) {
            return match self.mir.hir[node.index()].kind {
                HirKind::Dict => self.literal_fields(node, depth + 1),
                HirKind::FieldProjection => self.projected_fields(node, depth + 1),
                _ => Err("Wasm: record evidence is not a runtime value".into()),
            };
        }
        let value = self.expression(node)?;
        self.record_value(node, value)
    }

    fn record_value(&mut self, node: HirId, value: u32) -> Result<Fields, String> {
        let ty = self.effective_ty(node)?;
        let members = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("Wasm: record source has no sealed object layout")?
            .members
            .iter()
            .map(|member| (member.name.clone(), member.type_id, member.offset))
            .collect::<Vec<_>>();
        let data = self.table_data(RECORDS, value, DATA);
        let mut fields = Fields::new();
        for (name, ty, offset) in members {
            let ty = self.plan.layouts[ty.ok_or("Wasm: source field type missing")?].id();
            let pointer = self.local(ValType::I32);
            self.extend([
                I::LocalGet(data),
                I::I32Const(offset.ok_or("Wasm: source field offset missing")? as i32),
                I::I32Add,
                I::LocalSet(pointer),
            ]);
            fields.insert(name, (node, ty, pointer));
        }
        Ok(fields)
    }

    fn literal_fields(&mut self, node: HirId, depth: usize) -> Result<Fields, String> {
        let contributions = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|edge| edge.role == Role::Field)
            .map(|edge| edge.node)
            .collect::<Vec<_>>();
        let mut fields = Fields::new();
        for field in contributions {
            let item = child(self.mir, field, Role::Value)?;
            if let Ok(name) = child(self.mir, field, Role::Name) {
                let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                    return Err("Wasm: record field name missing".into());
                };
                let name = name.clone();
                let value = self.expression(item)?;
                fields.insert(name, (item, self.effective_ty(item)?, value));
            } else {
                fields
                    .extend(self.record_inputs(child(self.mir, item, Role::Operand)?, depth + 1)?);
            }
        }
        Ok(fields)
    }

    fn projected_fields(&mut self, node: HirId, depth: usize) -> Result<Fields, String> {
        let receiver = child(self.mir, node, Role::Receiver)?;
        let source = self.record_inputs(receiver, depth + 1)?;
        self.select_fields(node, source)
    }

    fn select_fields(&self, node: HirId, source: Fields) -> Result<Fields, String> {
        let names = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|edge| edge.role == Role::Name)
            .map(|edge| edge.node)
            .collect::<Vec<_>>();
        let targets = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|edge| edge.role == Role::Target)
            .map(|edge| edge.node)
            .collect::<Vec<_>>();
        if names.len() != targets.len() {
            return Err("Wasm: projection source/target arity mismatch".into());
        }
        let mut fields = Fields::new();
        for (name, target) in names.into_iter().zip(targets) {
            let (HirKind::Name(name), HirKind::Name(target)) = (
                &self.mir.hir[name.index()].kind,
                &self.mir.hir[target.index()].kind,
            ) else {
                return Err("Wasm: projection names missing".into());
            };
            fields.insert(
                target.clone(),
                *source
                    .get(name)
                    .ok_or("Wasm: projection field not sealed")?,
            );
        }
        Ok(fields)
    }

    fn finish_record(&mut self, node: HirId, mut fields: Fields) -> Result<u32, String> {
        let ty = self.effective_ty(node)?;
        let members = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("Wasm: record target has no sealed layout")?
            .members
            .iter()
            .map(|member| (member.name.clone(), member.type_id))
            .collect::<Vec<_>>();
        if members.len() != fields.len() {
            return Err("Wasm: contributions differ from the sealed record skeleton".into());
        }
        let mut values = Vec::new();
        for (name, target) in members {
            let (source, actual, value) = fields
                .remove(&name)
                .ok_or("Wasm: sealed record field missing")?;
            let target = self.plan.layouts[target.ok_or("Wasm: target field type missing")?].id();
            values.push(self.adapt(source, actual, target, value)?);
        }
        let result = self.packed_tuple_at(node, ty, &values)?;
        if self.mir.value_adjustments[node.index()].is_none() {
            self.construction_check(node, ty, PropertySite::Type, result)?;
        }
        Ok(result)
    }

    pub fn record(&mut self, node: HirId) -> Result<u32, String> {
        let fields = self.literal_fields(node, 0)?;
        self.finish_record(node, fields)
    }

    pub fn field_projection(&mut self, node: HirId) -> Result<u32, String> {
        let fields = self.projected_fields(node, 0)?;
        self.finish_record(node, fields)
    }

    pub fn field_projection_value(&mut self, node: HirId, value: u32) -> Result<u32, String> {
        let receiver = child(self.mir, node, Role::Receiver)?;
        let source = self.record_value(receiver, value)?;
        let fields = self.select_fields(node, source)?;
        self.finish_record(node, fields)
    }

    pub fn struct_update_left(&mut self, node: HirId, value: u32) -> Result<u32, String> {
        let left = child(self.mir, node, Role::Left)?;
        let right = child(self.mir, node, Role::Right)?;
        if self.effective_ty(left)? != self.effective_ty(node)? {
            return Err("Wasm: struct update changed its sealed owner identity".into());
        }
        let mut fields = self.record_value(left, value)?;
        fields.extend(self.record_inputs(right, 0)?);
        self.finish_record(node, fields)
    }
}
