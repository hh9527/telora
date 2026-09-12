use super::*;

impl Lower<'_, '_> {
    pub(super) fn sequence_spread(&mut self, node: HirId, depth: usize) -> EmitResult<Vec<ir::Value>> {
        let owner = TypeKey::try_from(self.ty(node)?)?;
        let array = matches!(self.mir.hir[node.index()].kind, HirKind::Array);
        let targets = self.mir.types[owner.index()].arguments.clone();
        let items = self.mir.hir[node.index()].children.iter().filter(|edge| edge.role == Role::Item).map(|edge| edge.node).collect::<Vec<_>>();
        let mut values = vec![];
        let mut parts = vec![];
        let mut count = 0;
        for item in items {
            if matches!(self.mir.hir[item.index()].kind, HirKind::Spread) {
                if array && count != 0 {
                    let data = self.stack_words(&values)?;
                    let length = self.builder.ins().iconst(types::I64, count as i64);
                    parts.extend(self.object(node, helpers::ARRAY, owner, data, length)?);
                    values.clear();
                    count = 0;
                }
                let operand = child(self.mir, item, Role::Operand)?;
                let source_ty = TypeKey::try_from(self.ty(operand)?)?;
                let value = self.expression(operand, depth + 1)?;
                if array {
                    parts.extend(self.fit_metadata(operand, owner, value)?);
                } else {
                    let fields = self.mir.types[source_ty.index()].arguments.clone();
                    let data = self.stack_words(&value)?;
                    for (index, actual) in fields.into_iter().enumerate() {
                        let expected = *targets.get(count).ok_or("tuple spread exceeds its sealed shape")?;
                        let actual = TypeKey::try_from(actual)?;
                        let index = self.builder.ins().iconst(types::I64, index as i64);
                        let value = self.object(operand, helpers::FIELD, actual, data, index)?;
                        values.extend(self.adapt_metadata(actual, TypeKey::try_from(expected)?, value)?);
                        count += 1;
                    }
                }
            } else {
                let expected = *targets.get(if array { 0 } else { count }).ok_or("sequence item has no sealed target type")?;
                let value = self.expression(item, depth + 1)?;
                values.extend(self.fit_metadata(item, TypeKey::try_from(expected)?, value)?);
                count += 1;
            }
        }
        if array {
            if count != 0 || parts.is_empty() {
                let data = self.stack_words(&values)?;
                let length = self.builder.ins().iconst(types::I64, count as i64);
                parts.extend(self.object(node, helpers::ARRAY, owner, data, length)?);
            }
            let count = self.builder.ins().iconst(types::I64, (parts.len() / 4) as i64);
            let data = self.stack_words(&parts)?;
            self.object(node, helpers::ARRAY_CONCAT, owner, data, count)
        } else {
            if count != targets.len() { return Err("tuple spread does not fill its sealed shape".into()); }
            let data = self.stack_words(&values)?;
            let count = self.builder.ins().iconst(types::I64, count as i64);
            self.object(node, helpers::AGGREGATE, owner, data, count)
        }
    }
}
