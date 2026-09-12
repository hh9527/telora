use super::*;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use telora_core::ast::{BinaryOperator as B, UnaryOperator as U};

impl Lower<'_, '_> {
    fn scalar_result(&mut self, node: HirId, bits: ir::Value) -> Result<Vec<ir::Value>> {
        let ty = TypeKey::try_from(known(self.mir, node)?)?;
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
    fn fail_if(&mut self, node: HirId, condition: ir::Value, message: &str) -> Result<()> {
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
        self.builder.ins().return_(&[status]);
        self.builder.switch_to_block(next);
        self.builder.seal_block(next);
        Ok(())
    }
    pub(super) fn unary(&mut self, node: HirId, op: U, depth: usize) -> Result<Vec<ir::Value>> {
        let operand = child(self.mir, node, Role::Operand)?;
        let ty = known(self.mir, operand)?;
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
    pub(super) fn binary(&mut self, node: HirId, op: B, depth: usize) -> Result<Vec<ir::Value>> {
        let left_node = child(self.mir, node, Role::Left)?;
        let right_node = child(self.mir, node, Role::Right)?;
        let left_ty = known(self.mir, left_node)?;
        let right_ty = known(self.mir, right_node)?;
        if left_ty != right_ty {
            return Err("native binary operands require the solved same type".into());
        }
        let kind = &self.mir.types[left_ty.index()].constructor;
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
            let right = self.expression(right_node, depth + 1)?[2];
            self.builder.ins().jump(join, &[ir::BlockArg::from(right)]);
            self.builder.switch_to_block(join);
            self.builder.seal_block(join);
            return self.scalar_result(node, self.builder.block_params(join)[0]);
        }
        if !matches!(
            kind,
            TypeConstructor::Int | TypeConstructor::Float | TypeConstructor::Bool
        ) {
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
                let exponent = self.builder.ins().band_imm_s(bits, 0x7ff0_0000_0000_0000i64);
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
