use super::*;

impl Emitter<'_> {
    pub(super) fn interpreter(&mut self, node: HirId) -> Result<R, Diagnostic> {
        let plan = self.mir.interpreter_plans[node.index()].as_ref().expect("sealed interpreter plan");
        let operand = self.child(node, Role::Operand);
        let mut factory = Self::new(self.mir, self.graph, format!("interpreter:{}", node.index()));
        factory.instance = self.instance;
        factory.function.memoized_interpreter = true;
        factory.function.parameter_count = plan.witness_count;
        let witnesses = (0..plan.witness_count).map(|_| factory.register()).collect::<Vec<_>>();
        let mut adapter = Self::new(self.mir, self.graph, format!("interpreter-adapter:{}", node.index()));
        adapter.instance = self.instance;
        adapter.function.parameter_count = plan.parameters.len() as u32;
        let inputs = plan.parameters.iter().map(|_| adapter.register()).collect::<Vec<_>>();
        let adapter_witnesses = witnesses.iter().map(|_| adapter.register()).collect::<Vec<_>>();
        let mut adapter_captures = witnesses;

        // Carry enclosing lexical bindings through both ordinary closures.
        // Globals remain demand instructions in the operand's body.
        let mut references = std::collections::BTreeSet::new();
        let mut pending = vec![operand];
        while let Some(next) = pending.pop() {
            if let Some(slot) = self.mir.hir[next.index()].resolution
                && let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()]
                && self.lookup(symbol).is_some() {
                references.insert(symbol);
            }
            pending.extend(runtime_children(self.mir, next));
        }
        let mut captures = Vec::new();
        for symbol in references {
            captures.push(self.lookup(symbol).unwrap());
            let outer = factory.register();
            adapter_captures.push(outer);
            let inner = adapter.register();
            adapter.locals.push((symbol, inner));
        }
        for instance in self.referenced_instances(operand) {
            if let Some(capture) = self.lookup_instance(instance) {
                captures.push(capture);
                let outer = factory.register();
                adapter_captures.push(outer);
                let inner = adapter.register();
                adapter.local_instances.push((instance, inner));
            }
        }
        factory.function.capture_count = captures.len() as u32;
        adapter.function.capture_count = adapter_captures.len() as u32;

        // Evaluation order matches an ordinary call: operand, then arguments.
        // The sealed plan proves every witness/value pairing; pack stores the
        // witness and existing Val without rebuilding either graph.
        let callee = adapter.expression(operand)?;
        let pack = plan.parameters.iter().any(Option::is_some).then(|| adapter.constant(node,
            Constant::Native(crate::value::NativeFunction::core_dyn(crate::value::CoreDynFunction::Pack))));
        let mut arguments = Vec::new();
        for (index, witness) in plan.parameters.iter().enumerate() {
            if let Some(witness) = witness {
                let base = adapter.register();
                adapter.emit(node, O::Move { dst: base, src: pack.unwrap() });
                let metadata = adapter.register();
                adapter.emit(node, O::Move { dst: metadata, src: adapter_witnesses[*witness as usize] });
                let value = adapter.register();
                adapter.emit(node, O::Move { dst: value, src: inputs[index] });
                adapter.emit(node, O::Call { base, argument_count: 2 });
                arguments.push(base);
            } else { arguments.push(inputs[index]); }
        }
        let base = adapter.register();
        adapter.emit(node, O::Move { dst: base, src: callee });
        for argument in arguments {
            let dst = adapter.register();
            adapter.emit(node, O::Move { dst, src: argument });
        }
        adapter.emit(node, O::TailCall { base, argument_count: plan.parameters.len() as u32 });
        if !adapter.native_links.is_empty() {
            return Err(self.error(node, "interpreter operand contains an unsupported local native relocation"));
        }
        let result = factory.register();
        factory.emit(node, O::MakeClosure { dst: result, function: Box::new(adapter.function), captures: adapter_captures });
        factory.emit(node, O::Return { src: result });
        let result = self.register();
        self.emit(node, O::MakeClosure { dst: result, function: Box::new(factory.function), captures });
        Ok(result)
    }
}
