//! Expression boundaries carry a value address on the Wasm operand stack.
//! Scratch locals belong to that evaluation, not to the returned heap value.
use crate::emit::Emitter;
use telora_core::mir::{HirId, TypeId};
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

    fn enter(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn leave(&mut self) {
        self.free
            .extend(self.scopes.pop().expect("balanced local scope"));
    }
}

impl Emitter<'_> {
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
