use super::*;

impl Solver<'_> {
    fn interpreter_error(&mut self, node: HirId, message: &str) {
        self.conflict(node.ty(), node.ty(), Some(self.mir.hir[node.index()].location), message.into());
    }

    pub(super) fn prepare_interpreter(&mut self, node: HirId) {
        let binding = self.mir.hir.iter().enumerate().find(|(_, hir)| {
            matches!(hir.kind, HirKind::Binding { kind: BindingKind::Def, .. })
                && hir.children.iter().any(|edge| edge.role == Role::Value && edge.node == node)
        }).map(|(index, _)| HirId(index as u32));
        if let Some(binding) = binding
            && self.child(binding, Role::Annotation).is_some()
            && let Some(symbol) = self.mir.hir_symbols[binding.index()]
            && !self.mir.symbol_generics[symbol.index()].is_empty()
        {
            self.tasks.push(Task::Interpreter { node, parameters: self.mir.symbol_generics[symbol.index()].clone() });
        } else {
            self.interpreter_error(node, "interpreter requires a directly annotated generic def");
        }
    }

    // Scan provisional type slots, including nominal arguments. Unknown
    // evidence keeps this constraint pending; it does not select an ABI.
    fn interpreter_contains_parameter(&self, slot: TypeSlotId, parameters: &[SymbolId]) -> Option<bool> {
        let mut pending = vec![slot];
        let mut seen = BTreeSet::new();
        while let Some(slot) = pending.pop() {
            let slot = self.root(slot);
            if !seen.insert(slot) { continue; }
            let term = self.term(slot)?;
            if let TypeConstructor::Parameter(parameter) = term.constructor
                && parameters.contains(&parameter) { return Some(true); }
            pending.extend(term.arguments.iter().copied());
        }
        Some(false)
    }

    pub(super) fn interpreter(&mut self, node: HirId, parameters: Vec<SymbolId>) -> Option<Task> {
        let pending = || Some(Task::Interpreter { node, parameters: parameters.clone() });
        if matches!(self.mir.ty_slots[self.root(node.ty()).index()], TypeState::Conflicted(_)) { return None; }
        let Some(outer) = self.term(node.ty()).cloned() else { return pending(); };
        if outer.constructor != TypeConstructor::Function || outer.arguments.is_empty() {
            self.interpreter_error(node, "interpreter requires a witness function returning a function");
            return None;
        }
        let mut witnesses = Vec::new();
        for (index, &slot) in outer.arguments[..outer.arguments.len() - 1].iter().enumerate() {
            let Some(witness) = self.term(slot) else { return pending(); };
            if witness.constructor != TypeConstructor::TypeOf {
                self.interpreter_error(node, "interpreter outer parameters must be TypeOf witnesses");
                return None;
            }
            let Some(subject) = self.term(witness.arguments[0]) else { return pending(); };
            let TypeConstructor::Parameter(parameter) = subject.constructor else {
                self.interpreter_error(node, "interpreter witness must name a quantified type parameter");
                return None;
            };
            if !parameters.contains(&parameter) || witnesses.iter().any(|&(p, _)| p == parameter) {
                self.interpreter_error(node, "interpreter requires a unique witness for each type parameter");
                return None;
            }
            witnesses.push((parameter, index as u32));
        }
        if witnesses.len() != parameters.len() {
            self.interpreter_error(node, "interpreter is missing a type parameter witness");
            return None;
        }
        let Some(inner) = self.term(*outer.arguments.last().unwrap()).cloned() else { return pending(); };
        if inner.constructor != TypeConstructor::Function || inner.arguments.is_empty() {
            self.interpreter_error(node, "interpreter witness function must return a function");
            return None;
        }
        let mut adapters = Vec::new();
        let mut erased = Vec::new();
        for &slot in &inner.arguments[..inner.arguments.len() - 1] {
            let Some(term) = self.term(slot) else { return pending(); };
            if let TypeConstructor::Parameter(parameter) = term.constructor
                && let Some(&(_, index)) = witnesses.iter().find(|&&(p, _)| p == parameter)
            {
                adapters.push(Some(index));
                erased.push(None);
            } else {
                match self.interpreter_contains_parameter(slot, &parameters) {
                    None => return pending(),
                    Some(true) => {
                        self.interpreter_error(node, "interpreter input cannot nest an interpreted type parameter");
                        return None;
                    }
                    Some(false) => { adapters.push(None); erased.push(Some(slot)); }
                }
            }
        }
        let result = *inner.arguments.last().unwrap();
        match self.interpreter_contains_parameter(result, &parameters) {
            None => return pending(),
            Some(true) => {
                self.interpreter_error(node, "interpreter result cannot contain an interpreted type parameter");
                return None;
            }
            Some(false) => {}
        }
        let mut erased = erased.into_iter().map(|slot| slot.unwrap_or_else(|| self.structure(TypeConstructor::Dyn, vec![]))).collect::<Vec<_>>();
        erased.push(result);
        let signature = self.structure(TypeConstructor::Function, erased);
        let operand = self.child(node, Role::Operand).unwrap();
        self.fit(operand, signature, operand.ty());
        self.mir.interpreter_plans[node.index()] = Some(InterpreterPlan {
            witness_count: witnesses.len() as u32, parameters: adapters,
        });
        None
    }
}
