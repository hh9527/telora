use super::*;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};

impl Lower<'_, '_> {
    fn require_pattern(&mut self, condition: ir::Value, mismatch: ir::Block) {
        let matched = self.builder.create_block();
        self.builder
            .ins()
            .brif(condition, matched, &[], mismatch, &[]);
        self.builder.switch_to_block(matched);
        self.builder.seal_block(matched);
    }
    fn pattern(
        &mut self,
        node: HirId,
        value: &[ir::Value],
        mismatch: ir::Block,
        depth: usize,
    ) -> EmitResult<()> {
        if depth > 512 {
            return Err("native pattern nesting limit".into());
        }
        let syntax = &self.mir.hir[node.index()];
        if matches!(syntax.kind, HirKind::Wildcard) {
            return Ok(());
        }
        if let HirKind::PatternName(_) = syntax.kind {
            let symbol =
                self.mir.hir_symbols[node.index()].ok_or("native pattern has no binder")?;
            if self.mir.symbols[symbol.index()].resolution == ResolveState::Bound(symbol) {
                self.locals.insert(symbol, value.to_vec());
                return Ok(());
            }
        }
        if let Some(selection) = self.selected_member(node) {
            if matches!(selection, MemberSelection::NewtypePattern) {
                let payload = child(self.mir, node, Role::Pattern)?;
                let data = self.stack_words(value)?;
                let zero = self.builder.ins().iconst(types::I64, 0);
                let contents = self.object(node, helpers::FIELD, TypeKey::try_from(self.ty(payload)?)?, data, zero)?;
                self.pattern(payload, &contents, mismatch, depth + 1)?;
                return Ok(());
            }
            let tag = match selection {
                MemberSelection::Boolean(b) => Some(u32::from(b)),
                MemberSelection::EnumVariant { index } => Some(index),
                _ => None,
            };
            if let Some(tag) = tag {
                let matched = self
                    .builder
                    .ins()
                    .icmp_imm_s(IntCC::Equal, value[2], tag as i64);
                self.require_pattern(matched, mismatch);
                if let Ok(payload) = child(self.mir, node, Role::Pattern) {
                    let data = self.stack_words(value)?;
                    let zero = self.builder.ins().iconst(types::I64, 0);
                    let contents = self.object(
                        node,
                        helpers::PAYLOAD,
                        TypeKey::try_from(self.ty(payload)?)?,
                        data,
                        zero,
                    )?;
                    self.pattern(payload, &contents, mismatch, depth + 1)?;
                }
                return Ok(());
            }
        }
        match syntax.kind {
            HirKind::Int(_) | HirKind::Float(_) | HirKind::String(_) => {
                let expected = self.expression(node, depth + 1)?;
                let condition = match syntax.kind {
                    HirKind::Float(_) => {
                        let actual =
                            self.builder
                                .ins()
                                .bitcast(types::F64, MemFlagsData::new(), value[2]);
                        let expected = self.builder.ins().bitcast(
                            types::F64,
                            MemFlagsData::new(),
                            expected[2],
                        );
                        self.builder.ins().fcmp(FloatCC::Equal, actual, expected)
                    }
                    HirKind::String(_) => {
                        let boolean = self
                            .mir
                            .types
                            .iter()
                            .position(|t| t.constructor == TypeConstructor::Bool)
                            .ok_or("sealed Bool missing")?;
                        let boolean = self.layouts.type_at(boolean)?;
                        let mut values = value.to_vec();
                        values.extend(expected);
                        let data = self.stack_words(&values)?;
                        let zero = self.builder.ins().iconst(types::I64, 0);
                        self.object(node, helpers::TEXT_EQUAL, boolean, data, zero)?[2]
                    }
                    _ => self.builder.ins().icmp(IntCC::Equal, value[2], expected[2]),
                };
                self.require_pattern(condition, mismatch);
            }
            HirKind::TuplePattern | HirKind::StructPattern => {
                let fields = syntax
                    .children
                    .iter()
                    .filter(|e| matches!(e.role, Role::Item | Role::Field))
                    .map(|e| e.node)
                    .collect::<Vec<_>>();
                let owner = TypeKey::try_from(self.ty(node)?)?;
                let data = self.stack_words(value)?;
                for (index, field) in fields.into_iter().enumerate() {
                    let (pattern, index) = if matches!(syntax.kind, HirKind::StructPattern) {
                        let name = child(self.mir, field, Role::Name)?;
                        let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                            return Err("native pattern field name missing".into());
                        };
                        let index = self.layouts.field_names[owner.index()]
                            .iter()
                            .position(|n| n == name)
                            .ok_or("native pattern field not in skeleton")?;
                        (child(self.mir, field, Role::Pattern)?, index)
                    } else {
                        (field, index)
                    };
                    let index = self.builder.ins().iconst(types::I64, index as i64);
                    let contents = self.object(
                        field,
                        helpers::FIELD,
                        TypeKey::try_from(self.ty(pattern)?)?,
                        data,
                        index,
                    )?;
                    self.pattern(pattern, &contents, mismatch, depth + 1)?;
                }
            }
            _ => {
                return Err(format!(
                    "native unsupported pattern {:?} at {:?}",
                    syntax.kind, syntax.location
                )
                .into());
            }
        }
        Ok(())
    }
    fn join_branch(
        &mut self,
        outcome: EmitResult<Vec<ir::Value>>,
        join: ir::Block,
        width: usize,
    ) -> EmitResult<bool> {
        match outcome {
            Ok(value) => {
                if value.len() != width {
                    return Err("native pattern branch width mismatch".into());
                }
                let args = value
                    .into_iter()
                    .map(ir::BlockArg::from)
                    .collect::<Vec<_>>();
                self.builder.ins().jump(join, &args);
                Ok(true)
            }
            Err(EmitError::Diverged) => Ok(false),
            Err(error) => Err(error),
        }
    }
    pub(super) fn pattern_branch(
        &mut self,
        node: HirId,
        depth: usize,
    ) -> EmitResult<Vec<ir::Value>> {
        let value = self.expression(child(self.mir, node, Role::Value)?, depth + 1)?;
        let ty = TypeKey::try_from(self.ty(node)?)?;
        let width = if self.layouts.is_never(ty)? {
            0
        } else {
            self.layouts.words(ty)?
        };
        let join = self.builder.create_block();
        for _ in 0..width {
            self.builder.append_block_param(join, types::I64);
        }
        let mut live = false;
        if matches!(self.mir.hir[node.index()].kind, HirKind::Match) {
            let arms = self.mir.hir[node.index()]
                .children
                .iter()
                .filter(|e| e.role == Role::Arm)
                .map(|e| e.node)
                .collect::<Vec<_>>();
            for arm in arms {
                let mismatch = self.builder.create_block();
                self.pattern(
                    child(self.mir, arm, Role::Pattern)?,
                    &value,
                    mismatch,
                    depth + 1,
                )?;
                let outcome = (|| {
                    if let Ok(guard) = child(self.mir, arm, Role::Guard) {
                        let guard = self.expression(guard, depth + 1)?;
                        self.require_pattern(guard[2], mismatch);
                    }
                    self.expression(child(self.mir, arm, Role::Value)?, depth + 1)
                })();
                live |= self.join_branch(outcome, join, width)?;
                self.builder.switch_to_block(mismatch);
                self.builder.seal_block(mismatch);
            }
            self.report_failure(node, "no match arm accepted the value")?;
        } else {
            let mismatch = self.builder.create_block();
            self.pattern(
                child(self.mir, node, Role::Pattern)?,
                &value,
                mismatch,
                depth + 1,
            )?;
            let then = if matches!(self.mir.hir[node.index()].kind, HirKind::LetElse) {
                Role::Body
            } else {
                Role::Then
            };
            let outcome = self.expression(child(self.mir, node, then)?, depth + 1);
            live |= self.join_branch(outcome, join, width)?;
            self.builder.switch_to_block(mismatch);
            self.builder.seal_block(mismatch);
            let outcome = self.expression(child(self.mir, node, Role::Else)?, depth + 1);
            live |= self.join_branch(outcome, join, width)?;
        }
        if !live {
            return Err(EmitError::Diverged);
        }
        self.builder.switch_to_block(join);
        self.builder.seal_block(join);
        Ok(self.builder.block_params(join).to_vec())
    }
}
