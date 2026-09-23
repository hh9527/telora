//! Checked construction from existential fields into a sealed nominal layout.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{PropertySite, TypeConstructor as T, TypeId, TypeOperation};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    fn dyn_field_error(
        &mut self,
        result_ty: TypeId,
        variant: &str,
        fields: &[u32],
    ) -> Result<(), String> {
        let node = self.key.node;
        let error_ty = self.mir.types[result_ty.index()].arguments[1];
        let index = self.plan.layouts[error_ty.index()]
            .variants
            .iter()
            .position(|item| item.name == variant)
            .ok_or("Wasm: missing Dyn field error variant")? as u32;
        let payload = self.plan.layouts[error_ty.index()].variants[index as usize]
            .type_id
            .ok_or("Wasm: missing Dyn field error payload")?;
        let tuple = self.packed_tuple(self.plan.layouts[payload].id(), fields)?;
        let error = self.enum_value(node, error_ty, index, Some(tuple))?;
        let failed = self.plan.layouts[result_ty.index()]
            .variants
            .iter()
            .position(|item| item.name == "Err")
            .ok_or("Wasm: Result.Err missing")? as u32;
        let value = self.enum_value(node, result_ty, failed, Some(error))?;
        self.extend([I::LocalGet(value), I::Return]);
        Ok(())
    }

    pub(crate) fn dynamic_construct(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let signature = &self.mir.types[self.ty(node)?.index()].arguments;
        if signature.len() != 3 || self.mir.types[signature[0].index()].constructor != T::TypeOf {
            return Err("Wasm: from_dyn_fields signature mismatch".into());
        }
        let target = self.mir.types[signature[0].index()].arguments[0];
        let result_ty = signature[2];
        if self.mir.types[result_ty.index()].constructor != T::Result
            || self.mir.types[result_ty.index()].arguments[0] != target
            || self.mir.types[signature[1].index()].constructor != T::Array
            || self.mir.types[self.mir.types[signature[1].index()].arguments[0].index()].constructor
                != T::Dyn
        {
            return Err("Wasm: from_dyn_fields types are not closed".into());
        }
        let T::Nominal(symbol) = self.mir.types[target.index()].constructor else {
            return Err("from_dyn_fields requires a concrete struct or newtype".into());
        };
        let operation = self
            .mir
            .type_definitions
            .iter()
            .find(|definition| definition.symbol == symbol)
            .map(|definition| definition.operation)
            .ok_or("from_dyn_fields target has no nominal definition")?;
        if !matches!(operation, TypeOperation::Struct | TypeOperation::Newtype) {
            return Err("from_dyn_fields requires a concrete struct or newtype".into());
        }
        let members = self.plan.layouts[target.index()]
            .object
            .as_ref()
            .ok_or("from_dyn_fields target has no sealed layout")?
            .members
            .iter()
            .map(|member| member.type_id.map(|id| self.plan.layouts[id].id()))
            .collect::<Option<Vec<_>>>()
            .ok_or("from_dyn_fields member type is not sealed")?;
        if operation == TypeOperation::Newtype && members.len() != 1 {
            return Err("from_dyn_fields newtype must have one payload".into());
        }
        let input = self.parameter(1);
        let (base, count) = self.array_parts(input, DYN_BYTES);
        self.extend([
            I::LocalGet(count),
            I::I32Const(members.len() as i32),
            I::I32Ne,
            I::If(wasm_encoder::BlockType::Empty),
        ]);
        let count_ty = self.dyn_field_error_types(result_ty, "Count")?[0];
        let expected = self.scalar_as(node, count_ty, members.len() as i64)?;
        let actual = self.scalar_as(node, count_ty, 0)?;
        self.extend([
            I::LocalGet(actual),
            I::LocalGet(count),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        self.dyn_field_error(result_ty, "Count", &[expected, actual])?;
        self.emit(I::End);
        let mut fields = Vec::with_capacity(members.len());
        for (index, &field_ty) in members.iter().enumerate() {
            let position = self.constant_i32(index as i32);
            let item = self.array_item(base, position, DYN_BYTES);
            self.extend([
                I::LocalGet(item),
                I::I32Load(memory(DATA, 2)),
                I::I32Const(field_ty.index() as i32),
                I::I32Ne,
                I::If(wasm_encoder::BlockType::Empty),
            ]);
            let field_types = self.dyn_field_error_types(result_ty, "Field")?;
            let (index_ty, type_ty) = (field_types[0], field_types[1]);
            let at = self.scalar_as(node, index_ty, index as i64)?;
            let expected = self.scalar_as(node, type_ty, field_ty.index() as i64)?;
            let actual = self.scalar_as(node, type_ty, 0)?;
            self.extend([
                I::LocalGet(actual),
                I::LocalGet(item),
                I::I32Load(memory(DATA, 2)),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA, 3)),
            ]);
            self.dyn_field_error(result_ty, "Field", &[at, expected, actual])?;
            self.emit(I::End);
            fields.push(self.table_data(VALUES, item, DATA + 8));
        }
        let value = if operation == TypeOperation::Struct {
            let value = self.packed_tuple(target, &fields)?;
            self.construction_check(node, target, PropertySite::Type, value)?;
            value
        } else {
            self.construction_check(node, target, PropertySite::Type, fields[0])?;
            let id = self.table_push(NEWTYPES, fields[0], self.width(members[0])?, Some(target))?;
            let value = self.value_as(node, target, self.width(target)?)?;
            self.extend([
                I::LocalGet(value),
                I::LocalGet(id),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA, 3)),
            ]);
            value
        };
        let ok = self.plan.layouts[result_ty.index()]
            .variants
            .iter()
            .position(|v| v.name == "Ok")
            .ok_or("Wasm: Result.Ok missing")? as u32;
        self.enum_value(node, result_ty, ok, Some(value))
    }

    fn dyn_field_error_types(
        &self,
        result_ty: TypeId,
        variant: &str,
    ) -> Result<Vec<TypeId>, String> {
        let error_ty = self.mir.types[result_ty.index()].arguments[1];
        let payload = self.plan.layouts[error_ty.index()]
            .variants
            .iter()
            .find(|v| v.name == variant)
            .and_then(|v| v.type_id)
            .ok_or("missing Dyn field error payload")?;
        let members = &self.mir.types[self.plan.layouts[payload].id().index()].arguments;
        Ok(members.clone())
    }

    fn constant_i32(&mut self, value: i32) -> u32 {
        let local = self.local(ValType::I32);
        self.extend([I::I32Const(value), I::LocalSet(local)]);
        local
    }
}
