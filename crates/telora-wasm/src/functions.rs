use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Special, child},
};
use telora_core::mir::{HirId, Role, TypeConstructor};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    pub fn closure(&mut self, node: HirId) -> Result<u32, String> {
        let key = Key {
            node,
            instance: self.key.instance,
            callable: true,
            special: Special::Normal,
        };
        let captures = &self.plan.captures[&key];
        let mut values = vec![];
        for symbol in captures {
            let capture = *self.bindings.get(symbol).ok_or_else(|| {
                format!(
                    "Wasm: unavailable capture {symbol:?} ({}) for {key:?} in {:?}",
                    self.mir.symbols[symbol.index()].name,
                    self.key
                )
            })?;
            values.push(capture);
        }
        self.function_value(node, key, self.effective_ty(node)?, &values)
    }
    pub fn function_value(
        &mut self,
        node: HirId,
        key: Key,
        ty: telora_core::mir::TypeId,
        captures: &[u32],
    ) -> Result<u32, String> {
        let function = *self
            .plan
            .functions
            .get(&key)
            .ok_or("Wasm: callable is absent from sealed plan")?;
        let environment = self.alloc(captures.len() as u32 * 4);
        for (index, &capture) in captures.iter().enumerate() {
            self.extend([
                I::LocalGet(environment),
                I::LocalGet(capture),
                I::I32Store(memory(index as u64 * 4, 2)),
            ]);
        }
        let result = self.value_as(node, ty, FUNCTION_BYTES)?;
        self.emit(I::LocalGet(result));
        self.function_pointer(function);
        self.emit(I::I32Store(memory(DATA, 2)));
        if captures.is_empty() {
            self.store32(result, ENVIRONMENT, 0);
        } else {
            let id = self.table_push(ENVIRONMENTS, environment, captures.len() as u32 * 4);
            self.extend([
                I::LocalGet(result),
                I::LocalGet(id),
                I::I32Const(1),
                I::I32Add,
                I::I32Store(memory(ENVIRONMENT, 2)),
            ]);
        }
        Ok(result)
    }
    pub fn call(&mut self, node: HirId) -> Result<u32, String> {
        let callee_node = child(self.mir, node, Role::Callee)?;
        let callee_ty = self.ty(callee_node)?;
        if self.mir.types[callee_ty.index()].constructor != TypeConstructor::Function {
            return Err("Wasm: call target does not have a sealed function type".into());
        }
        let callee = self.expression(callee_node)?;
        let arguments = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|edge| edge.role == Role::Argument)
            .map(|edge| edge.node)
            .collect::<Vec<_>>();
        let signature = &self.mir.types[callee_ty.index()].arguments;
        if signature.len() != arguments.len() + 1 {
            return Err("Wasm: call arity mismatch".into());
        }
        let mut values = vec![];
        for (index, argument) in arguments.iter().enumerate() {
            let value = self.expression(*argument)?;
            values.push(self.adapt(
                *argument,
                self.effective_ty(*argument)?,
                signature[index],
                value,
            )?);
        }
        self.invoke(callee, &values)
    }
    pub fn invoke(&mut self, callee: u32, values: &[u32]) -> Result<u32, String> {
        let args = self.alloc(values.len() as u32 * 4);
        for (index, &value) in values.iter().enumerate() {
            self.extend([
                I::LocalGet(args),
                I::LocalGet(value),
                I::I32Store(memory(index as u64 * 4, 2)),
            ]);
        }
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(callee),
            I::LocalGet(args),
            I::Call(INVOKE),
            I::LocalSet(result),
        ]);
        self.checked(result);
        Ok(result)
    }
}
