use super::*;
use std::collections::BTreeSet;

impl Solver<'_> {
    // Constructor aliases retain resolved declarations/member selections;
    // ordinary functions returning the same type are not constructors.
    fn constructor_value(&self, mut node: HirId) -> bool {
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(node) { return false; }
            if matches!(self.mir.member_selections[node.index()], Some(MemberSelection::EnumVariant { .. })) { return true; }
            if matches!(self.mir.hir[node.index()].kind, HirKind::TypeApply) {
                node = self.child(node, Role::Callee).unwrap();
                continue;
            }
            let Some(slot) = self.mir.hir[node.index()].resolution else { return false; };
            let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] else { return false; };
            if self.nominal_index[symbol.index()].is_some_and(|index| self.mir.type_definitions[index].operation == TypeOperation::Newtype) { return true; }
            let Some(&declaration) = self.mir.symbols[symbol.index()].declarations.first() else { return false; };
            let Some(value) = self.child(declaration, Role::Value) else { return false; };
            node = value;
        }
    }

    /// A binding without a construction contract publishes the record it
    /// creates. Later uses cannot choose a different nominal owner for it.
    pub(super) fn prepare_construction_origins(&mut self) {
        for index in 0..self.mir.hir.len() {
            let node = HirId(index as u32);
            if !matches!(self.mir.hir[index].kind,
                HirKind::Binding { kind: BindingKind::Let | BindingKind::Def, .. })
                || self.child(node, Role::Annotation).is_some() { continue; }
            if let Some(value) = self.child(node, Role::Value) {
                let mut pending = vec![value];
                while let Some(node) = pending.pop() {
                    // An update inherits its source's nominal owner. It does
                    // not publish a fresh anonymous record; marking it here
                    // would freeze the source through their shared type slot.
                    if matches!(self.mir.hir[node.index()].kind, HirKind::Dict | HirKind::FieldProjection) {
                        self.materialized_records[node.index()] = true;
                    }
                    pending.extend(self.construction_children(node));
                }
            }
        }
    }

    /// Only result-producing construction syntax propagates an expected owner.
    /// In particular a call's arguments are not its result constructors.
    fn construction_children(&self, node: HirId) -> Vec<HirId> {
        match self.mir.hir[node.index()].kind {
            HirKind::Dict => self.children(node, Role::Field).into_iter()
                .filter_map(|field| self.child(field, Role::Value)).collect(),
            HirKind::Array | HirKind::Tuple => self.children(node, Role::Item),
            HirKind::Spread => self.child(node, Role::Operand).into_iter().collect(),
            HirKind::Block => self.child(node, Role::Result).into_iter().collect(),
            HirKind::If | HirKind::IfLet => [Role::Then, Role::Else].into_iter()
                .filter_map(|role| self.child(node, role)).collect(),
            HirKind::Match => self.children(node, Role::Arm).into_iter()
                .filter_map(|arm| self.child(arm, Role::Value)).collect(),
            HirKind::Call if self.constructor_value(self.child(node, Role::Callee).unwrap()) => self.children(node, Role::Argument),
            _ => vec![],
        }
    }

    pub(super) fn retain_comparison_origins(&mut self, expression: HirId) {
        let mut pending = vec![expression];
        while let Some(node) = pending.pop() {
            if matches!(self.mir.hir[node.index()].kind,
                HirKind::Dict | HirKind::Array | HirKind::Tuple | HirKind::Spread
                | HirKind::Block | HirKind::If | HirKind::IfLet | HirKind::Match
                | HirKind::FieldProjection | HirKind::Binary(BinaryOperator::StructUpdate))
                || (matches!(self.mir.hir[node.index()].kind, HirKind::Call)
                    && self.constructor_value(self.child(node, Role::Callee).unwrap())) {
                pending.extend(self.construction_children(node));
                continue;
            }
            let mut slots = vec![node.ty()];
            let mut seen = BTreeSet::new();
            while let Some(slot) = slots.pop() {
                let root = self.root(slot);
                if !seen.insert(root) { continue; }
                if let Some(term) = self.term(root).cloned() {
                    if matches!(term.constructor, TypeConstructor::Record(_)) {
                        self.materialized_records[root.index()] = true;
                    }
                    slots.extend(term.arguments);
                }
            }
        }
    }
}
