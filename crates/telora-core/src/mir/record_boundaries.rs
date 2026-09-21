//! Validate direct record field boundaries before publishing executable types.
use super::*;

impl Mir {
    fn boundary_type(&self, node: HirId, instance: Option<&GenericInstance>) -> Option<TypeId> {
        if let Some(slot) = self.value_adjustments.get(node.index()).copied().flatten() {
            if let Some(instance) = instance {
                return instance.adjustment(node);
            }
            return match self.ty_slots.get(slot.index()) {
                Some(TypeState::Known(ty)) => Some(*ty),
                _ => None,
            };
        }
        if let Some(instance) = instance {
            return instance.ty(node);
        }
        match self.ty_slots.get(node.index()) {
            Some(TypeState::Known(ty)) => Some(*ty),
            _ => None,
        }
    }

    pub(super) fn record_boundary_diagnostics(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut check = |node: HirId, instance: Option<&GenericInstance>| {
            let hir = &self.hir[node.index()];
            if !matches!(hir.kind, HirKind::Dict) {
                return;
            }
            let Some(mut ty) = self.boundary_type(node, instance) else {
                return;
            };
            if self.types[ty.index()].constructor == TypeConstructor::Unchecked {
                ty = self.types[ty.index()].arguments[0];
            }
            let TypeConstructor::Nominal(symbol) = self.types[ty.index()].constructor else {
                return;
            };
            let Some(definition) = self
                .type_definitions
                .iter()
                .find(|d| d.symbol == symbol && d.operation == TypeOperation::Struct)
            else {
                return;
            };
            let Some(layout) = self.type_layouts.get(ty.index()).and_then(Option::as_ref) else {
                return;
            };
            for field in hir.children.iter().filter(|edge| edge.role == Role::Field) {
                let edges = &self.hir[field.node.index()].children;
                let Some(name) = edges.iter().find(|e| e.role == Role::Name) else {
                    continue;
                };
                let HirKind::Name(name) = &self.hir[name.node.index()].kind else {
                    continue;
                };
                let Some(value) = edges.iter().find(|e| e.role == Role::Value).map(|e| e.node)
                else {
                    continue;
                };
                let Some(index) = definition.members.iter().position(|m| &m.name == name) else {
                    continue;
                };
                let Some(expected) = layout.members.get(index).copied().flatten() else {
                    continue;
                };
                let Some(actual) = self.boundary_type(value, instance) else {
                    continue;
                };
                let source = &self.types[actual.index()];
                if actual == expected
                    || source.constructor == TypeConstructor::Never
                    || (source.constructor == TypeConstructor::TypeOf
                        && self.types[expected.index()].constructor == TypeConstructor::Type)
                {
                    continue;
                }
                diagnostics.push(Diagnostic::error(
                    format!("record field {name:?} has no sealed boundary adaptation: {actual:?} -> {expected:?}"),
                    self.hir[value.index()].location,
                ));
            }
        };
        for index in 0..self.hir.len() {
            check(HirId(index as u32), None);
        }
        for instance in &self.generic_instances {
            if instance.concrete {
                for &(node, _) in &instance.types {
                    check(node, Some(instance));
                }
            }
        }
        diagnostics
    }
}
