use super::*;

impl Mir {
    /// Follow type-domain aliases only. Def/let values terminate the chain.
    pub(crate) fn materialization_identity(&self, mut node: HirId) -> Option<ValueMaterialization> {
        if matches!(
            self.hir[node.index()].kind,
            HirKind::ConstructorPattern | HirKind::PatternName(_)
        ) {
            return None;
        }
        let mut visited = std::collections::BTreeSet::new();
        while visited.insert(node) {
            match self.member_selections[node.index()] {
                Some(MemberSelection::Boolean(value)) => {
                    return Some(ValueMaterialization::Boolean(value));
                }
                Some(MemberSelection::EnumVariant { index }) => {
                    return Some(ValueMaterialization::EnumVariant { index });
                }
                Some(MemberSelection::NewtypeConstructor) => {
                    return Some(ValueMaterialization::NewtypeConstructor);
                }
                _ => {}
            }
            let role = match self.hir[node.index()].kind {
                HirKind::TypeApply => Some(Role::Callee),
                HirKind::Binding {
                    imported: Some(_), ..
                } => Some(Role::Value),
                HirKind::Binding { .. } => return None,
                _ => None,
            };
            node = if let Some(role) = role {
                self.hir[node.index()]
                    .children
                    .iter()
                    .find(|edge| edge.role == role)?
                    .node
            } else {
                let slot = self.hir[node.index()].resolution?;
                let ResolveState::Bound(symbol) = self.resolve_slots[slot.index()] else {
                    return None;
                };
                *self.symbols[symbol.index()].declarations.last()?
            };
        }
        None
    }

    pub(crate) fn valid_materialization_type(&self, node: HirId, ty: TypeId) -> bool {
        let Some(fact) = self.value_materializations[node.index()] else {
            return true;
        };
        let Some(shape) = self.types.get(ty.index()) else {
            return false;
        };
        let (owner, payload) = if shape.constructor == TypeConstructor::Function {
            if shape.arguments.len() != 2 {
                return false;
            }
            (shape.arguments[1], Some(shape.arguments[0]))
        } else {
            (ty, None)
        };
        let Some(shape) = self.types.get(owner.index()) else {
            return false;
        };
        match fact {
            ValueMaterialization::Boolean(_) => {
                shape.constructor == TypeConstructor::Bool && payload.is_none()
            }
            ValueMaterialization::EnumVariant { index } => {
                let expected = if let Some((_, has_payload)) =
                    crate::type_image::builtin_variant(&shape.constructor, index)
                {
                    if has_payload {
                        crate::type_image::builtin_variant_argument(&shape.constructor, index)
                            .and_then(|argument| shape.arguments.get(argument).copied())
                            .map(Some)
                    } else {
                        Some(None)
                    }
                } else if let TypeConstructor::Nominal(symbol) = shape.constructor {
                    if !self
                        .type_definitions
                        .iter()
                        .any(|d| d.symbol == symbol && d.operation == TypeOperation::Enum)
                    {
                        return false;
                    }
                    self.type_layouts
                        .get(owner.index())
                        .and_then(Option::as_ref)
                        .and_then(|layout| layout.members.get(index as usize))
                        .copied()
                } else {
                    None
                };
                expected == Some(payload)
                    || matches!((payload, expected), (Some(source), Some(Some(target)))
                        if self.types[source.index()].constructor == TypeConstructor::TypeOf
                            && self.types[target.index()].constructor == TypeConstructor::Type)
            }
            ValueMaterialization::NewtypeConstructor => {
                matches!(shape.constructor, TypeConstructor::Nominal(symbol)
                    if self.type_definitions.iter().any(|d| d.symbol == symbol && d.operation == TypeOperation::Newtype))
                    && payload.is_some()
                    && self
                        .type_layouts
                        .get(owner.index())
                        .and_then(Option::as_ref)
                        .is_some_and(|layout| layout.members.as_slice() == [payload])
            }
        }
    }
}
