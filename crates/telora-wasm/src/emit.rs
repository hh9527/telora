use crate::{
    abi::*,
    plan::{Key, Plan, Special, child, symbol},
};
use std::collections::{BTreeMap, BTreeSet};
use telora_core::mir::{HirId, HirKind, Mir, Role, SymbolId, TypeConstructor, TypeId};
use wasm_encoder::{BlockType, Function, Instruction as I, ValType};

pub(crate) struct Emitter<'a> {
    pub mir: &'a Mir,
    pub plan: &'a Plan,
    pub key: Key,
    code: Vec<I<'static>>,
    pub parameter_count: u32,
    frame_base: Option<u32>,
    scratch_bytes: u32,
    function_pointers: BTreeMap<usize, u32>,
    static_pointers: BTreeSet<usize>,
    pub locals: Vec<ValType>,
    pub local_scopes: crate::stack_values::LocalScopes,
    pub bindings: BTreeMap<SymbolId, u32>,
    pub local_instances: BTreeMap<telora_core::mir::GenericInstanceId, u32>,
    pub tail_calls: BTreeSet<HirId>,
}

impl<'a> Emitter<'a> {
    pub fn new(mir: &'a Mir, plan: &'a Plan, key: Key) -> Self {
        Self {
            mir,
            plan,
            key,
            code: vec![],
            parameter_count: 3,
            frame_base: key.callable.then_some(3),
            scratch_bytes: 0,
            function_pointers: BTreeMap::new(),
            static_pointers: BTreeSet::new(),
            locals: if key.callable {
                vec![ValType::I32]
            } else {
                vec![]
            },
            local_scopes: Default::default(),
            bindings: BTreeMap::new(),
            local_instances: BTreeMap::new(),
            tail_calls: BTreeSet::new(),
        }
    }
    pub fn local(&mut self, ty: ValType) -> u32 {
        if let Some(index) = self
            .local_scopes
            .take(&self.locals, ty, self.parameter_count)
        {
            // Existing emitters can rely on Wasm's default-zero local value.
            // A recycled slot must provide the same starting value.
            self.emit(match ty {
                ValType::I32 => I::I32Const(0),
                ValType::I64 => I::I64Const(0),
                _ => unreachable!("codegen scratch locals are integer words"),
            });
            self.emit(I::LocalSet(index));
            self.root_clear(index);
            self.local_scopes.track(index);
            return index;
        }
        let index = self.locals.len() as u32 + self.parameter_count;
        self.locals.push(ty);
        self.local_scopes.track(index);
        index
    }
    pub fn emit(&mut self, instruction: I<'static>) {
        if let I::Call(index) = instruction
            && let Some(&result) = self.plan.function_results.get(&index)
        {
            let output = result.and_then(|ty| {
                if self.plan.demand_functions.contains(&index)
                    && !self.plan.layouts[ty.index()].plain
                {
                    // Ready demands own these persistent heads; borrowing one
                    // avoids per-reference allocation for global callables.
                    None
                } else {
                    Some(
                        self.output_alloc(ty)
                            .expect("sealed function result layout"),
                    )
                }
            });
            self.code.push(output.map_or(I::I32Const(0), I::LocalGet));
        }
        let allocation = matches!(self.code.last(), Some(I::Call(ALLOC)));
        let result_type = match self.code.last() {
            Some(I::Call(index)) => self.plan.function_results.get(index).copied().flatten(),
            _ => None,
        };
        let assigned = match instruction {
            I::LocalSet(local) | I::LocalTee(local) => Some(local),
            _ => None,
        };
        self.code.push(instruction);
        if allocation && let Some(local) = assigned {
            self.root_storage(local);
        }
        if let (Some(ty), Some(local)) = (result_type, assigned) {
            self.root_value(local, ty);
        }
    }
    pub fn extend(&mut self, code: impl IntoIterator<Item = I<'static>>) {
        for instruction in code {
            self.emit(instruction);
        }
    }
    pub fn function_pointer(&mut self, index: u32) {
        self.function_pointers.insert(self.code.len(), index);
        self.emit(I::I32Const(0));
    }
    pub fn static_base(&mut self) -> u32 {
        let local = self.local(ValType::I32);
        self.static_pointers.insert(self.code.len());
        self.extend([I::I32Const(0), I::LocalSet(local)]);
        local
    }
    pub fn finish(mut self) -> crate::object::ObjectFunction {
        let demand = (!self.key.callable).then(|| self.plan.demands[&self.key]);
        let frame = self.frame_base.filter(|_| self.parameter_count == 3);
        let result = self.locals.len() as u32 + self.parameter_count;
        let result_width = (self.parameter_count == 3)
            .then(|| self.key.result_type(self.mir).expect("sealed result type"))
            .flatten()
            .map(|ty| self.width(ty).expect("sealed result layout"));
        if demand.is_some() || result_width.is_some() || frame.is_some() {
            self.locals.push(ValType::I32);
        }
        let scratch = self.locals.len() as u32 + self.parameter_count;
        self.locals.extend([ValType::I32, ValType::I64]);
        let root_slots = self.parameter_count + self.locals.len() as u32;
        let mut function = crate::object::ObjectFunction::new(
            Function::new(self.locals.drain(..).map(|ty| (1, ty))),
            scratch,
        );
        let count = FIRST_FUNCTION + self.plan.functions.len() as u32 + self.plan.generated_helpers;
        if let Some(base) = frame {
            for instruction in [
                I::I32Const(self.scratch_bytes as i32),
                I::LocalGet(1),
                I::Call(FRAME_ENTER),
                I::LocalTee(base),
                I::I32Const(8),
                I::I32Sub,
                I::I32Load(memory(0, 2)),
                I::LocalSet(1),
            ] {
                function.linked_instruction(&instruction, count);
            }
        }
        self.root_prologue(&mut function, count, root_slots);
        // All ordinary exits share one epilogue, including raw-result helpers.
        // This is the boundary for frame cleanup and caller-owned result handoff.
        // ReturnCallIndirect remains a separate tail-transfer path.
        function.instruction(&I::Block(wasm_encoder::BlockType::Result(ValType::I32)));
        let mut depth = 0;
        for (index, instruction) in self.code.into_iter().enumerate() {
            match instruction {
                I::Return => {
                    function.instruction(&I::Br(depth));
                    continue;
                }
                I::Block(_) | I::Loop(_) | I::If(_) => depth += 1,
                I::End => depth -= 1,
                _ => {}
            }
            if let Some(&symbol) = self.function_pointers.get(&index) {
                function.function_pointer(symbol);
            } else if self.static_pointers.contains(&index) {
                function.memory_pointer(count + GLOBAL_COUNT + 1);
            } else {
                function.linked_instruction(&instruction, count);
            }
        }
        function.instruction(&I::End);
        if let Some(offset) = demand {
            // All exits, including failure propagation, pass through this
            // boundary. The second word holds either a value or failure origin.
            for instruction in [
                I::LocalSet(result),
                I::I32Const(offset as i32),
                I::LocalGet(result),
                I::I32Eqz,
                I::If(wasm_encoder::BlockType::Result(ValType::I32)),
                I::I32Const(3),
                I::Else,
                I::I32Const(2),
                I::End,
                I::I32Store(memory(0, 2)),
                I::I32Const(offset as i32),
                I::LocalGet(result),
                I::I32Eqz,
                I::If(wasm_encoder::BlockType::Result(ValType::I32)),
                I::GlobalGet(ERROR_GLOBAL),
                I::Else,
                I::LocalGet(result),
                I::End,
                I::I32Store(memory(4, 2)),
                I::LocalGet(result),
            ] {
                function.linked_instruction(&instruction, count);
            }
        }
        if let Some(bytes) = result_width {
            if frame.is_some() {
                for instruction in [
                    I::LocalSet(result),
                    I::LocalGet(result),
                    I::I32Eqz,
                    I::I32Eqz,
                    I::LocalGet(2),
                    I::I32Eqz,
                    I::I32And,
                    I::If(BlockType::Empty),
                    I::I32Const(bytes as i32),
                    I::Call(ALLOC),
                    I::LocalSet(2),
                    I::End,
                    I::LocalGet(result),
                ] {
                    function.linked_instruction(&instruction, count);
                }
            }
            // Publish demand state before copying: a Ready demand owns its
            // persistent value and never retains the caller's output buffer.
            for instruction in [
                I::LocalSet(result),
                I::LocalGet(result),
                I::I32Eqz,
                I::LocalGet(2),
                I::I32Eqz,
                I::I32Or,
                I::If(BlockType::Result(ValType::I32)),
                I::LocalGet(result),
                I::Else,
                I::LocalGet(2),
                I::LocalGet(result),
                I::I32Const(bytes as i32),
                I::Call(HEAP_COPY),
                I::End,
            ] {
                function.linked_instruction(&instruction, count);
            }
        }
        if frame.is_some() {
            for instruction in [
                I::LocalSet(result),
                I::Call(FRAME_LEAVE),
                I::Drop,
                I::LocalGet(result),
            ] {
                function.linked_instruction(&instruction, count);
            }
        }
        function.linked_instruction(&I::Call(ROOTS_LEAVE), count);
        function.instruction(&I::Drop);
        function.instruction(&I::End);
        function
    }
    pub fn ty(&self, node: HirId) -> Result<TypeId, String> {
        self.key.ty(self.mir, node)
    }
    pub fn scratch_alloc(&mut self, bytes: u32, clear: bool) -> u32 {
        let Some(base) = self.frame_base.filter(|_| self.parameter_count == 3) else {
            return self.alloc(bytes);
        };
        let offset = self.scratch_bytes;
        self.scratch_bytes = offset
            .checked_add(bytes.max(1).checked_add(7).expect("scratch extent") & !7)
            .expect("scratch function extent");
        let result = self.local(ValType::I32);
        self.extend([I::LocalGet(base), I::I32Const(offset as i32), I::I32Add]);
        if clear {
            self.extend([I::I32Const(bytes as i32), I::Call(FRAME_SLOT)]);
        }
        self.emit(I::LocalSet(result));
        result
    }
    pub fn output_alloc(&mut self, ty: TypeId) -> Result<u32, String> {
        let bytes = self.width(ty)?;
        Ok(if self.plan.layouts[ty.index()].plain {
            self.scratch_alloc(bytes, false)
        } else {
            self.alloc(bytes)
        })
    }
    pub fn escape_plain(&mut self, value: u32, ty: TypeId) -> Result<u32, String> {
        if !self.plan.layouts[ty.index()].plain {
            return Ok(value);
        }
        let bytes = self.width(ty)?;
        let owned = self.alloc(bytes);
        self.copy(owned, 0, value, bytes);
        Ok(owned)
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
    pub fn store64(&mut self, pointer: u32, offset: u64, value: u64) {
        self.extend([
            I::LocalGet(pointer),
            I::I64Const(value as i64),
            I::I64Store(memory(offset, 3)),
        ]);
    }
    pub fn store_location(&mut self, pointer: u32, loc: telora_core::Loc) {
        let loc = telora_wasm_shared::source_range::SourceRange::checked(
            loc.source.get(),
            loc.start,
            loc.end,
            telora_wasm_shared::source_range::OFFSET_LIMIT - 1,
        )
        .expect("source location exceeds packed range");
        self.store64(pointer, SOURCE, loc.packed());
    }
    pub fn value(&mut self, node: HirId, bytes: u32) -> Result<u32, String> {
        let ty = self.effective_ty(node)?;
        self.value_as(node, ty, bytes)
    }
    pub fn value_as(&mut self, node: HirId, ty: TypeId, bytes: u32) -> Result<u32, String> {
        if self.width(ty)? != bytes {
            return Err(format!("Wasm: value width mismatch at {node:?}"));
        }
        let result = if self.plan.layouts[ty.index()].plain {
            self.scratch_alloc(bytes, true)
        } else {
            self.alloc(bytes)
        };
        self.produced_origin(result, node)?;
        Ok(result)
    }
    pub fn scalar(&mut self, node: HirId, bits: i64) -> Result<u32, String> {
        self.scalar_as(node, self.effective_ty(node)?, bits)
    }
    pub fn scalar_as(&mut self, node: HirId, ty: TypeId, bits: i64) -> Result<u32, String> {
        let result = self.value_as(node, ty, SCALAR_BYTES)?;
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
        let pointer = self.alloc(DIAGNOSTIC_BYTES);
        self.store_location(pointer, location);
        self.store32(pointer, DIAG_CODE, code);
        self.store32(
            pointer,
            DIAG_MESSAGE_TYPE,
            self.string_type().expect("sealed String type").index() as u32,
        );
        self.extend([
            I::LocalGet(pointer),
            I::GlobalGet(INITIALIZATION_ROOT_GLOBAL),
            I::I32Store(memory(DIAG_ROOT, 2)),
        ]);
        self.table_push(DIAGNOSTICS, pointer, DIAGNOSTIC_BYTES, None)
            .expect("diagnostic table push");
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
        // Prefix and else-if chains have no delimiter nesting limit. Keep
        // continuations on the heap, including each node's value adjustment.
        enum Pending {
            Release(HirId, Option<crate::stack_values::ExpressionBindings>),
            Unary(HirId, telora_core::syntax::kinds::UnaryOperator),
            Binary(HirId, telora_core::syntax::kinds::BinaryOperator),
            If(HirId, u32),
            IfLet(HirId, u32),
            LetElse(HirId, u32),
            Debug(HirId),
            RaiseMessage(HirId, telora_core::syntax::kinds::BlameAction),
            RaiseSubject(
                HirId,
                telora_core::syntax::kinds::BlameAction,
                u32,
                Vec<HirId>,
                Vec<u32>,
            ),
            MatchInput(HirId),
            MatchArm(HirId, u32, u32, Vec<HirId>, usize),
            Adjust(HirId),
            Ascription(HirId),
            StructUpdate(HirId),
            FieldProjection(HirId),
            Field(HirId),
            Projection(HirId, usize),
            Index(HirId),
            Propagate(HirId),
            CallCallee(HirId, Vec<HirId>),
            CallArgument(HirId, u32, Vec<HirId>, Vec<u32>),
        }
        let mut pending = Vec::new();
        let mut current = node;
        'evaluate: loop {
            let mut prepared = None;
            loop {
                let saved = self.enter_expression(current);
                pending.push(Pending::Release(current, saved));
                match self.mir.hir[current.index()].kind {
                    HirKind::Raise(_) | HirKind::Panic => {
                        let action = match self.mir.hir[current.index()].kind {
                            HirKind::Raise(action) => action,
                            _ => telora_core::syntax::kinds::BlameAction::Fail,
                        };
                        pending.push(Pending::RaiseMessage(current, action));
                        current = child(self.mir, current, Role::Value)?;
                    }
                    HirKind::Debug { .. } => {
                        pending.push(Pending::Debug(current));
                        current = child(self.mir, current, Role::Value)?;
                    }
                    HirKind::Match => {
                        pending.push(Pending::MatchInput(current));
                        current = child(self.mir, current, Role::Value)?;
                    }
                    HirKind::LetElse => {
                        let result = self.let_start(current)?;
                        pending.push(Pending::LetElse(current, result));
                        current = child(self.mir, current, Role::Body)?;
                    }
                    HirKind::Index => {
                        pending.push(Pending::Index(current));
                        current = child(self.mir, current, Role::Receiver)?;
                    }
                    HirKind::Propagate => {
                        pending.push(Pending::Propagate(current));
                        current = child(self.mir, current, Role::Operand)?;
                    }
                    HirKind::Field => {
                        if crate::enums::selection(self.mir, current).is_some() {
                            break;
                        }
                        if let Some(value) = self.field_start(current)? {
                            prepared = Some(value);
                            break;
                        }
                        pending.push(Pending::Field(current));
                        current = child(self.mir, current, Role::Receiver)?;
                    }
                    HirKind::TupleProjection(index) => {
                        pending.push(Pending::Projection(current, index));
                        current = child(self.mir, current, Role::Receiver)?;
                    }
                    HirKind::FieldProjection => {
                        let receiver = child(self.mir, current, Role::Receiver)?;
                        if matches!(
                            self.mir.types[self.effective_ty(receiver)?.index()].constructor,
                            TypeConstructor::Record(_)
                        ) {
                            break;
                        }
                        pending.push(Pending::FieldProjection(current));
                        current = receiver;
                    }
                    HirKind::Binary(telora_core::syntax::kinds::BinaryOperator::StructUpdate) => {
                        pending.push(Pending::StructUpdate(current));
                        current = child(self.mir, current, Role::Left)?;
                    }
                    HirKind::Call => {
                        if let Some(value) = self.format_render_call(current)? {
                            prepared = Some(value);
                            break;
                        }
                        if let Some(value) = self.literal_pairs_call(current)? {
                            prepared = Some(value);
                            break;
                        }
                        let arguments = self.call_arguments(current)?;
                        pending.push(Pending::CallCallee(current, arguments));
                        current = child(self.mir, current, Role::Callee)?;
                    }
                    HirKind::TypeAscription => {
                        pending.push(Pending::Ascription(current));
                        current = child(self.mir, current, Role::Value)?;
                    }
                    HirKind::IfLet => {
                        let result = self.if_let_start(current)?;
                        pending.push(Pending::IfLet(current, result));
                        current = child(self.mir, current, Role::Else)?;
                    }
                    HirKind::Binary(op)
                        if op != telora_core::syntax::kinds::BinaryOperator::StructUpdate =>
                    {
                        pending.push(Pending::Binary(current, op));
                        current = child(self.mir, current, Role::Left)?;
                    }
                    HirKind::Block => {
                        self.block_bindings(current)?;
                        pending.push(Pending::Adjust(current));
                        current = child(self.mir, current, Role::Result)?;
                    }
                    HirKind::Unary(op) => {
                        pending.push(Pending::Unary(current, op));
                        current = child(self.mir, current, Role::Operand)?;
                    }
                    HirKind::If => {
                        let condition_node = child(self.mir, current, Role::Condition)?;
                        if self.mir.types[self.ty(condition_node)?.index()].constructor
                            != TypeConstructor::Bool
                        {
                            return Err("Wasm: condition is not sealed Bool".into());
                        }
                        let condition = self.expression(condition_node)?;
                        let result = self.local(ValType::I32);
                        self.bits(condition);
                        self.extend([I::I64Eqz, I::If(BlockType::Empty)]);
                        pending.push(Pending::If(current, result));
                        current = child(self.mir, current, Role::Else)?;
                    }
                    _ => break,
                }
            }
            let mut value = match prepared {
                Some(value) => self.adjust_value(current, value)?,
                None => self.adjusted_expression(current)?,
            };
            while let Some(frame) = pending.pop() {
                let node = match frame {
                    Pending::Release(node, saved) => {
                        self.root_value(value, self.effective_ty(node)?);
                        self.leave_expression(value, saved);
                        continue;
                    }
                    Pending::RaiseMessage(node, action) => {
                        let subjects = self.mir.hir[node.index()]
                            .children
                            .iter()
                            .filter(|edge| edge.role == Role::Subject)
                            .map(|edge| edge.node)
                            .collect::<Vec<_>>();
                        if let Some(&subject) = subjects.first() {
                            pending.push(Pending::RaiseSubject(
                                node,
                                action,
                                value,
                                subjects,
                                Vec::new(),
                            ));
                            current = subject;
                            continue 'evaluate;
                        }
                        value = self.raise_values(node, action, value, &[])?;
                        node
                    }
                    Pending::RaiseSubject(node, action, message, subjects, mut values) => {
                        values.push(value);
                        if let Some(&subject) = subjects.get(values.len()) {
                            pending.push(Pending::RaiseSubject(
                                node, action, message, subjects, values,
                            ));
                            current = subject;
                            continue 'evaluate;
                        }
                        value = self.raise_values(node, action, message, &values)?;
                        node
                    }
                    Pending::MatchInput(node) => {
                        let arms = self.mir.hir[node.index()]
                            .children
                            .iter()
                            .filter(|edge| edge.role == Role::Arm)
                            .map(|edge| edge.node)
                            .collect::<Vec<_>>();
                        let result = self.match_start();
                        if let Some(&arm) = arms.first() {
                            current = self.match_arm_start(arm, value)?;
                            pending.push(Pending::MatchArm(node, result, value, arms, 0));
                            continue 'evaluate;
                        }
                        self.match_finish(node);
                        value = result;
                        node
                    }
                    Pending::MatchArm(node, result, input, arms, index) => {
                        self.match_arm_finish(node, arms[index], result, value)?;
                        if let Some(&arm) = arms.get(index + 1) {
                            current = self.match_arm_start(arm, input)?;
                            pending.push(Pending::MatchArm(node, result, input, arms, index + 1));
                            continue 'evaluate;
                        }
                        self.match_finish(node);
                        value = result;
                        node
                    }
                    Pending::CallCallee(node, arguments) => {
                        if let Some(&argument) = arguments.first() {
                            pending.push(Pending::CallArgument(node, value, arguments, Vec::new()));
                            current = argument;
                            continue 'evaluate;
                        }
                        value = self.call_values(node, value, &[])?;
                        node
                    }
                    Pending::CallArgument(node, callee, arguments, mut values) => {
                        let index = values.len();
                        values.push(self.call_argument(node, index, arguments[index], value)?);
                        if let Some(&argument) = arguments.get(values.len()) {
                            pending.push(Pending::CallArgument(node, callee, arguments, values));
                            current = argument;
                            continue 'evaluate;
                        }
                        value = self.call_values(node, callee, &values)?;
                        node
                    }
                    Pending::Adjust(node) => node,
                    Pending::Debug(node) => {
                        value = self.debug_value(node, value)?;
                        node
                    }
                    Pending::LetElse(node, result) => {
                        let body = child(self.mir, node, Role::Body)?;
                        self.let_success(node, body, result, value)?;
                        let no = self.expression(child(self.mir, node, Role::Else)?)?;
                        value = self.if_let_finish(node, result, no)?;
                        node
                    }
                    Pending::Index(node) => {
                        value = self.index_receiver(node, value)?;
                        node
                    }
                    Pending::Propagate(node) => {
                        value = self.propagate_value(node, value)?;
                        node
                    }
                    Pending::Field(node) => {
                        value = self.field_value(node, value)?;
                        node
                    }
                    Pending::Projection(node, index) => {
                        value = self.projection_value(node, index, value)?;
                        node
                    }
                    Pending::FieldProjection(node) => {
                        value = self.field_projection_value(node, value)?;
                        node
                    }
                    Pending::StructUpdate(node) => {
                        value = self.struct_update_left(node, value)?;
                        node
                    }
                    Pending::Ascription(node) => {
                        let inner = child(self.mir, node, Role::Value)?;
                        value = self.adapt(
                            node,
                            self.effective_ty(inner)?,
                            self.effective_ty(node)?,
                            value,
                        )?;
                        node
                    }
                    Pending::IfLet(node, result) => {
                        value = self.if_let_finish(node, result, value)?;
                        node
                    }
                    Pending::Binary(node, op) => {
                        value = self.binary_left(node, op, value)?;
                        node
                    }
                    Pending::Unary(node, op) => {
                        value = self.unary_value(node, op, value)?;
                        node
                    }
                    Pending::If(node, result) => {
                        self.extend([I::LocalGet(value), I::LocalSet(result), I::Else]);
                        let yes = self.expression(child(self.mir, node, Role::Then)?)?;
                        self.extend([I::LocalGet(yes), I::LocalSet(result), I::End]);
                        value = result;
                        node
                    }
                };
                value = self.adjust_value(node, value)?;
            }
            return Ok(value);
        }
    }
    fn adjusted_expression(&mut self, node: HirId) -> Result<u32, String> {
        let value = self.raw_expression(node)?;
        self.adjust_value(node, value)
    }
    fn block_bindings(&mut self, node: HirId) -> Result<(), String> {
        self.reserve_local_instances(node)?;
        for edge in &self.mir.hir[node.index()].children {
            if edge.role == Role::Binding
                && matches!(
                    self.mir.hir[edge.node.index()].kind,
                    HirKind::Binding {
                        kind: telora_core::syntax::kinds::BindingKind::Def,
                        ..
                    }
                )
                && self.mir.hir_symbols[edge.node.index()]
                    .is_some_and(|symbol| self.mir.symbol_generics[symbol.index()].is_empty())
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
                if matches!(
                    self.mir.hir[edge.node.index()].kind,
                    HirKind::Binding {
                        kind: telora_core::syntax::kinds::BindingKind::Type
                            | telora_core::syntax::kinds::BindingKind::Trait,
                        ..
                    }
                ) {
                    continue;
                }
                if self.emit_local_template(edge.node)? {
                    continue;
                }
                self.expression(edge.node)?;
            }
        }
        Ok(())
    }
    fn adjust_value(&mut self, node: HirId, value: u32) -> Result<u32, String> {
        if self.mir.value_adjustments[node.index()].is_some() {
            let target = self.effective_ty(node)?;
            self.construction_check(node, target, telora_core::mir::PropertySite::Type, value)?;
            let result = self.value_as(node, target, self.width(target)?)?;
            self.copy(result, 0, value, self.width(target)?);
            return Ok(result);
        }
        Ok(value)
    }
    fn raw_expression(&mut self, node: HirId) -> Result<u32, String> {
        if let Some(telora_core::mir::MemberSelection::Boolean(value)) =
            crate::enums::selection(self.mir, node)
        {
            return self.scalar(node, i64::from(value));
        }
        if let Some(
            selection @ (telora_core::mir::MemberSelection::EnumVariant { .. }
            | telora_core::mir::MemberSelection::NewtypeConstructor),
        ) = crate::enums::selection(self.mir, node)
        {
            if self.mir.types[self.ty(node)?.index()].constructor == TypeConstructor::Function {
                let key = Key {
                    node,
                    instance: self.key.instance,
                    callable: true,
                    special: Special::Normal,
                };
                return self.function_value(node, key, self.ty(node)?, &[]);
            }
            if let telora_core::mir::MemberSelection::EnumVariant { index } = selection {
                return self.enum_value(node, self.effective_ty(node)?, index, None);
            }
        }
        match &self.mir.hir[node.index()].kind {
            HirKind::Raise(_) | HirKind::Panic => {
                unreachable!("diagnostics are emitted by expression")
            }
            HirKind::TypeMetadata => {
                let ty = self.ty(node)?;
                let shape = &self.mir.types[ty.index()];
                if shape.constructor != TypeConstructor::TypeOf || shape.arguments.len() != 1 {
                    return Err("Wasm: metadata witness is not sealed".into());
                }
                self.scalar(node, shape.arguments[0].index() as i64)
            }
            HirKind::Binding {
                kind: telora_core::syntax::kinds::BindingKind::Native,
                ..
            } => {
                let key = Key {
                    node,
                    instance: self.key.instance,
                    callable: true,
                    special: Special::Normal,
                };
                self.function_value(node, key, self.ty(node)?, &[])
            }
            HirKind::Int(value) => self.scalar(node, *value),
            HirKind::Float(value) => self.scalar(node, value.to_bits() as i64),
            HirKind::String(value) => self.text(node, value.as_bytes()),
            HirKind::InterpolatedString => self.interpolate(node),
            HirKind::Bytes(value) => self.bytes_literal(node, value),
            HirKind::Array => self.array_expression(node),
            HirKind::Dict => {
                if self.mir.types[self.effective_ty(node)?.index()].constructor
                    == TypeConstructor::Dict
                {
                    self.dictionary(node)
                } else {
                    self.record(node)
                }
            }
            HirKind::TupleProjection(_) => unreachable!("projections are emitted by expression"),
            HirKind::FieldProjection => self.field_projection(node),
            HirKind::Field => unreachable!("fields are emitted by expression"),
            HirKind::Index => unreachable!("index nodes are emitted by expression"),
            HirKind::Match => unreachable!("match is emitted by expression"),
            HirKind::LetElse => unreachable!("let-else is emitted by expression"),
            HirKind::IfLet => unreachable!("if-let nodes are emitted by expression"),
            HirKind::Propagate => unreachable!("propagation is emitted by expression"),
            HirKind::Tuple => self.tuple_expression(node),
            HirKind::Binding { kind, .. } => {
                if *kind == telora_core::syntax::kinds::BindingKind::Decl
                    && self.mir.modules[self.mir.hir[node.index()].module.index()].kind
                        == telora_core::mir::ModuleKind::Data
                {
                    self.failure(node, ERROR_DATA);
                    return Ok(self.local(ValType::I32));
                }
                if *kind == telora_core::syntax::kinds::BindingKind::Decl {
                    let symbol = self.mir.hir_symbols[node.index()]
                        .ok_or("Wasm: local declaration has no stable symbol")?;
                    return self.bindings.get(&symbol).copied().ok_or_else(|| {
                        format!("Wasm: local declaration {symbol:?} has no reserved function")
                    });
                }
                if !matches!(
                    kind,
                    telora_core::syntax::kinds::BindingKind::Let
                        | telora_core::syntax::kinds::BindingKind::Def
                        | telora_core::syntax::kinds::BindingKind::Impl
                ) {
                    return Err(format!("Wasm: unsupported binding {kind:?}"));
                }
                let result = self.expression(child(self.mir, node, Role::Value)?)?;
                if self.mir.types[self.ty(node)?.index()].constructor == TypeConstructor::Function {
                    self.extend([I::LocalGet(result), I::I32Load(memory(DATA, 2)), I::I32Eqz]);
                    self.fail_if(node, ERROR_UNINITIALIZED_FUNCTION);
                }
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
                    if let Some(&value) = self.local_instances.get(&instance) {
                        return Ok(value);
                    }
                    if self.plan.local_instances.contains(&instance) {
                        return Err(format!(
                            "Wasm: local instance {instance:?} is unavailable in {:?}",
                            self.key
                        ));
                    }
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
                        .ok_or_else(|| format!(
                            "Wasm: lexical capture {} ({symbol:?}) is not available at {node:?} in {:?}",
                            self.mir.symbols[symbol.index()].name, self.key
                        ))?,
                )
            }
            HirKind::TypeAscription => unreachable!("ascriptions are emitted by expression"),
            HirKind::Block => unreachable!("block nodes are emitted by expression"),
            HirKind::Binary(_) => unreachable!("binary nodes are emitted by expression"),
            HirKind::Unary(_) => unreachable!("prefix nodes are emitted by expression"),
            HirKind::If => unreachable!("if nodes are emitted by expression"),
            HirKind::Return => {
                let value = self.expression(child(self.mir, node, Role::Value)?)?;
                self.extend([I::LocalGet(value), I::Return]);
                Ok(value)
            }
            HirKind::Debug { .. } => unreachable!("debug is emitted by expression"),
            HirKind::Closure | HirKind::Interpreter => self.closure(node),
            HirKind::Call => unreachable!("calls are emitted by expression"),
            other => Err(format!(
                "Wasm: unsupported expression {other:?} at {:?}",
                self.mir.hir[node.index()].location
            )),
        }
    }
}

pub(crate) fn compile(
    mir: &Mir,
    plan: &Plan,
    key: Key,
) -> Result<crate::object::ObjectFunction, String> {
    let mut emit = Emitter::new(mir, plan, key);
    if let Special::Equal(ty) = key.special {
        emit.compare_type(ty)?;
    } else if let Special::Parse(ty) = key.special {
        let value = emit.parse_type(ty)?;
        emit.emit(I::LocalGet(value));
    } else if let Special::Json(ty) = key.special {
        emit.json_type(ty)?;
    } else if let Special::Encode(source, target) = key.special {
        let value = emit.codec_encode_type(source, target, 1)?;
        emit.emit(I::LocalGet(value));
    } else if let Special::Decode(source, target) = key.special {
        let value = emit.codec_decode_type(source, target, 1)?;
        emit.emit(I::LocalGet(value));
    } else if let Special::DecodeVariant(source, target, index) = key.special {
        let value = emit.codec_decode_variant(source, target, index)?;
        emit.emit(I::LocalGet(value));
    } else if key.callable && crate::natives::identity(mir, key.node).is_some() {
        let value = emit.native()?;
        emit.emit(I::LocalGet(value));
    } else if key.callable && matches!(mir.hir[key.node.index()].kind, HirKind::Interpreter) {
        let value = emit.interpreter()?;
        emit.emit(I::LocalGet(value));
    } else if key.callable && !matches!(mir.hir[key.node.index()].kind, HirKind::Closure) {
        let value = emit.constructor()?;
        emit.emit(I::LocalGet(value));
    } else if key.callable {
        emit.plan_tail_calls()?;
        for (index, symbol) in plan.captures[&key].iter().enumerate() {
            let local = emit.local(ValType::I32);
            emit.extend([
                I::LocalGet(0),
                I::I32Load(memory(8 + index as u64 * 4, 2)),
                I::LocalSet(local),
            ]);
            emit.bindings.insert(*symbol, local);
        }
        for (index, instance) in plan.instance_captures[&key].iter().enumerate() {
            let local = emit.local(ValType::I32);
            emit.extend([
                I::LocalGet(0),
                I::I32Load(memory(
                    8 + (index + plan.captures[&key].len()) as u64 * 4,
                    2,
                )),
                I::LocalSet(local),
            ]);
            emit.local_instances.insert(*instance, local);
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
        // 0 = empty, 1 = evaluating, 2 = ready, 3 = failed.
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
            I::I32Const(3),
            I::I32Eq,
            I::If(BlockType::Empty),
            I::I32Const(offset as i32),
            I::I32Load(memory(4, 2)),
            I::GlobalSet(ERROR_GLOBAL),
            I::I32Const(3),
            I::GlobalSet(PHASE_GLOBAL),
            I::I32Const(0),
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
        let value = match key.special {
            Special::Property(index) => emit.property_chain(index)?,
            _ => emit.expression(key.node)?,
        };
        emit.emit(I::LocalGet(value));
    }
    Ok(emit.finish())
}
