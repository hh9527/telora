use super::*;
use std::collections::BTreeSet;

impl Solver<'_> {
    /// Only an actual empty constructor supplies bottom evidence. An arbitrary
    /// function returning Option(_) must still solve its own result type.
    fn empty_option_value(&self, mut node: HirId) -> bool {
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(node) { return false; }
            if let Some(MemberSelection::EnumVariant { index: 0 }) = self.mir.member_selections[node.index()] {
                return self.term(node.ty()).is_some_and(|term| term.constructor == TypeConstructor::Option);
            }
            if matches!(self.mir.hir[node.index()].kind, HirKind::TypeApply) {
                node = self.child(node, Role::Callee).unwrap();
                continue;
            }
            let Some(slot) = self.mir.hir[node.index()].resolution else { return false; };
            let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] else { return false; };
            let Some(&declaration) = self.mir.symbols[symbol.index()].declarations.first() else { return false; };
            let Some(value) = self.child(declaration, Role::Value) else { return false; };
            node = value;
        }
    }

    pub(super) fn finish_empty_options(&mut self) -> bool {
        let mut changed = false;
        for index in 0..self.mir.hir.len() {
            let node = HirId(index as u32);
            let Some(term) = self.term(node.ty()) else { continue; };
            if term.constructor != TypeConstructor::Option { continue; }
            let argument = self.root(term.arguments[0]);
            if self.mir.ty_slots[argument.index()] != TypeState::Unknown || !self.empty_option_value(node) { continue; }
            let never = self.structure(TypeConstructor::Never, vec![]);
            self.equal(argument, never, Some(self.mir.hir[index].location));
            changed = true;
        }
        changed
    }

}
