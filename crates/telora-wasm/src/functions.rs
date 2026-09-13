use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, child},
};
use telora_core::mir::{HirId, Role, TypeConstructor};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    pub fn closure(&mut self, node: HirId) -> Result<u32, String> {
        let key = Key {
            node,
            instance: self.key.instance,
            callable: true,
        };
        let function = *self
            .plan
            .functions
            .get(&key)
            .ok_or("Wasm: closure is absent from sealed plan")?;
        let captures = &self.plan.captures[&key];
        let environment = self.alloc(captures.len() as u32 * 4);
        for (index, symbol) in captures.iter().enumerate() {
            let capture = *self
                .bindings
                .get(symbol)
                .ok_or_else(|| format!("Wasm: unavailable capture {symbol:?}"))?;
            self.extend([
                I::LocalGet(environment),
                I::LocalGet(capture),
                I::I32Store(memory(index as u64 * 4, 2)),
            ]);
        }
        let result = self.value(node, FUNCTION_BYTES)?;
        self.store32(result, DATA, function);
        self.extend([
            I::LocalGet(result),
            I::LocalGet(environment),
            I::I32Store(memory(ENVIRONMENT, 2)),
        ]);
        self.extend([
            I::LocalGet(result),
            I::I64Const(0),
            I::I64Store(memory(24, 3)),
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
        let args = self.alloc(arguments.len() as u32 * 4);
        for (index, argument) in arguments.iter().enumerate() {
            if self.ty(*argument)? != signature[index] {
                return Err("Wasm: argument needs its sealed boundary adaptation".into());
            }
            let value = self.expression(*argument)?;
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
