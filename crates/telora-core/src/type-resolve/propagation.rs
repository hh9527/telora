use super::*;

impl Solver<'_> {
    pub(super) fn propagate(&mut self, node: HirId) -> Option<Task> {
        let operand = self.child(node, Role::Operand).unwrap();
        let boundary = self.mir.propagation_boundaries[node.index()].expect("lexical propagation boundary");
        let (result, body) = if matches!(self.mir.hir[boundary.index()].kind, HirKind::Closure) {
            (self.child(boundary, Role::ReturnType).unwrap(), self.child(boundary, Role::Body).unwrap())
        } else { (boundary, boundary) };
        let operand_term = self.term(operand.ty()).cloned();
        if matches!(self.mir.ty_slots[self.root(operand.ty()).index()], TypeState::Conflicted(_)) {
            self.same(node, operand.ty());
            return None;
        }
        let family = operand_term.as_ref().map(|term| term.constructor.clone())
            .or_else(|| self.term(result.ty()).map(|term| term.constructor.clone()));
        let Some(family) = family else { return Some(Task::Propagate { node }); };
        if !matches!(family, TypeConstructor::Option | TypeConstructor::Result) {
            self.conflict(node.ty(), node.ty(), Some(self.mir.hir[node.index()].location),
                "? requires an Option or Result operand and matching return boundary".into());
            return None;
        }
        let success = self.fresh();
        let mut input = vec![node.ty()];
        let mut output = vec![success];
        if family == TypeConstructor::Result {
            let input_error = self.fresh();
            let output_error = self.fresh();
            input.push(input_error);
            output.push(output_error);
            self.fit(node, output_error, input_error);
        }
        self.assign(operand, family.clone(), input);
        let output = self.structure(family, output);
        self.equal(result.ty(), output, Some(self.mir.hir[node.index()].location));
        Some(Task::PropagationBottom { body, success })
    }
}
