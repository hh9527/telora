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
    fn interpreter_contains_parameter(&self, slot: TypeSlotId, parameters: &[SymbolId]) -> Option<Option<SymbolId>> {
        let mut pending = vec![slot];
        let mut seen = BTreeSet::new();
        while let Some(slot) = pending.pop() {
            let slot = self.root(slot);
            if !seen.insert(slot) { continue; }
            let term = self.term(slot)?;
            if let TypeConstructor::Parameter(parameter) = term.constructor
                && parameters.contains(&parameter) { return Some(Some(parameter)); }
            pending.extend(term.arguments.iter().copied());
        }
        Some(None)
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
                self.interpreter_error(node, &format!("interpreter outer parameters must be TypeOf witnesses: witness parameter {} has type {}",
                    index + 1, self.diagnostic_type(slot)));
                return None;
            }
            let Some(subject) = self.term(witness.arguments[0]) else { return pending(); };
            let TypeConstructor::Parameter(parameter) = subject.constructor else {
                self.interpreter_error(node, "interpreter witness must name a quantified type parameter");
                return None;
            };
            if !parameters.contains(&parameter) || witnesses.iter().any(|&(p, _)| p == parameter) {
                self.interpreter_error(node, &format!("interpreter requires a unique witness for each type parameter: type parameter {} {}",
                    self.mir.symbols[parameter.index()].name,
                    if parameters.contains(&parameter) { "has more than one TypeOf witness" } else { "is not quantified by this declaration" }));
                return None;
            }
            witnesses.push((parameter, index as u32));
        }
        if witnesses.len() != parameters.len() {
            let missing = parameters.iter().find(|&&parameter| !witnesses.iter().any(|&(p, _)| p == parameter)).unwrap();
            self.interpreter_error(node, &format!("interpreter is missing a type parameter witness: type parameter {} has no TypeOf witness",
                self.mir.symbols[missing.index()].name));
            return None;
        }
        let Some(inner) = self.term(*outer.arguments.last().unwrap()).cloned() else { return pending(); };
        if inner.constructor != TypeConstructor::Function || inner.arguments.is_empty() {
            self.interpreter_error(node, "interpreter witness function must return a function");
            return None;
        }
        let mut adapters = Vec::new();
        let mut erased = Vec::new();
        for (index, &slot) in inner.arguments[..inner.arguments.len() - 1].iter().enumerate() {
            let Some(term) = self.term(slot) else { return pending(); };
            if let TypeConstructor::Parameter(parameter) = term.constructor
                && let Some(&(_, index)) = witnesses.iter().find(|&&(p, _)| p == parameter)
            {
                adapters.push(Some(index));
                erased.push(None);
            } else {
                match self.interpreter_contains_parameter(slot, &parameters) {
                    None => return pending(),
                    Some(Some(parameter)) => {
                        self.interpreter_error(node, &format!("interpreter input cannot nest an interpreted type parameter: inner parameter {} contains type parameter {} in {}",
                            index + 1, self.mir.symbols[parameter.index()].name, self.diagnostic_type(slot)));
                        return None;
                    }
                    Some(None) => { adapters.push(None); erased.push(Some(slot)); }
                }
            }
        }
        let result = *inner.arguments.last().unwrap();
        match self.interpreter_contains_parameter(result, &parameters) {
            None => return pending(),
            Some(Some(parameter)) => {
                self.interpreter_error(node, &format!("interpreter result cannot contain an interpreted type parameter: result contains type parameter {} in {}",
                    self.mir.symbols[parameter.index()].name, self.diagnostic_type(result)));
                return None;
            }
            Some(None) => {}
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
