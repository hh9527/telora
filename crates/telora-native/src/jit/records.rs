use super::*;

impl Lower<'_, '_> {
    fn record_field_type(&self, owner: TypeKey, index: usize) -> Result<TypeKey> {
        let ty = self.mir.type_layouts[owner.index()].as_ref()
            .and_then(|layout| layout.members.get(index)).copied().flatten()
            .or_else(|| match self.mir.types[owner.index()].constructor {
                TypeConstructor::Record(_) => self.mir.types[owner.index()].arguments.get(index).copied(),
                _ => None,
            }).ok_or("record field has no sealed type")?;
        TypeKey::try_from(ty)
    }

    pub(super) fn struct_update(&mut self, node: HirId, depth: usize) -> EmitResult<Vec<ir::Value>> {
        let left_node = child(self.mir, node, Role::Left)?;
        let right_node = child(self.mir, node, Role::Right)?;
        let owner = TypeKey::try_from(self.ty(node)?)?;
        let left_ty = TypeKey::try_from(self.ty(left_node)?)?;
        let right_ty = TypeKey::try_from(self.ty(right_node)?)?;
        if owner != left_ty { return Err("struct update lost its sealed owner identity".into()); }
        // Evaluate both authored operands once and in source order. Field
        // selection below is a compile-time layout operation, not a VM lookup.
        let left = self.expression(left_node, depth + 1)?;
        let right = self.expression(right_node, depth + 1)?;
        let left = self.stack_words(&left)?;
        let right = self.stack_words(&right)?;
        let names = self.layouts.field_names[owner.index()].clone();
        let patch_names = self.layouts.field_names[right_ty.index()].clone();
        if patch_names.iter().any(|name| !names.contains(name)) {
            return Err("sealed struct update contains an unknown patch field".into());
        }
        let mut words = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let expected = self.record_field_type(owner, index)?;
            let (source, data, actual, field) = match patch_names.iter().position(|field| field == name) {
                Some(field) => (right_node, right, self.record_field_type(right_ty, field)?, field),
                None => (left_node, left, expected, index),
            };
            let field = self.builder.ins().iconst(types::I64, field as i64);
            let value = self.object(source, helpers::FIELD, actual, data, field)?;
            words.extend(self.adapt_metadata(actual, expected, value)?);
        }
        let data = self.stack_words(&words)?;
        let count = self.builder.ins().iconst(types::I64, names.len() as i64);
        let result = self.object(node, helpers::AGGREGATE, owner, data, count)?;
        self.construction_check(node, owner, telora_core::mir::PropertySite::Type, &result)?;
        Ok(result)
    }
}
