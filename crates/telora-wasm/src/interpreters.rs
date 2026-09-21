//! Type-bound higher-order adapters capturing an already evaluated function.
use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Special, child},
};
use telora_core::mir::{Role, TypeConstructor as T};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    pub fn interpreter(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let factory = Key {
            special: Special::Normal,
            ..self.key
        };
        let outer = self.mir.types[factory.ty(self.mir, node)?.index()]
            .arguments
            .clone();
        let plan = self.mir.interpreter_plans[node.index()]
            .as_ref()
            .ok_or("Wasm: interpreter has no sealed plan")?
            .clone();
        if outer.len() != plan.witness_count as usize + 1 {
            return Err("Wasm: interpreter witness arity mismatch".into());
        }
        let inner_ty = *outer.last().unwrap();
        let inner = self.mir.types[inner_ty.index()].arguments.clone();
        let operand = child(self.mir, node, Role::Operand)?;
        let erased = self.mir.types[self.ty(operand)?.index()].arguments.clone();
        if inner.len() != plan.parameters.len() + 1 || erased.len() != inner.len() {
            return Err("Wasm: interpreter parameter arity mismatch".into());
        }
        for (index, witness) in plan.parameters.iter().enumerate() {
            if let Some(witness) = witness {
                let ty = *outer
                    .get(*witness as usize)
                    .ok_or("Wasm: invalid interpreter witness")?;
                let desc = &self.mir.types[ty.index()];
                if desc.constructor != T::TypeOf
                    || desc.arguments != [inner[index]]
                    || self.mir.types[erased[index].index()].constructor != T::Dyn
                {
                    return Err("Wasm: interpreter witness is not sealed to its input".into());
                }
            } else if inner[index] != erased[index] {
                return Err("Wasm: interpreter passthrough type mismatch".into());
            }
        }
        let callee = self.local(ValType::I32);
        self.extend([
            I::LocalGet(0),
            I::I32Load(memory(0, 2)),
            I::LocalSet(callee),
        ]);
        if self.key.special == Special::Normal {
            return self.function_value(
                node,
                Key {
                    special: Special::Configured,
                    ..factory
                },
                inner_ty,
                &[(callee, self.ty(operand)?)],
            );
        }
        let mut inputs = Vec::new();
        for (index, witness) in plan.parameters.iter().enumerate() {
            let input = self.parameter(index as u32);
            if witness.is_none() {
                inputs.push(input);
                continue;
            }
            let id = self.table_push(VALUES, input, self.width(inner[index])?);
            let packed = self.value_as(node, erased[index], DYN_BYTES)?;
            self.store32(packed, DATA, inner[index].index() as u32);
            self.store32(packed, DATA + 4, 1);
            self.extend([
                I::LocalGet(packed),
                I::LocalGet(id),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA + 8, 3)),
            ]);
            inputs.push(packed);
        }
        self.invoke(callee, &inputs)
    }
}
