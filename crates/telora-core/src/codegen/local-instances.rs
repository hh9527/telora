use super::*;
use std::collections::BTreeSet;

impl Emitter<'_> {
    pub(super) fn lookup_instance(&self, instance: GenericInstanceId) -> Option<R> {
        self.local_instances.iter().rev().find(|(id, _)| *id == instance).map(|(_, value)| *value)
    }

    pub(super) fn referenced_instances(&self, root: HirId) -> BTreeSet<GenericInstanceId> {
        let mut pending = vec![root];
        let mut instances = vec![];
        while let Some(node) = pending.pop() {
            let instance = if let Some(instance) = self.instance {
                self.mir.generic_instances[instance.index()].reference(node)
            } else { self.mir.reference_instances[node.index()] };
            instances.extend(instance);
            pending.extend(runtime_children(self.mir, node));
        }
        let mut seen = BTreeSet::new();
        while let Some(instance) = instances.pop() {
            if !seen.insert(instance) { continue; }
            instances.extend(self.mir.generic_instances[instance.index()].references.iter().map(|(_, id)| *id));
        }
        seen
    }

    pub(super) fn allocate_local_instances(&mut self, node: HirId, bindings: &[HirId]) {
        let symbols = bindings.iter().filter_map(|binding| self.mir.hir_symbols[binding.index()]).collect::<BTreeSet<_>>();
        for instance in self.referenced_instances(node) {
            if symbols.contains(&self.mir.generic_instances[instance.index()].symbol)
                && self.lookup_instance(instance).is_none() {
                let dst = self.register();
                let signature = self.mir.generic_instances[instance.index()].signature;
                if self.mir.types[signature.index()].constructor == TypeConstructor::Function {
                    self.emit(node, O::AllocFunc { dst, static_id: None });
                }
                self.local_instances.push((instance, dst));
            }
        }
    }

    pub(super) fn local_instance_binding(&mut self, node: HirId, symbol: SymbolId) -> Result<R, Diagnostic> {
        let instances = self.local_instances.iter().copied().filter(|(instance, _)| {
            self.mir.generic_instances[instance.index()].symbol == symbol
        }).collect::<Vec<_>>();
        let value = self.child(node, Role::Value);
        if !matches!(self.mir.hir[value.index()].kind,
            HirKind::Closure | HirKind::Interpreter | HirKind::Variable(_) | HirKind::Field | HirKind::TypeApply) {
            return Err(self.error(node, "local generic initializer requires a non-expansive value"));
        }
        let previous = self.instance;
        for &(instance, target) in &instances {
            self.instance = Some(instance);
            let source = self.expression(value);
            self.instance = previous;
            let signature = self.mir.generic_instances[instance.index()].signature;
            if self.mir.types[signature.index()].constructor == TypeConstructor::Function {
                self.emit(node, O::SealFunc { target, source: source? });
            } else {
                self.emit(node, O::Move { dst: target, src: source? });
            }
        }
        if let Some((_, value)) = instances.first() { return Ok(*value); }
        // Unused non-expansive values have no execution effects.
        let dst = self.register();
        self.emit(node, O::MakeTuple { dst, items: vec![] });
        Ok(dst)
    }
}
