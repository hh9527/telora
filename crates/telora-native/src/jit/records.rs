use super::*;

impl Lower<'_, '_> {
    fn record_field_type(&self, owner: TypeKey, index: usize) -> Result<TypeKey> {
        let owner = if self.mir.types[owner.index()].constructor == TypeConstructor::Unchecked {
            TypeKey::try_from(self.mir.types[owner.index()].arguments[0])?
        } else { owner };
        let ty = self.mir.type_layouts[owner.index()].as_ref()
            .and_then(|layout| layout.members.get(index)).copied().flatten()
            .or_else(|| match self.mir.types[owner.index()].constructor {
                TypeConstructor::Record(_) => self.mir.types[owner.index()].arguments.get(index).copied(),
                _ => None,
            }).ok_or("record field has no sealed type")?;
        TypeKey::try_from(ty)
    }

    fn finish_record(&mut self, node: HirId, owner: TypeKey, mut fields: BTreeMap<String, (TypeKey, Vec<ir::Value>)>) -> EmitResult<Vec<ir::Value>> {
        let names = self.layouts.field_names[owner.index()].clone();
        if names.len() != fields.len() { return Err("record contributions differ from the sealed skeleton".into()); }
        let mut words = vec![];
        for (index, name) in names.iter().enumerate() {
            let (actual, value) = fields.remove(name).ok_or("sealed record contribution missing")?;
            let expected = self.record_field_type(owner, index)?;
            words.extend(self.adapt_metadata(actual, expected, value)?);
        }
        let data = self.stack_words(&words)?;
        let count = self.builder.ins().iconst(types::I64, names.len() as i64);
        let result = self.object(node, helpers::AGGREGATE, owner, data, count)?;
        self.construction_check(node, owner, telora_core::mir::PropertySite::Type, &result)?;
        Ok(result)
    }

    pub(super) fn field_projection(&mut self, node: HirId, depth: usize) -> EmitResult<Vec<ir::Value>> {
        let receiver = child(self.mir, node, Role::Receiver)?;
        let source_ty = TypeKey::try_from(self.ty(receiver)?)?;
        let owner = TypeKey::try_from(self.ty(node)?)?;
        let value = self.expression(receiver, depth + 1)?;
        let data = self.stack_words(&value)?;
        let sources = self.mir.hir[node.index()].children.iter().filter(|edge| edge.role == Role::Name).map(|edge| edge.node).collect::<Vec<_>>();
        let targets = self.mir.hir[node.index()].children.iter().filter(|edge| edge.role == Role::Target).map(|edge| edge.node).collect::<Vec<_>>();
        if sources.len() != targets.len() { return Err("sealed projection has mismatched names".into()); }
        let mut fields = BTreeMap::new();
        for (source, target) in sources.into_iter().zip(targets) {
            let (HirKind::Name(source_name), HirKind::Name(target_name)) = (&self.mir.hir[source.index()].kind, &self.mir.hir[target.index()].kind) else {
                return Err("sealed projection has no field name".into());
            };
            let index = self.layouts.field_names[source_ty.index()].iter().position(|name| name == source_name).ok_or("projection source is outside the sealed skeleton")?;
            let target_name = target_name.clone();
            let actual = self.record_field_type(source_ty, index)?;
            let index = self.builder.ins().iconst(types::I64, index as i64);
            let value = self.object(source, helpers::FIELD, actual, data, index)?;
            fields.insert(target_name, (actual, value));
        }
        self.finish_record(node, owner, fields)
    }

    pub(super) fn record_spread(&mut self, node: HirId, depth: usize) -> EmitResult<Vec<ir::Value>> {
        let owner = TypeKey::try_from(self.ty(node)?)?;
        if self.mir.types[owner.index()].constructor == TypeConstructor::Dict { return self.dict_spread(node, owner, depth); }
        let mut fields = BTreeMap::new();
        let contributions = self.mir.hir[node.index()].children.iter().filter(|edge| edge.role == Role::Field).map(|edge| edge.node).collect::<Vec<_>>();
        for field in contributions {
            if self.mir.hir[field.index()].children.iter().any(|edge| edge.role == Role::Decorator) {
                return Err("native field property construction is not yet linked".into());
            }
            let value_node = child(self.mir, field, Role::Value)?;
            if let Ok(name) = child(self.mir, field, Role::Name) {
                let HirKind::Name(name) = &self.mir.hir[name.index()].kind else { return Err("record field name missing".into()); };
                let name = name.clone();
                let value = self.expression(value_node, depth + 1)?;
                let actual = self.adjustment_type(value_node)?.unwrap_or(TypeKey::try_from(self.ty(value_node)?)?);
                fields.insert(name, (actual, value));
            } else {
                let operand = child(self.mir, value_node, Role::Operand)?;
                let source_ty = TypeKey::try_from(self.ty(operand)?)?;
                let value = self.expression(operand, depth + 1)?;
                let data = self.stack_words(&value)?;
                let names = self.layouts.field_names[source_ty.index()].clone();
                for (index, name) in names.into_iter().enumerate() {
                    let actual = self.record_field_type(source_ty, index)?;
                    let index = self.builder.ins().iconst(types::I64, index as i64);
                    let value = self.object(operand, helpers::FIELD, actual, data, index)?;
                    fields.insert(name, (actual, value));
                }
            }
        }
        // Only winners must fit the final field types. Losers have already
        // executed, including their failures and side effects.
        self.finish_record(node, owner, fields)
    }

    fn dict_spread(&mut self, node: HirId, owner: TypeKey, depth: usize) -> EmitResult<Vec<ir::Value>> {
        let string = self.mir.types.iter().position(|ty| ty.constructor == TypeConstructor::String).ok_or("sealed String type missing")?;
        let string = self.layouts.type_at(string)?;
        let element = TypeKey::try_from(self.mir.types[owner.index()].arguments[0])?;
        let contributions = self.mir.hir[node.index()].children.iter().filter(|edge| edge.role == Role::Field).map(|edge| edge.node).collect::<Vec<_>>();
        let mut parts = vec![];
        let mut words = vec![];
        let mut count = 0;
        for field in contributions {
            if self.mir.hir[field.index()].children.iter().any(|edge| edge.role == Role::Decorator) {
                return Err("native field property construction is not yet linked".into());
            }
            let value_node = child(self.mir, field, Role::Value)?;
            if let Ok(name) = child(self.mir, field, Role::Name) {
                let HirKind::Name(name) = &self.mir.hir[name.index()].kind else { return Err("dictionary field name missing".into()); };
                let name = name.clone();
                let value = self.expression(value_node, depth + 1)?;
                words.extend(self.string(field, string, &name)?);
                words.extend(self.fit_metadata(value_node, element, value)?);
                count += 1;
            } else {
                if count != 0 {
                    let data = self.stack_words(&words)?;
                    let length = self.builder.ins().iconst(types::I64, count);
                    parts.push(self.object(node, helpers::DICT, owner, data, length)?);
                    words.clear();
                    count = 0;
                }
                let operand = child(self.mir, value_node, Role::Operand)?;
                let value = self.expression(operand, depth + 1)?;
                parts.push(self.fit_metadata(operand, owner, value)?);
            }
        }
        if count != 0 || parts.is_empty() {
            let data = self.stack_words(&words)?;
            let length = self.builder.ins().iconst(types::I64, count);
            parts.push(self.object(node, helpers::DICT, owner, data, length)?);
        }
        let mut parts = parts.into_iter();
        let mut result = parts.next().ok_or("dictionary spread has no contributions")?;
        for part in parts {
            result.extend(part);
            let data = self.stack_words(&result)?;
            let merge = self.builder.ins().iconst(types::I64, 5);
            result = self.object(node, helpers::DICT_READ, owner, data, merge)?;
        }
        Ok(result)
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
