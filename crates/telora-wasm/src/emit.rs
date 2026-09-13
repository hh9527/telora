use crate::{
    abi::*,
    plan::{Key, Plan, child, symbol},
};
use std::collections::BTreeMap;
use telora_core::mir::{HirId, HirKind, Mir, Role, SymbolId, TypeConstructor, TypeId};
use wasm_encoder::{BlockType, Function, Instruction as I, ValType};

pub(crate) struct Emitter<'a> {
    pub mir: &'a Mir,
    pub plan: &'a Plan,
    pub key: Key,
    pub code: Vec<I<'static>>,
    pub locals: Vec<ValType>,
    pub bindings: BTreeMap<SymbolId, u32>,
}

impl<'a> Emitter<'a> {
    pub fn new(mir: &'a Mir, plan: &'a Plan, key: Key) -> Self {
        Self {
            mir,
            plan,
            key,
            code: vec![],
            locals: vec![],
            bindings: BTreeMap::new(),
        }
    }
    pub fn local(&mut self, ty: ValType) -> u32 {
        let index = self.locals.len() as u32 + 2;
        self.locals.push(ty);
        index
    }
    pub fn emit(&mut self, instruction: I<'static>) {
        self.code.push(instruction);
    }
    pub fn extend(&mut self, code: impl IntoIterator<Item = I<'static>>) {
        self.code.extend(code);
    }
    pub fn finish(self) -> Function {
        let mut function = Function::new(self.locals.into_iter().map(|ty| (1, ty)));
        for instruction in self.code {
            function.instruction(&instruction);
        }
        function.instruction(&I::End);
        function
    }
    pub fn ty(&self, node: HirId) -> Result<TypeId, String> {
        self.key.ty(self.mir, node)
    }
    pub fn alloc(&mut self, bytes: u32) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::I32Const(bytes as i32),
            I::Call(ALLOC),
            I::LocalSet(result),
        ]);
        result
    }
    pub fn store32(&mut self, pointer: u32, offset: u64, value: u32) {
        self.extend([
            I::LocalGet(pointer),
            I::I32Const(value as i32),
            I::I32Store(memory(offset, 2)),
        ]);
    }
    pub fn value(&mut self, node: HirId, bytes: u32) -> Result<u32, String> {
        let result = self.alloc(bytes);
        let loc = self.mir.hir[node.index()].location;
        self.store32(result, SOURCE, loc.source.get());
        self.store32(result, START, loc.start);
        self.store32(result, END, loc.end);
        self.store32(result, TYPE, self.ty(node)?.index() as u32);
        Ok(result)
    }
    pub fn scalar(&mut self, node: HirId, bits: i64) -> Result<u32, String> {
        let result = self.value(node, SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(result),
            I::I64Const(bits),
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }
    pub fn bits(&mut self, pointer: u32) {
        self.extend([I::LocalGet(pointer), I::I64Load(memory(DATA, 3))]);
    }
    pub fn checked(&mut self, result: u32) {
        self.extend([
            I::LocalGet(result),
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
        ]);
    }
    pub fn failure(&mut self, node: HirId, code: u32) {
        let location = self.mir.hir[node.index()].location;
        let pointer = self.alloc(16);
        self.store32(pointer, 0, location.source.get());
        self.store32(pointer, 4, location.start);
        self.store32(pointer, 8, location.end);
        self.store32(pointer, 12, code);
        self.extend([
            I::LocalGet(pointer),
            I::GlobalSet(ERROR_GLOBAL),
            I::I32Const(3),
            I::GlobalSet(PHASE_GLOBAL),
            I::I32Const(0),
            I::Return,
        ]);
    }
    pub fn fail_if(&mut self, node: HirId, code: u32) {
        self.emit(I::If(BlockType::Empty));
        self.failure(node, code);
        self.emit(I::End);
    }
    pub fn call_key(&mut self, key: Key) -> Result<u32, String> {
        let function = *self
            .plan
            .functions
            .get(&key)
            .ok_or("Wasm: unregistered function")?;
        let result = self.local(ValType::I32);
        self.extend([
            I::I32Const(0),
            I::I32Const(0),
            I::Call(function),
            I::LocalSet(result),
        ]);
        self.checked(result);
        Ok(result)
    }
    pub fn expression(&mut self, node: HirId) -> Result<u32, String> {
        match &self.mir.hir[node.index()].kind {
            HirKind::Int(value) => self.scalar(node, *value),
            HirKind::Float(value) => self.scalar(node, value.to_bits() as i64),
            HirKind::Tuple if self.mir.hir[node.index()].children.is_empty() => {
                self.value(node, HEADER_BYTES)
            }
            HirKind::Binding { kind, .. } => {
                if !matches!(
                    kind,
                    telora_core::ast::BindingKind::Let | telora_core::ast::BindingKind::Def
                ) {
                    return Err(format!("Wasm: unsupported binding {kind:?}"));
                }
                let result = self.expression(child(self.mir, node, Role::Value)?)?;
                if let Some(symbol) = self.mir.hir_symbols[node.index()] {
                    if let Some(&reserved) = self.bindings.get(&symbol)
                        && self.mir.types[self.ty(node)?.index()].constructor
                            == TypeConstructor::Function
                    {
                        self.extend([
                            I::LocalGet(reserved),
                            I::LocalGet(result),
                            I::I32Const(FUNCTION_BYTES as i32),
                            I::MemoryCopy {
                                src_mem: 0,
                                dst_mem: 0,
                            },
                        ]);
                        return Ok(reserved);
                    }
                    self.bindings.insert(symbol, result);
                }
                Ok(result)
            }
            HirKind::Variable(_) | HirKind::TypeApply => {
                if let Some(instance) = self.key.reference(self.mir, node) {
                    return self.call_key(
                        *self
                            .plan
                            .instances
                            .get(&instance)
                            .ok_or("Wasm: missing sealed instance")?,
                    );
                }
                if matches!(self.mir.hir[node.index()].kind, HirKind::TypeApply) {
                    return self.expression(child(self.mir, node, Role::Callee)?);
                }
                let symbol = symbol(self.mir, node)?;
                if let Some(&local) = self.bindings.get(&symbol) {
                    return Ok(local);
                }
                self.call_key(
                    *self
                        .plan
                        .globals
                        .get(&symbol)
                        .ok_or("Wasm: lexical capture is not available")?,
                )
            }
            HirKind::TypeAscription => self.expression(child(self.mir, node, Role::Value)?),
            HirKind::Block => {
                for edge in &self.mir.hir[node.index()].children {
                    if edge.role == Role::Binding
                        && matches!(
                            self.mir.hir[edge.node.index()].kind,
                            HirKind::Binding {
                                kind: telora_core::ast::BindingKind::Def,
                                ..
                            }
                        )
                        && self.mir.types[self.ty(edge.node)?.index()].constructor
                            == TypeConstructor::Function
                    {
                        let symbol = self.mir.hir_symbols[edge.node.index()]
                            .ok_or("Wasm: local definition has no symbol")?;
                        let reserved = self.alloc(FUNCTION_BYTES);
                        self.bindings.insert(symbol, reserved);
                    }
                }
                for edge in &self.mir.hir[node.index()].children {
                    if edge.role == Role::Binding {
                        self.expression(edge.node)?;
                    }
                }
                self.expression(child(self.mir, node, Role::Result)?)
            }
            HirKind::Binary(op) => self.binary(node, *op),
            HirKind::Unary(op) => self.unary(node, *op),
            HirKind::If => {
                let condition_node = child(self.mir, node, Role::Condition)?;
                if self.mir.types[self.ty(condition_node)?.index()].constructor
                    != TypeConstructor::Bool
                {
                    return Err("Wasm: condition is not sealed Bool".into());
                }
                let condition = self.expression(condition_node)?;
                let result = self.local(ValType::I32);
                self.bits(condition);
                self.extend([I::I64Eqz, I::If(BlockType::Empty)]);
                let no = self.expression(child(self.mir, node, Role::Else)?)?;
                self.extend([I::LocalGet(no), I::LocalSet(result), I::Else]);
                let yes = self.expression(child(self.mir, node, Role::Then)?)?;
                self.extend([I::LocalGet(yes), I::LocalSet(result), I::End]);
                Ok(result)
            }
            HirKind::Return => {
                let value = self.expression(child(self.mir, node, Role::Value)?)?;
                self.extend([I::LocalGet(value), I::Return]);
                Ok(value)
            }
            HirKind::Closure => self.closure(node),
            HirKind::Call => self.call(node),
            other => Err(format!(
                "Wasm: unsupported expression {other:?} at {:?}",
                self.mir.hir[node.index()].location
            )),
        }
    }
}

pub(crate) fn compile(mir: &Mir, plan: &Plan, key: Key) -> Result<Function, String> {
    let mut emit = Emitter::new(mir, plan, key);
    if key.callable {
        for (index, symbol) in plan.captures[&key].iter().enumerate() {
            let local = emit.local(ValType::I32);
            emit.extend([
                I::LocalGet(0),
                I::I32Load(memory(index as u64 * 4, 2)),
                I::LocalSet(local),
            ]);
            emit.bindings.insert(*symbol, local);
        }
        for (index, edge) in mir.hir[key.node.index()]
            .children
            .iter()
            .filter(|e| e.role == Role::Parameter)
            .enumerate()
        {
            let symbol =
                mir.hir_symbols[edge.node.index()].ok_or("Wasm: parameter has no stable symbol")?;
            let local = emit.local(ValType::I32);
            emit.extend([
                I::LocalGet(1),
                I::I32Load(memory(index as u64 * 4, 2)),
                I::LocalSet(local),
            ]);
            emit.bindings.insert(symbol, local);
        }
        let value = emit.expression(child(mir, key.node, Role::Body)?)?;
        emit.emit(I::LocalGet(value));
    } else {
        let offset = plan.demands[&key];
        // 0 = empty, 1 = evaluating/failed, 2 = ready. A failed session is terminal.
        emit.extend([
            I::I32Const(offset as i32),
            I::I32Load(memory(0, 2)),
            I::I32Const(2),
            I::I32Eq,
            I::If(BlockType::Empty),
            I::I32Const(offset as i32),
            I::I32Load(memory(4, 2)),
            I::Return,
            I::End,
            I::I32Const(offset as i32),
            I::I32Load(memory(0, 2)),
        ]);
        emit.fail_if(key.node, ERROR_CYCLE);
        emit.extend([
            I::I32Const(offset as i32),
            I::I32Const(1),
            I::I32Store(memory(0, 2)),
        ]);
        let value = emit.expression(key.node)?;
        emit.extend([
            I::I32Const(offset as i32),
            I::LocalGet(value),
            I::I32Store(memory(4, 2)),
            I::I32Const(offset as i32),
            I::I32Const(2),
            I::I32Store(memory(0, 2)),
            I::LocalGet(value),
        ]);
    }
    Ok(emit.finish())
}
