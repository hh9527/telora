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
        let captures = self
            .plan
            .captures
            .get(&key)
            .ok_or_else(|| format!("Wasm: closure absent from sealed plan: {key:?}"))?;
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
        for instance in &self.plan.instance_captures[&key] {
            values.push(*self.local_instances.get(instance).ok_or_else(|| {
                format!("Wasm: missing local instance capture {instance:?} for {key:?}")
            })?);
        }
        if matches!(
            self.mir.hir[node.index()].kind,
            telora_core::mir::HirKind::Interpreter
        ) {
            let cache = self.local(ValType::I32);
            self.extend([I::I32Const(0), I::LocalSet(cache)]);
            values.push(cache);
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
        // Even an empty environment gives each evaluated closure an identity.
        let raw_parent = key.special == Special::Configured
            && matches!(self.mir.hir[node.index()].kind, telora_core::mir::HirKind::Interpreter);
        let bytes = captures.len() as u32 * 4 | if raw_parent { ENV_RAW_PARENT } else { 0 };
        let id = self.table_push(ENVIRONMENTS, environment, bytes);
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I32Const(1),
            I::I32Add,
            I::I32Store(memory(ENVIRONMENT, 2)),
        ]);
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
        self.extend([I::LocalGet(callee), I::I32Load(memory(DATA, 2)), I::I32Eqz]);
        self.fail_if(node, ERROR_UNINITIALIZED_CALL);
        let location = self.mir.hir[node.index()].location;
        for (index, word) in [location.source.get(), location.start, location.end].into_iter().enumerate() {
            self.extend([I::I32Const(word as i32), I::GlobalSet(CALL_SOURCE_GLOBAL + index as u32)]);
        }
        if self.tail_calls.contains(&node) {
            self.tail_invoke(callee, &values)
        } else {
            self.invoke(callee, &values)
        }
    }
    pub fn argument_array(&mut self, values: &[u32]) -> u32 {
        let args = self.alloc(values.len() as u32 * 4);
        for (index, &value) in values.iter().enumerate() {
            self.extend([
                I::LocalGet(args),
                I::LocalGet(value),
                I::I32Store(memory(index as u64 * 4, 2)),
            ]);
        }
        args
    }
    pub fn invoke(&mut self, callee: u32, values: &[u32]) -> Result<u32, String> {
        let args = self.argument_array(values);
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
