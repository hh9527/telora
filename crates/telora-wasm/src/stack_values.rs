//! Expression boundaries carry a value address on the Wasm operand stack.
//! Scratch locals belong to that evaluation, not to the returned heap value.
use crate::emit::Emitter;
use std::collections::{BTreeMap, BTreeSet};
use telora_core::mir::{GenericInstanceId, HirId, HirKind, SymbolId, TypeId};
use wasm_encoder::{Instruction as I, ValType};

#[derive(Default)]
pub(crate) struct LocalScopes {
    free: Vec<u32>,
    scopes: Vec<Vec<u32>>,
}

impl LocalScopes {
    pub fn take(&mut self, types: &[ValType], ty: ValType) -> Option<u32> {
        let position = self
            .free
            .iter()
            .rposition(|&id| types[id as usize - 2] == ty)?;
        Some(self.free.swap_remove(position))
    }

    pub fn track(&mut self, id: u32) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(id);
        }
    }

    pub fn enter(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn leave(&mut self) {
        self.leave_preserving(&BTreeSet::new());
    }

    pub fn leave_preserving(&mut self, live: &BTreeSet<u32>) {
        for id in self.scopes.pop().expect("balanced local scope") {
            if live.contains(&id) {
                // Transfer ownership to the consumer's scope, rather than
                // freeing the result while a continuation still needs it.
                self.track(id);
            } else {
                self.free.push(id);
            }
        }
    }
}

pub(crate) struct ExpressionBindings {
    bindings: BTreeMap<SymbolId, u32>,
    instances: BTreeMap<GenericInstanceId, u32>,
}

impl Emitter<'_> {
    pub fn enter_expression(&mut self, node: HirId) -> Option<ExpressionBindings> {
        self.local_scopes.enter();
        matches!(
            self.mir.hir[node.index()].kind,
            HirKind::Block | HirKind::Match | HirKind::IfLet | HirKind::LetElse
        )
        .then(|| ExpressionBindings {
            bindings: self.bindings.clone(),
            instances: self.local_instances.clone(),
        })
    }

    pub fn leave_expression(&mut self, value: u32, saved: Option<ExpressionBindings>) {
        if let Some(saved) = saved {
            self.bindings = saved.bindings;
            self.local_instances = saved.instances;
        }
        let live = std::iter::once(value)
            .chain(self.bindings.values().copied())
            .chain(self.local_instances.values().copied())
            .collect();
        self.local_scopes.leave_preserving(&live);
    }

    /// Leave an adapted value address on the stack. Only lexical bindings that
    /// existed before this expression survive; escaping closures retain heap
    /// values in their environments, never references to Wasm locals.
    pub fn expression_on_stack(&mut self, node: HirId, target: TypeId) -> Result<(), String> {
        let bindings = self.bindings.clone();
        let instances = self.local_instances.clone();
        self.local_scopes.enter();
        let emitted = (|| {
            let actual = self.effective_ty(node)?;
            let value = self.expression(node)?;
            let value = self.adapt(node, actual, target, value)?;
            self.emit(I::LocalGet(value));
            Ok(())
        })();
        self.bindings = bindings;
        self.local_instances = instances;
        self.local_scopes.leave();
        emitted
    }
}
