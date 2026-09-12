use super::*;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use telora_core::ast::{BinaryOperator as B, UnaryOperator as U};

impl Lower<'_, '_> {
    fn call_guard_helper(&mut self, node: HirId, operation: u32) -> ir::Value {
        let admission = operation == helpers::ENTER_CALL;
        let operation = self.builder.ins().iconst(types::I32, operation as i64);
        let ty = self.builder.ins().iconst(types::I32, 0);
        let origin = Origin::from_loc(Some(self.mir.hir[node.index()].location)).words();
        let loc0 = self.builder.ins().iconst(types::I64, (u64::from(origin[0]) | (u64::from(origin[1]) << 32)) as i64);
        let end = self.builder.ins().iconst(types::I32, origin[2] as i64);
        let null = self.builder.ins().iconst(self.module.target_config().pointer_type(), 0);
        let count = self.builder.ins().iconst(types::I64, 0);
        if admission { self.frame_charge = Some(self.builder.func.dfg.value_def(count).unwrap_inst()); }
        let call = self.builder.ins().call(self.object_helper, &[self.context, operation, ty, loc0, end, null, count, null]);
        self.builder.inst_results(call)[0]
    }
    pub(super) fn enter_call(&mut self, node: HirId) {
        let status = self.call_guard_helper(node, helpers::ENTER_CALL);
        let fail = self.builder.create_block();
        let ready = self.builder.create_block();
        self.builder.ins().brif(status, fail, &[], ready, &[]);
        self.builder.switch_to_block(fail);
        self.builder.seal_block(fail);
        // Failed admission did not increment the depth.
        self.builder.ins().return_(&[status]);
        self.builder.switch_to_block(ready);
        self.builder.seal_block(ready);
        self.guarded = true;
    }
    pub(super) fn seal_frame_charge(&mut self) {
        if let Some(inst) = self.frame_charge {
            let words = self.builder.func.sized_stack_slots.values().map(|slot| u64::from(slot.size).div_ceil(8)).sum::<u64>();
            let ir::InstructionData::UnaryImm { imm, .. } = &mut self.builder.func.dfg.insts[inst] else { unreachable!("frame charge constant") };
            *imm = (words as i64).into();
        }
    }
    pub(super) fn return_status(&mut self, status: ir::Value) {
        if self.guarded { self.call_guard_helper(self.function_key.node, helpers::LEAVE_CALL); }
        self.builder.ins().return_(&[status]);
    }
    pub(super) fn charge_fuel(&mut self, node: HirId) {
        let operation = self.builder.ins().iconst(types::I32, helpers::FUEL as i64);
        let ty = self.builder.ins().iconst(types::I32, 0);
        let origin = Origin::from_loc(Some(self.mir.hir[node.index()].location)).words();
        let loc0 = self.builder.ins().iconst(types::I64, (u64::from(origin[0]) | (u64::from(origin[1]) << 32)) as i64);
        let end = self.builder.ins().iconst(types::I32, origin[2] as i64);
        let null = self.builder.ins().iconst(self.module.target_config().pointer_type(), 0);
        let count = self.builder.ins().iconst(types::I64, 1);
        let call = self.builder.ins().call(self.object_helper, &[self.context, operation, ty, loc0, end, null, count, null]);
        let status = self.builder.inst_results(call)[0];
        let failed = self.builder.create_block();
        let next = self.builder.create_block();
        self.builder.ins().brif(status, failed, &[], next, &[]);
        self.builder.switch_to_block(failed);
        self.builder.seal_block(failed);
        self.return_status(status);
        self.builder.switch_to_block(next);
        self.builder.seal_block(next);
    }
    fn scalar_result(&mut self, node: HirId, bits: ir::Value) -> EmitResult<Vec<ir::Value>> {
        let ty = TypeKey::try_from(self.ty(node)?)?;
        let header = self.layouts.value(
            ty,
            Origin::from_loc(Some(self.mir.hir[node.index()].location)),
            &[0],
        )?;
        let mut values = header
            .words()
            .iter()
            .map(|&w| self.builder.ins().iconst(types::I64, w as i64))
            .collect::<Vec<_>>();
        values[2] = bits;
        Ok(values)
    }
    fn fail_if(&mut self, node: HirId, condition: ir::Value, message: &str) -> EmitResult<()> {
        let failed = self.builder.create_block();
        let next = self.builder.create_block();
        self.builder.ins().brif(condition, failed, &[], next, &[]);
        self.builder.switch_to_block(failed);
        self.builder.seal_block(failed);
        let (data, count) = self.literal_bytes(message)?;
        let operation = self.builder.ins().iconst(types::I32, helpers::FAIL as i64);
        let ty = self.builder.ins().iconst(types::I32, 0);
        let origin = Origin::from_loc(Some(self.mir.hir[node.index()].location)).words();
        let loc0 = self.builder.ins().iconst(
            types::I64,
            (u64::from(origin[0]) | (u64::from(origin[1]) << 32)) as i64,
        );
        let end = self.builder.ins().iconst(types::I32, origin[2] as i64);
        let zero = self.builder.ins().iconst(types::I64, 0);
        let out = self.stack_words(&[zero])?;
        let call = self.builder.ins().call(
            self.object_helper,
            &[self.context, operation, ty, loc0, end, data, count, out],
        );
        let status = self.builder.inst_results(call)[0];
        self.return_status(status);
        self.builder.switch_to_block(next);
        self.builder.seal_block(next);
        Ok(())
    }
    pub(super) fn unary(&mut self, node: HirId, op: U, depth: usize) -> EmitResult<Vec<ir::Value>> {
        let operand = child(self.mir, node, Role::Operand)?;
        let ty = self.ty(operand)?;
        let value = self.expression(operand, depth + 1)?[2];
        let result = match (&self.mir.types[ty.index()].constructor, op) {
            (TypeConstructor::Int, U::Negate) => {
                let min = self.builder.ins().icmp_imm_s(IntCC::Equal, value, i64::MIN);
                self.fail_if(node, min, "integer arithmetic overflowed")?;
                self.builder.ins().ineg(value)
            }
            (TypeConstructor::Float, U::Negate) => self.builder.ins().bxor_imm_s(value, i64::MIN),
            (TypeConstructor::Int, U::BitNot | U::Not) => self.builder.ins().bnot(value),
            (TypeConstructor::Bool, U::LogicalNot | U::Not) => {
                let b = self.builder.ins().icmp_imm_s(IntCC::Equal, value, 0);
                self.builder.ins().uextend(types::I64, b)
            }
            _ => return Err("native unsupported unary operation/type".into()),
        };
        self.scalar_result(node, result)
    }
    pub(super) fn binary(
        &mut self,
        node: HirId,
        op: B,
        depth: usize,
    ) -> EmitResult<Vec<ir::Value>> {
        let left_node = child(self.mir, node, Role::Left)?;
        let right_node = child(self.mir, node, Role::Right)?;
        let left_ty = self.ty(left_node)?;
        let right_ty = self.ty(right_node)?;
        let left_never = self.layouts.is_never(TypeKey::try_from(left_ty)?)?;
        let right_never = self.layouts.is_never(TypeKey::try_from(right_ty)?)?;
        if left_never {
            self.expression(left_node, depth + 1)?;
            return Err("native Never operand produced a value".into());
        }
        let metadata = |ty: telora_core::mir::TypeId| {
            matches!(
                self.mir.types[ty.index()].constructor,
                TypeConstructor::Type | TypeConstructor::TypeOf
            )
        };
        let variants = &self.layouts.variant_payloads[left_ty.index()];
        let nullary_enum =
            left_ty == right_ty && !variants.is_empty() && variants.iter().all(Option::is_none);
        if matches!(op, B::Equal | B::NotEqual)
            && ((metadata(left_ty) && metadata(right_ty)) || nullary_enum)
        {
            let left = self.expression(left_node, depth + 1)?;
            let right = self.expression(right_node, depth + 1)?;
            let condition = self.builder.ins().icmp(
                if op == B::Equal {
                    IntCC::Equal
                } else {
                    IntCC::NotEqual
                },
                left[2],
                right[2],
            );
            let result = self.builder.ins().uextend(types::I64, condition);
            return self.scalar_result(node, result);
        }
        if left_ty != right_ty && !right_never {
            return Err(format!("native binary operands require the solved same type: {op:?} at {:?}, left {:?}, right {:?}", self.mir.hir[node.index()].location, self.mir.types[left_ty.index()], self.mir.types[right_ty.index()]).into());
        }
        let kind = &self.mir.types[left_ty.index()].constructor;
        if *kind == TypeConstructor::String && matches!(op, B::LessThan | B::LessThanOrEqual | B::GreaterThan | B::GreaterThanOrEqual) {
            let mut words = self.expression(left_node, depth + 1)?;
            words.extend(self.expression(right_node, depth + 1)?);
            let data = self.stack_words(&words)?;
            let mode = match op { B::LessThan => 0, B::LessThanOrEqual => 1, B::GreaterThan => 2, _ => 3 };
            let mode = self.builder.ins().iconst(types::I64, mode);
            return self.object(node, helpers::TEXT_ORDER, TypeKey::try_from(self.ty(node)?)?, data, mode);
        }
        if *kind == TypeConstructor::Float && op == B::Remainder {
            let mut words = self.expression(left_node, depth + 1)?;
            words.extend(self.expression(right_node, depth + 1)?);
            let data = self.stack_words(&words)?;
            let zero = self.builder.ins().iconst(types::I64, 0);
            return self.object(node, helpers::FLOAT_REMAINDER, TypeKey::try_from(self.ty(node)?)?, data, zero);
        }
        if matches!(kind, TypeConstructor::String | TypeConstructor::Bytes) && matches!(op, B::Equal | B::NotEqual) {
            let operation = if *kind == TypeConstructor::Bytes { helpers::BYTES_EQUAL } else { helpers::TEXT_EQUAL };
            let mut words = self.expression(left_node, depth + 1)?;
            words.extend(self.expression(right_node, depth + 1)?);
            let data = self.stack_words(&words)?;
            let zero = self.builder.ins().iconst(types::I64, 0);
            let mut result = self.object(node, operation, TypeKey::try_from(self.ty(node)?)?, data, zero)?;
            if op == B::NotEqual { result[2] = self.builder.ins().bxor_imm_u(result[2], 1); }
            return Ok(result);
        }
        if matches!(op, B::And | B::Or) {
            if *kind != TypeConstructor::Bool {
                return Err("native logical operation requires Bool".into());
            }
            let left = self.expression(left_node, depth + 1)?[2];
            let rhs = self.builder.create_block();
            let join = self.builder.create_block();
            self.builder.append_block_param(join, types::I64);
            let args = [ir::BlockArg::from(left)];
            if op == B::And {
                self.builder.ins().brif(left, rhs, &[], join, &args);
            } else {
                self.builder.ins().brif(left, join, &args, rhs, &[]);
            }
            self.builder.switch_to_block(rhs);
            self.builder.seal_block(rhs);
            match self.expression(right_node, depth + 1) {
                Ok(value) => {
                    self.builder
                        .ins()
                        .jump(join, &[ir::BlockArg::from(value[2])]);
                }
                Err(EmitError::Diverged) => {}
                Err(error) => return Err(error),
            }
            self.builder.switch_to_block(join);
            self.builder.seal_block(join);
            return self.scalar_result(node, self.builder.block_params(join)[0]);
        }
        if !matches!(
            kind,
            TypeConstructor::Int | TypeConstructor::Float | TypeConstructor::Bool
        ) {
            if matches!(op, B::Equal | B::NotEqual) {
                let mut words = self.expression(left_node, depth + 1)?;
                words.extend(self.expression(right_node, depth + 1)?);
                let data = self.stack_words(&words)?;
                let zero = self.builder.ins().iconst(types::I64, 0);
                let mut result = self.object(node, helpers::EQUAL, TypeKey::try_from(self.ty(node)?)?, data, zero)?;
                if op == B::NotEqual { result[2] = self.builder.ins().bxor_imm_u(result[2], 1); }
                return Ok(result);
            }
            return Err("native non-scalar operator is not yet linked".into());
        }
        let left = self.expression(left_node, depth + 1)?[2];
        let right = self.expression(right_node, depth + 1)?[2];
        let comparison = matches!(
            op,
            B::Equal
                | B::NotEqual
                | B::LessThan
                | B::LessThanOrEqual
                | B::GreaterThan
                | B::GreaterThanOrEqual
        );
        let result = if *kind == TypeConstructor::Float {
            let l = self
                .builder
                .ins()
                .bitcast(types::F64, MemFlagsData::new(), left);
            let r = self
                .builder
                .ins()
                .bitcast(types::F64, MemFlagsData::new(), right);
            if comparison {
                let cc = match op {
                    B::Equal => FloatCC::Equal,
                    B::NotEqual => FloatCC::NotEqual,
                    B::LessThan => FloatCC::LessThan,
                    B::LessThanOrEqual => FloatCC::LessThanOrEqual,
                    B::GreaterThan => FloatCC::GreaterThan,
                    _ => FloatCC::GreaterThanOrEqual,
                };
                let result = self.builder.ins().fcmp(cc, l, r);
                self.builder.ins().uextend(types::I64, result)
            } else {
                let result = match op {
                    B::Add => self.builder.ins().fadd(l, r),
                    B::Subtract => self.builder.ins().fsub(l, r),
                    B::Multiply => self.builder.ins().fmul(l, r),
                    B::Divide => self.builder.ins().fdiv(l, r),
                    _ => return Err("native Float operation not yet supported".into()),
                };
                let bits = self
                    .builder
                    .ins()
                    .bitcast(types::I64, MemFlagsData::new(), result);
                let exponent = self
                    .builder
                    .ins()
                    .band_imm_s(bits, 0x7ff0_0000_0000_0000i64);
                let nonfinite =
                    self.builder
                        .ins()
                        .icmp_imm_s(IntCC::Equal, exponent, 0x7ff0_0000_0000_0000i64);
                self.fail_if(
                    node,
                    nonfinite,
                    "floating-point arithmetic produced a non-finite value",
                )?;
                bits
            }
        } else if comparison {
            let cc = match op {
                B::Equal => IntCC::Equal,
                B::NotEqual => IntCC::NotEqual,
                B::LessThan => IntCC::SignedLessThan,
                B::LessThanOrEqual => IntCC::SignedLessThanOrEqual,
                B::GreaterThan => IntCC::SignedGreaterThan,
                _ => IntCC::SignedGreaterThanOrEqual,
            };
            let result = self.builder.ins().icmp(cc, left, right);
            self.builder.ins().uextend(types::I64, result)
        } else if *kind == TypeConstructor::Int {
            match op {
                B::Add | B::Subtract | B::Multiply => {
                    let (result, overflow) = match op {
                        B::Add => self.builder.ins().sadd_overflow(left, right),
                        B::Subtract => self.builder.ins().ssub_overflow(left, right),
                        _ => self.builder.ins().smul_overflow(left, right),
                    };
                    self.fail_if(node, overflow, "integer arithmetic overflowed")?;
                    result
                }
                B::Divide | B::Remainder => {
                    let zero = self.builder.ins().icmp_imm_s(IntCC::Equal, right, 0);
                    self.fail_if(
                        node,
                        zero,
                        if op == B::Divide {
                            "integer division by zero"
                        } else {
                            "integer remainder by zero"
                        },
                    )?;
                    let min = self.builder.ins().icmp_imm_s(IntCC::Equal, left, i64::MIN);
                    let minusone = self.builder.ins().icmp_imm_s(IntCC::Equal, right, -1);
                    let overflow = self.builder.ins().band(min, minusone);
                    self.fail_if(node, overflow, "integer arithmetic overflowed")?;
                    if op == B::Divide {
                        self.builder.ins().sdiv(left, right)
                    } else {
                        self.builder.ins().srem(left, right)
                    }
                }
                B::BitAnd => self.builder.ins().band(left, right),
                B::BitOr => self.builder.ins().bor(left, right),
                B::BitXor => self.builder.ins().bxor(left, right),
                _ => return Err("native Int operation not yet supported".into()),
            }
        } else {
            return Err("native Bool operation not yet supported".into());
        };
        self.scalar_result(node, result)
    }
}
