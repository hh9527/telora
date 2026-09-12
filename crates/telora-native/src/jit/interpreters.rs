use super::*;

impl Lower<'_, '_> {
    pub(super) fn load_instance_captures(&mut self, key: functions::Key, environment: ir::Value) -> EmitResult<()> {
        let offset = self.functions.captures.get(&key).map_or(0, Vec::len);
        for (index, (instance, ty)) in self.functions.instance_captures.get(&key).cloned().unwrap_or_default().into_iter().enumerate() {
            let index = self.builder.ins().iconst(types::I64, (index + offset) as i64);
            let value = self.object(key.node, helpers::CAPTURE, ty, environment, index)?;
            self.local_instances.insert(instance, value);
        }
        Ok(())
    }
    fn local_template_instances(&self, symbol: SymbolId) -> Vec<telora_core::mir::GenericInstanceId> {
        let mut selected = std::collections::BTreeSet::new();
        let mut pending = vec![self.function_key.node];
        while let Some(reference) = pending.pop() {
            if let Some(instance) = self.instance_reference(reference)
                && self.mir.generic_instances[instance.index()].concrete {
                selected.insert(instance);
            }
            pending.extend(self.mir.hir[reference.index()].children.iter().map(|edge| edge.node));
        }
        // Follow already sealed instance references, including siblings only
        // referenced by another template body. No substitution is performed.
        let mut pending = selected.iter().copied().collect::<Vec<_>>();
        while let Some(instance) = pending.pop() {
            for &(_, next) in &self.mir.generic_instances[instance.index()].references {
                if self.mir.generic_instances[next.index()].concrete && selected.insert(next) {
                    pending.push(next);
                }
            }
        }
        selected.into_iter().filter(|id| self.mir.generic_instances[id.index()].symbol == symbol).collect()
    }
    pub(super) fn reserve_local_template(&mut self, node: HirId, symbol: SymbolId) -> EmitResult<()> {
        for instance in self.local_template_instances(symbol) {
            if self.local_instances.contains_key(&instance) { continue; }
            let key = functions::Key { instance: Some(instance), ..self.function_key };
            let ty = key.ty(self.mir, node)?;
            if self.mir.types[ty.index()].constructor != TypeConstructor::Function { continue; }
            let zero = self.builder.ins().iconst(types::I64, 0);
            let value = self.object(node, helpers::RESERVE_FUNCTION, TypeKey::try_from(ty)?, zero, zero)?;
            self.local_instances.insert(instance, value);
        }
        Ok(())
    }
    pub(super) fn local_template(&mut self, node: HirId) -> EmitResult<bool> {
        let Some(symbol) = self.mir.hir_symbols[node.index()] else { return Ok(false); };
        if self.mir.symbol_generics[symbol.index()].is_empty() { return Ok(false); }
        if matches!(self.mir.hir[node.index()].kind, HirKind::Binding { kind: telora_core::ast::BindingKind::Decl, .. }) {
            return Ok(true);
        }
        let selected = self.local_template_instances(symbol);
        let previous = self.function_key;
        for instance in selected {
            self.function_key.instance = Some(instance);
            let value = self.expression(child(self.mir, node, Role::Value)?, 0);
            self.function_key = previous;
            let value = value?;
            if let Some(mut target) = self.local_instances.get(&instance).cloned() {
                let ty = TypeKey::try_from(functions::Key { instance: Some(instance), ..previous }.ty(self.mir, node)?)?;
                target.extend(value);
                let data = self.stack_words(&target)?;
                let zero = self.builder.ins().iconst(types::I64, 0);
                self.object(node, helpers::FILL_FUNCTION, ty, data, zero)?;
            } else { self.local_instances.insert(instance, value); }
        }
        Ok(true)
    }
    pub(super) fn interpreter_adapter(&mut self, arguments: &[TypeKey], args: ir::Value, environment: ir::Value) -> EmitResult<()> {
        let key = self.function_key;
        let node = key.node;
        let plan = self.mir.interpreter_plans[node.index()].as_ref().ok_or("interpreter has no sealed plan")?.clone();
        let factory_key = functions::Key { configured_factory: false, ..key };
        let factory_ty = TypeKey::try_from(factory_key.ty(self.mir, node)?)?;
        let mut inputs = Vec::new();
        let mut offset = 0;
        for &ty in arguments {
            let mut value = Vec::new();
            for _ in 0..self.layouts.words(ty)? {
                value.push(self.builder.ins().load(types::I64, MemFlagsData::new(), args, offset));
                offset += 8;
            }
            inputs.push(value);
        }
        if !key.configured_factory {
            if inputs.len() != plan.witness_count as usize { return Err("interpreter witness arity mismatch".into()); }
            let adapter = self.functions.declare(self.mir, functions::Key { configured_factory: true, ..key }, self.module)?;
            let mut words = vec![self.builder.ins().iconst(types::I64, i64::from(adapter.as_u32()))];
            for index in 0..self.layouts.words(factory_ty)? {
                words.push(self.builder.ins().load(types::I64, MemFlagsData::new(), environment, (index * 8) as i32));
            }
            words.extend(inputs.into_iter().flatten());
            let count = self.builder.ins().iconst(types::I64, words.len() as i64);
            let data = self.stack_words(&words)?;
            let result = self.object(node, helpers::INTERPRETER_ADAPTER, self.return_type, data, count)?;
            return self.write_return(&result);
        }
        // Adapter capture zero retains the factory; its captures are the lexical
        // environment of the unevaluated operand, not a precomputed operand.
        let zero = self.builder.ins().iconst(types::I64, 0);
        let factory = self.object(node, helpers::CAPTURE, factory_ty, environment, zero)?;
        let factory = self.stack_words(&factory)?;
        for (index, (symbol, ty)) in self.functions.captures.get(&factory_key).cloned().unwrap_or_default().into_iter().enumerate() {
            let index = self.builder.ins().iconst(types::I64, index as i64);
            let value = self.object(node, helpers::CAPTURE, ty, factory, index)?;
            self.locals.insert(symbol, value);
        }
        let operand = child(self.mir, node, Role::Operand)?;
        self.load_instance_captures(factory_key, factory)?;
        let closure = self.expression(operand, 0)?;
        let signature = TypeKey::try_from(self.ty(operand)?)?;
        if inputs.len() != plan.parameters.len() { return Err("interpreter parameter arity mismatch".into()); }
        let witness_types = self.mir.types[factory_ty.index()].arguments.clone();
        let parameters = self.mir.types[signature.index()].arguments.clone();
        for (index, witness) in plan.parameters.iter().enumerate() {
            if let Some(witness) = witness {
                let capture_index = self.builder.ins().iconst(types::I64, i64::from(*witness) + 1);
                let ty = TypeKey::try_from(witness_types[*witness as usize])?;
                let mut words = self.object(node, helpers::CAPTURE, ty, environment, capture_index)?;
                words.extend_from_slice(&inputs[index]);
                let data = self.stack_words(&words)?;
                let count = self.builder.ins().iconst(types::I64, 0);
                inputs[index] = self.object(node, helpers::DYN_PACK, TypeKey::try_from(parameters[index])?, data, count)?;
            }
        }
        let value = self.invoke_provider(signature, &closure, &inputs)?;
        self.write_return(&value)
    }
}
