use super::*;
use cranelift_module::FuncId;

impl Lower<'_, '_> {
    pub(super) fn find_tail_calls(&mut self, body: HirId) -> EmitResult<()> {
        if self.function_key.initializer || !matches!(self.mir.hir[self.function_key.node.index()].kind, HirKind::Closure) { return Ok(()); }
        let boundary = child(self.mir, self.function_key.node, Role::ReturnType)?;
        if self.adjustment_type(boundary)?.is_some() { return Ok(()); }
        let mut pending = vec![body];
        let mut scan = vec![body];
        while let Some(node) = scan.pop() {
            if matches!(self.mir.hir[node.index()].kind, HirKind::Closure | HirKind::Interpreter) { continue; }
            if matches!(self.mir.hir[node.index()].kind, HirKind::Return) {
                pending.push(child(self.mir, node, Role::Value)?);
            }
            scan.extend(self.mir.hir[node.index()].children.iter().map(|edge| edge.node));
        }
        while let Some(node) = pending.pop() {
            if self.adjustment_type(node)?.is_some() { continue; }
            let syntax = &self.mir.hir[node.index()];
            let roles: &[Role] = match syntax.kind {
                HirKind::Call if TypeKey::try_from(self.ty(node)?)? == self.return_type => {
                    self.tail_calls.insert(node);
                    continue;
                }
                HirKind::Block => &[Role::Result],
                HirKind::If | HirKind::IfLet => &[Role::Then, Role::Else],
                HirKind::LetElse => &[Role::Body, Role::Else],
                HirKind::Match => &[Role::Arm],
                HirKind::MatchArm { .. } | HirKind::TypeAscription => &[Role::Value],
                _ => &[],
            };
            pending.extend(syntax.children.iter().filter(|edge| roles.contains(&edge.role)).map(|edge| edge.node));
        }
        Ok(())
    }

    pub(super) fn transfer_tail(&mut self, node: HirId, callee_type: TypeKey, environment: ir::Value, arguments: &[ir::Value]) -> EmitResult<Vec<ir::Value>> {
        let dispatcher = self.functions.tail_dispatcher(callee_type, self.module)?;
        let dispatcher = self.module.declare_func_in_func(dispatcher, self.builder.func);
        let address = self.builder.ins().func_addr(self.module.target_config().pointer_type(), dispatcher);
        let mut packet = vec![address];
        for index in 0..4 { packet.push(self.builder.ins().load(types::I64, MemFlagsData::new(), environment, index * 8)); }
        packet.extend_from_slice(arguments);
        let data = self.stack_words(&packet)?;
        let operation = self.builder.ins().iconst(types::I32, helpers::TAIL_PREPARE as i64);
        let zero = self.builder.ins().iconst(types::I32, 0);
        let origin = Origin::from_loc(Some(self.mir.hir[node.index()].location)).words();
        let loc = self.builder.ins().iconst(types::I64, (u64::from(origin[0]) | (u64::from(origin[1]) << 32)) as i64);
        let end = self.builder.ins().iconst(types::I32, origin[2] as i64);
        let count = self.builder.ins().iconst(types::I64, packet.len() as i64);
        let null = self.builder.ins().iconst(types::I64, 0);
        let call = self.builder.ins().call(self.object_helper, &[self.context, operation, zero, loc, end, data, count, null]);
        let status = self.builder.inst_results(call)[0];
        let success = self.builder.ins().icmp_imm_s(cranelift_codegen::ir::condcodes::IntCC::Equal, status, 0);
        let transfer = self.builder.ins().iconst(types::I32, 2);
        let status = self.builder.ins().select(success, transfer, status);
        self.return_status(status);
        Err(EmitError::Diverged)
    }

    pub(super) fn save_environment(&mut self, environment: ir::Value) -> EmitResult<ir::Value> {
        let absent = self.builder.create_block();
        let present = self.builder.create_block();
        let merge = self.builder.create_block();
        self.builder.append_block_param(merge, types::I64);
        self.builder.ins().brif(environment, present, &[], absent, &[]);
        self.builder.switch_to_block(absent);
        self.builder.seal_block(absent);
        self.builder.ins().jump(merge, &[environment.into()]);
        self.builder.switch_to_block(present);
        self.builder.seal_block(present);
        let words = (0..4).map(|index| self.builder.ins().load(types::I64, MemFlagsData::new(), environment, index * 8)).collect::<Vec<_>>();
        let saved = self.stack_words(&words)?;
        self.builder.ins().jump(merge, &[saved.into()]);
        self.builder.switch_to_block(merge);
        self.builder.seal_block(merge);
        Ok(self.builder.block_params(merge)[0])
    }
}

/// Public entries keep the C ABI and return only Success/Failed. The private
/// body can request another body; its frame has already exited at that point.
pub(super) fn emit_wrapper(module: &mut JITModule, wrapper: FuncId, body: FuncId, helper: FuncId, signature: &ir::Signature) -> Result<()> {
    let mut context = module.make_context();
    context.func.signature = signature.clone();
    let helper = module.declare_func_in_func(helper, &mut context.func);
    let body = module.declare_func_in_func(body, &mut context.func);
    let indirect = context.func.import_signature(signature.clone());
    let mut frontend = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut context.func, &mut frontend);
    let entry = builder.create_block();
    let invoke = builder.create_block();
    let transfer = builder.create_block();
    let done = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    for _ in 0..3 { builder.append_block_param(invoke, types::I64); }
    builder.append_block_param(done, types::I32);
    builder.switch_to_block(entry);
    builder.seal_block(entry);
    let inputs = builder.block_params(entry).to_vec();
    let address = builder.ins().func_addr(types::I64, body);
    builder.ins().jump(invoke, &[address.into(), inputs[1].into(), inputs[3].into()]);
    builder.switch_to_block(invoke);
    let next = builder.block_params(invoke).to_vec();
    let call = builder.ins().call_indirect(indirect, next[0], &[inputs[0], next[1], inputs[2], next[2]]);
    let status = builder.inst_results(call)[0];
    let tail = builder.ins().icmp_imm_s(cranelift_codegen::ir::condcodes::IntCC::Equal, status, 2);
    builder.ins().brif(tail, transfer, &[], done, &[status.into()]);
    builder.switch_to_block(transfer);
    builder.seal_block(transfer);
    let slot = builder.create_sized_stack_slot(ir::StackSlotData::new(ir::StackSlotKind::ExplicitSlot, 24, 3));
    let output = builder.ins().stack_addr(types::I64, slot, 0);
    let operation = builder.ins().iconst(types::I32, helpers::TAIL_TAKE as i64);
    let zero32 = builder.ins().iconst(types::I32, 0);
    let zero64 = builder.ins().iconst(types::I64, 0);
    let call = builder.ins().call(helper, &[inputs[0], operation, zero32, zero64, zero32, zero64, zero64, output]);
    let status = builder.inst_results(call)[0];
    let ready = builder.create_block();
    builder.ins().brif(status, done, &[status.into()], ready, &[]);
    builder.switch_to_block(ready);
    builder.seal_block(ready);
    let packet = (0..3).map(|index| builder.ins().stack_load(types::I64, types::I64, slot, index * 8).into()).collect::<Vec<ir::BlockArg>>();
    builder.ins().jump(invoke, &packet);
    builder.seal_block(invoke);
    builder.switch_to_block(done);
    builder.seal_block(done);
    let status = builder.block_params(done)[0];
    builder.ins().return_(&[status]);
    builder.finalize(module.target_config());
    module.define_function(wrapper, &mut context).map_err(|error| error.to_string())
}
