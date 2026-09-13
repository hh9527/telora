//! Instantiate local function families in their lexical activation, using sealed IDs.
use crate::{
    emit::Emitter,
    plan::{Key, child},
};
use std::collections::BTreeSet;
use telora_core::mir::{GenericInstanceId, HirId, HirKind, Role, SymbolId};

impl Emitter<'_> {
    fn selected_local_instances(&self, symbol: SymbolId) -> Vec<GenericInstanceId> {
        let mut selected = BTreeSet::new();
        let mut nodes = vec![self.key.node];
        while let Some(node) = nodes.pop() {
            if let Some(id) = self.key.reference(self.mir, node) {
                selected.insert(id);
            }
            nodes.extend(
                self.mir.hir[node.index()]
                    .children
                    .iter()
                    .map(|edge| edge.node),
            );
        }
        let mut pending: Vec<_> = selected.iter().copied().collect();
        while let Some(id) = pending.pop() {
            for &(_, next) in &self.mir.generic_instances[id.index()].references {
                if selected.insert(next) {
                    pending.push(next);
                }
            }
        }
        selected
            .into_iter()
            .filter(|id| {
                self.plan.local_instances.contains(id)
                    && self.mir.generic_instances[id.index()].symbol == symbol
            })
            .collect()
    }
    pub fn reserve_local_instances(&mut self, block: HirId) -> Result<(), String> {
        for edge in &self.mir.hir[block.index()].children {
            if edge.role != Role::Binding {
                continue;
            }
            let Some(symbol) = self.mir.hir_symbols[edge.node.index()] else {
                continue;
            };
            if self.mir.symbol_generics[symbol.index()].is_empty() {
                continue;
            }
            for id in self.selected_local_instances(symbol) {
                if !self.local_instances.contains_key(&id) {
                    let ty = Key {
                        instance: Some(id),
                        ..self.key
                    }
                    .ty(self.mir, edge.node)?;
                    let value = self.alloc(self.width(ty)?);
                    self.local_instances.insert(id, value);
                }
            }
        }
        Ok(())
    }
    pub fn emit_local_template(&mut self, node: HirId) -> Result<bool, String> {
        let Some(symbol) = self.mir.hir_symbols[node.index()] else {
            return Ok(false);
        };
        if self.mir.symbol_generics[symbol.index()].is_empty() {
            return Ok(false);
        }
        if matches!(
            self.mir.hir[node.index()].kind,
            HirKind::Binding {
                kind: telora_core::ast::BindingKind::Decl,
                ..
            }
        ) {
            return Ok(true);
        }
        let body = child(self.mir, node, Role::Value)?;
        let instances = self.selected_local_instances(symbol);
        let previous = self.key;
        for instance in instances {
            self.key.instance = Some(instance);
            let value = self.expression(body);
            let ty = self.key.ty(self.mir, node);
            self.key = previous;
            let value = value?;
            let target = *self
                .local_instances
                .get(&instance)
                .ok_or("Wasm: local instance was not reserved")?;
            self.copy(target, 0, value, self.width(ty?)?);
        }
        Ok(true)
    }
}
