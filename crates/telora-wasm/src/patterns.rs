use crate::{abi::*, emit::Emitter, enums::selection, plan::child};
use telora_core::mir::{HirId, HirKind, MemberSelection, ResolveState, Role, TypeConstructor as T};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    /// Runs inside one arm block. Failed evidence branches to that block's end.
    fn pattern(&mut self, node: HirId, value: u32) -> Result<(), String> {
        let syntax = &self.mir.hir[node.index()];
        if matches!(syntax.kind, HirKind::Wildcard) {
            return Ok(());
        }
        if matches!(syntax.kind, HirKind::PatternName(_)) {
            let symbol =
                self.mir.hir_symbols[node.index()].ok_or("Wasm: pattern binder is missing")?;
            if self.mir.symbols[symbol.index()].resolution == ResolveState::Bound(symbol) {
                self.bindings.insert(symbol, value);
                return Ok(());
            }
        }
        if let Some(selected) = selection(self.mir, node) {
            match selected {
                MemberSelection::Boolean(boolean) => {
                    self.bits(value);
                    self.extend([I::I64Const(i64::from(boolean)), I::I64Ne, I::BrIf(0)]);
                    return Ok(());
                }
                MemberSelection::EnumVariant { index } => {
                    self.bits(value);
                    self.extend([I::I64Const(index as i64), I::I64Ne, I::BrIf(0)]);
                    if let Ok(pattern) = child(self.mir, node, Role::Pattern) {
                        let payload = self.enum_payload(self.ty(node)?, index, value)?;
                        self.pattern(pattern, payload)?;
                    }
                    return Ok(());
                }
                MemberSelection::NewtypePattern => {
                    let payload = self.table_data(NEWTYPES, value, DATA);
                    return self.pattern(child(self.mir, node, Role::Pattern)?, payload);
                }
                _ => {}
            }
        }
        match &syntax.kind {
            HirKind::Int(integer) => {
                self.bits(value);
                self.extend([I::I64Const(*integer), I::I64Ne, I::BrIf(0)]);
            }
            HirKind::Float(float) => {
                self.bits(value);
                self.extend([
                    I::F64ReinterpretI64,
                    I::I64Const(float.to_bits() as i64),
                    I::F64ReinterpretI64,
                    I::F64Ne,
                    I::BrIf(0),
                ]);
            }
            HirKind::String(text) => {
                let expected = self.text(node, text.as_bytes())?;
                self.extend([
                    I::LocalGet(value),
                    I::LocalGet(expected),
                    I::Call(STRING_COMPARE),
                    I::BrIf(0),
                ]);
            }
            HirKind::TuplePattern | HirKind::StructPattern => {
                let ty = self.ty(node)?;
                let object = self.plan.layouts[ty.index()]
                    .object
                    .as_ref()
                    .ok_or("Wasm: pattern has no closed aggregate layout")?;
                let data = self.table_data(RECORDS, value, DATA);
                let patterns = if matches!(syntax.kind, HirKind::TuplePattern) {
                    syntax
                        .children
                        .iter()
                        .filter(|e| e.role == Role::Item)
                        .enumerate()
                        .map(|(index, edge)| {
                            Ok((
                                edge.node,
                                object
                                    .members
                                    .get(index)
                                    .and_then(|m| m.offset)
                                    .ok_or("Wasm: tuple pattern arity mismatch")?,
                            ))
                        })
                        .collect::<Result<Vec<_>, String>>()?
                } else {
                    syntax
                        .children
                        .iter()
                        .filter(|e| e.role == Role::Field)
                        .map(|edge| {
                            let name = child(self.mir, edge.node, Role::Name)?;
                            let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                                return Err("Wasm: record pattern has no field name".into());
                            };
                            let offset = object
                                .members
                                .iter()
                                .find(|m| &m.name == name)
                                .and_then(|m| m.offset)
                                .ok_or("Wasm: record pattern field not sealed")?;
                            Ok((child(self.mir, edge.node, Role::Pattern)?, offset))
                        })
                        .collect::<Result<Vec<_>, String>>()?
                };
                for (pattern, offset) in patterns {
                    let field = self.local(ValType::I32);
                    self.extend([
                        I::LocalGet(data),
                        I::I32Const(offset as i32),
                        I::I32Add,
                        I::LocalSet(field),
                    ]);
                    self.pattern(pattern, field)?;
                }
            }
            _ => {
                return Err(format!(
                    "Wasm: pattern is not implemented: {:?}",
                    syntax.kind
                ));
            }
        }
        Ok(())
    }
    pub fn pattern_branch(&mut self, node: HirId) -> Result<u32, String> {
        let value = self.expression(child(self.mir, node, Role::Value)?)?;
        let result = self.local(ValType::I32);
        self.emit(I::Block(BlockType::Empty));
        if matches!(self.mir.hir[node.index()].kind, HirKind::Match) {
            let arms = self.mir.hir[node.index()]
                .children
                .iter()
                .filter(|e| e.role == Role::Arm)
                .map(|e| e.node)
                .collect::<Vec<_>>();
            for arm in arms {
                self.emit(I::Block(BlockType::Empty));
                self.pattern(child(self.mir, arm, Role::Pattern)?, value)?;
                if let Ok(guard) = child(self.mir, arm, Role::Guard) {
                    let guard = self.expression(guard)?;
                    self.bits(guard);
                    self.extend([I::I64Eqz, I::BrIf(0)]);
                }
                let body = child(self.mir, arm, Role::Value)?;
                let value = self.expression(body)?;
                let value = self.adapt(
                    body,
                    self.effective_ty(body)?,
                    self.effective_ty(node)?,
                    value,
                )?;
                self.extend([I::LocalGet(value), I::LocalSet(result), I::Br(1), I::End]);
            }
            self.failure(node, ERROR_MATCH);
        } else {
            self.emit(I::Block(BlockType::Empty));
            self.pattern(child(self.mir, node, Role::Pattern)?, value)?;
            let then = if matches!(self.mir.hir[node.index()].kind, HirKind::LetElse) {
                Role::Body
            } else {
                Role::Then
            };
            let body = child(self.mir, node, then)?;
            let yes = self.expression(body)?;
            let yes = self.adapt(
                body,
                self.effective_ty(body)?,
                self.effective_ty(node)?,
                yes,
            )?;
            self.extend([I::LocalGet(yes), I::LocalSet(result), I::Br(1), I::End]);
            let body = child(self.mir, node, Role::Else)?;
            let no = self.expression(body)?;
            let no = self.adapt(body, self.effective_ty(body)?, self.effective_ty(node)?, no)?;
            self.extend([I::LocalGet(no), I::LocalSet(result)]);
        }
        self.emit(I::End);
        Ok(result)
    }
    pub fn propagate(&mut self, node: HirId) -> Result<u32, String> {
        let operand = child(self.mir, node, Role::Operand)?;
        let input = self.ty(operand)?;
        let function = self.ty(self.key.node)?;
        let output = *self.mir.types[function.index()]
            .arguments
            .last()
            .ok_or("Wasm: propagation has no return type")?;
        let family = &self.mir.types[input.index()].constructor;
        if !matches!(family, T::Option | T::Result)
            || family != &self.mir.types[output.index()].constructor
        {
            return Err("Wasm: propagation return family differs from its sealed contract".into());
        }
        let value = self.expression(operand)?;
        self.extend([
            I::LocalGet(value),
            I::I32Load(memory(DATA, 2)),
            I::I32Eqz,
            I::If(BlockType::Empty),
        ]);
        let payload = if self.plan.layouts[input.index()].variants[0]
            .type_id
            .is_some()
        {
            Some(self.enum_payload(input, 0, value)?)
        } else {
            None
        };
        let failure = self.enum_value(node, output, 0, payload)?;
        self.copy(failure, 0, value, 12);
        self.extend([I::LocalGet(failure), I::Return, I::End]);
        self.enum_payload(input, 1, value)
    }
}
