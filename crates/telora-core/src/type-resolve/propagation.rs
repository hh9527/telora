use super::*;

impl Solver<'_> {
    /// A checked return boundary changes the callable's exposed signature,
    /// while the body's intrinsic slots must keep their unchecked provenance.
    /// Record the complete signature before instance materialization so every
    /// backend can consume an existing TypeId, including generic instances.
    pub(super) fn finalize_callable_adjustments(&mut self) {
        let mut canonical = self.mir.types.iter().enumerate().map(|(index, ty)|
            ((ty.constructor.clone(), ty.arguments.clone()), TypeId(index as u32)))
            .collect::<std::collections::BTreeMap<_, _>>();
        for index in 0..self.mir.hir.len() {
            if !matches!(self.mir.hir[index].kind, HirKind::Closure) { continue; }
            let Some(boundary) = self.child(HirId(index as u32), Role::ReturnType) else { continue; };
            let Some(slot) = self.mir.value_adjustments[boundary.index()] else { continue; };
            let (TypeState::Known(target), TypeState::Known(source)) =
                (self.mir.ty_slots[slot.index()], self.mir.ty_slots[index]) else { continue; };
            let signature = &self.mir.types[source.index()];
            if signature.constructor != TypeConstructor::Function { continue; }
            let mut arguments = signature.arguments.clone();
            let Some(result) = arguments.last_mut() else { continue; };
            *result = target;
            let key = (TypeConstructor::Function, arguments);
            let signature = *canonical.entry(key.clone()).or_insert_with(|| {
                let ty = TypeId(self.mir.types.len() as u32);
                self.mir.types.push(ResolvedType { constructor: key.0, arguments: key.1 });
                self.mir.type_layouts.push(None);
                ty
            });
            let slot = self.fresh();
            self.mir.ty_slots[slot.index()] = TypeState::Known(signature);
            self.mir.value_adjustments[index] = Some(slot);
        }
    }

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
