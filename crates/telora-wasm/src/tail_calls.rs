//! Tail positions follow closed control flow, with no pending value adjustment.
use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::mir::{HirId, HirKind, Role, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn plan_tail_calls(&mut self) -> Result<(), String> {
        let signature = self.ty(self.key.node)?;
        let output = *self.mir.types[signature.index()]
            .arguments
            .last()
            .ok_or("Wasm: tail call requires a sealed return type")?;
        let body = child(self.mir, self.key.node, Role::Body)?;
        self.mark_tail(body, output)?;
        // Explicit returns can occur outside the body's final expression. Each
        // nested closure receives its own analysis when its function is emitted.
        let mut pending = vec![body];
        while let Some(node) = pending.pop() {
            if matches!(
                self.mir.hir[node.index()].kind,
                HirKind::Closure | HirKind::Interpreter
            ) {
                continue;
            }
            if matches!(self.mir.hir[node.index()].kind, HirKind::Return) {
                self.mark_tail(child(self.mir, node, Role::Value)?, output)?;
            }
            pending.extend(
                self.mir.hir[node.index()]
                    .children
                    .iter()
                    .map(|edge| edge.node),
            );
        }
        Ok(())
    }
    fn mark_tail(&mut self, node: HirId, output: TypeId) -> Result<(), String> {
        let mut pending = vec![node];
        while let Some(node) = pending.pop() {
            if matches!(self.mir.hir[node.index()].kind, HirKind::Return) {
                pending.push(child(self.mir, node, Role::Value)?);
                continue;
            }
            // A constructor check or metadata widening must still run after the
            // operand returns; such a call is not a tail call in the lowered ABI.
            if self.mir.value_adjustments[node.index()].is_some() || self.ty(node)? != output {
                continue;
            }
            let roles: &[Role] = match self.mir.hir[node.index()].kind {
                HirKind::Call => {
                    self.tail_calls.insert(node);
                    continue;
                }
                HirKind::Block => &[Role::Result],
                HirKind::If | HirKind::IfLet => &[Role::Then, Role::Else],
                HirKind::LetElse => &[Role::Body, Role::Else],
                HirKind::TypeAscription => &[Role::Value],
                HirKind::Match => {
                    let bodies = self.mir.hir[node.index()]
                        .children
                        .iter()
                        .filter(|edge| edge.role == Role::Arm)
                        .map(|edge| child(self.mir, edge.node, Role::Value))
                        .collect::<Result<Vec<_>, _>>()?;
                    pending.extend(bodies.into_iter().rev());
                    continue;
                }
                _ => continue,
            };
            for role in roles.iter().rev() {
                pending.push(child(self.mir, node, *role)?);
            }
        }
        Ok(())
    }

    pub fn tail_invoke(
        &mut self,
        callee: u32,
        values: &[u32],
        origin: Option<u32>,
    ) -> Result<u32, String> {
        let args = self.argument_array(values, origin);
        let environment = self.local(ValType::I32);
        self.extend([
            I::LocalGet(callee),
            I::I32Load(memory(ENVIRONMENT, 2)),
            I::LocalTee(environment),
            I::I32Eqz,
            I::If(BlockType::Result(ValType::I32)),
            I::I32Const(0),
            I::Else,
            I::LocalGet(environment),
            I::End,
            I::LocalGet(args),
            I::LocalGet(callee),
            I::I32Load(memory(DATA, 2)),
            I::ReturnCallIndirect {
                type_index: CALL_TYPE,
                table_index: 0,
            },
        ]);
        // Only referenced by unreachable continuations emitted by enclosing
        // expression builders. No value or success is fabricated at runtime.
        Ok(self.local(ValType::I32))
    }
}
