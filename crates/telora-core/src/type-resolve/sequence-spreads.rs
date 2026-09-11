use super::*;

impl Solver<'_> {
    pub(super) fn has_sequence_spread(&self, node: HirId) -> bool {
        self.children(node, Role::Item).iter().any(|item| matches!(self.mir.hir[item.index()].kind, HirKind::Spread))
    }

    pub(super) fn array_spread(&mut self, node: HirId) {
        let element = self.fresh();
        let array = self.structure(TypeConstructor::Array, vec![element]);
        self.same(node, array);
        for item in self.children(node, Role::Item) {
            if matches!(self.mir.hir[item.index()].kind, HirKind::Spread) {
                let operand = self.child(item, Role::Operand).unwrap();
                self.fit(operand, array, operand.ty());
            } else {
                self.fit(item, element, item.ty());
            }
        }
    }

    pub(super) fn tuple_spread(&mut self, node: HirId) -> Option<Task> {
        let mut elements = vec![];
        for item in self.children(node, Role::Item) {
            if !matches!(self.mir.hir[item.index()].kind, HirKind::Spread) {
                elements.push(item.ty());
                continue;
            }
            let operand = self.child(item, Role::Operand).unwrap();
            let Some(term) = self.term(operand.ty()).cloned() else {
                if matches!(self.mir.ty_slots[self.root(operand.ty()).index()], TypeState::Conflicted(_)) {
                    self.same(node, operand.ty());
                    return None;
                }
                return Some(Task::TupleSpread { node });
            };
            match term.constructor {
                TypeConstructor::Tuple | TypeConstructor::TupleLiteral => elements.extend(term.arguments),
                TypeConstructor::Never => {
                    self.assign(node, TypeConstructor::Never, vec![]);
                    return None;
                }
                _ => {
                    self.conflict(node.ty(), node.ty(), Some(self.mir.hir[item.index()].location), "tuple spread requires an unnamed tuple operand".into());
                    return None;
                }
            }
        }
        // Flattening preserves the ordinary tuple's type/value-world rules.
        self.solve_task(Task::Tuple { node, items: elements })
    }
}
