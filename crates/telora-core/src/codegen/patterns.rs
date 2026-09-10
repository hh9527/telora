use super::*;

impl Emitter<'_> {
    // Follow already-bound aliases to their statically selected constructor.
    // This reads MIR links; it never executes a constructor or searches names.
    fn pattern_constructor(&self, start: HirId) -> Result<(crate::Atom, bool), Diagnostic> {
        let mut node = start;
        let mut visited = std::collections::BTreeSet::new();
        while visited.insert(node) {
            match self.mir.member_selections[node.index()] {
                Some(MemberSelection::Boolean(value)) => {
                    return Ok((
                        crate::Atom::builtin(if value {
                            crate::BuiltinAtom::True
                        } else {
                            crate::BuiltinAtom::False
                        }),
                        false,
                    ));
                }
                Some(MemberSelection::EnumVariant { index }) => {
                    // The selected tag belongs to the aliased declaration,
                    // but its applied type belongs to this pattern reference.
                    let ty = self.ty(start)?;
                    let signature = &self.mir.types[ty.index()];
                    let payload = signature.constructor == TypeConstructor::Function;
                    let owner = if payload {
                        *signature.arguments.last().unwrap()
                    } else {
                        ty
                    };
                    let constructor = &self.mir.types[owner.index()].constructor;
                    if let Some((tag, payload)) =
                        crate::type_image::builtin_variant(constructor, index)
                    {
                        return Ok((crate::Atom::named(tag), payload));
                    }
                    if let TypeConstructor::Nominal(symbol) = constructor {
                        let member = self
                            .mir
                            .type_definitions
                            .iter()
                            .find(|d| d.symbol == *symbol)
                            .and_then(|d| d.members.get(index as usize))
                            .ok_or_else(|| self.error(start, "missing solved variant"))?;
                        return Ok((crate::Atom::named(member.name.clone()), payload));
                    }
                    break;
                }
                _ => {}
            }
            if let Some(slot) = self.mir.hir[node.index()].resolution {
                if let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] {
                    if let Some(declaration) = self.mir.symbols[symbol.index()].declarations.last()
                    {
                        node = *declaration;
                        continue;
                    }
                }
            }
            node = match self.mir.hir[node.index()].kind {
                HirKind::Binding { .. } | HirKind::TypeAscription => self.child(node, Role::Value),
                HirKind::TypeApply => self.child(node, Role::Callee),
                _ => break,
            };
        }
        Err(self.error(start, "pattern requires a statically selected constructor"))
    }

    fn pattern(&mut self, node: HirId, value: R, mismatch: LabelId) -> Result<(), Diagnostic> {
        match &self.mir.hir[node.index()].kind {
            HirKind::Wildcard => {}
            HirKind::PatternName(_) => {
                let symbol = self.mir.hir_symbols[node.index()].expect("resolved pattern binder");
                if self.mir.symbols[symbol.index()].resolution == ResolveState::Bound(symbol) {
                    self.locals.push((symbol, value));
                } else {
                    let (tag, has_payload) = self.pattern_constructor(node)?;
                    if has_payload {
                        return Err(self.error(node, "constructor pattern is missing its payload"));
                    }
                    let tag = self.constant(node, Constant::Atom(tag));
                    let condition = self.register();
                    self.emit(
                        node,
                        O::TaggedTagEquals {
                            dst: condition,
                            value,
                            tag,
                        },
                    );
                    self.emit(
                        node,
                        O::JumpIfFalse {
                            condition,
                            target: mismatch,
                        },
                    );
                }
            }
            HirKind::Int(_) | HirKind::Float(_) | HirKind::String(_) => {
                let expected = self.expression(node)?;
                let condition = self.register();
                self.emit(
                    node,
                    O::Equal {
                        dst: condition,
                        left: value,
                        right: expected,
                    },
                );
                self.emit(
                    node,
                    O::JumpIfFalse {
                        condition,
                        target: mismatch,
                    },
                );
            }
            HirKind::ConstructorPattern => {
                if self
                    .newtype_owner(self.child(node, Role::Callee))?
                    .is_some()
                {
                    let dst = self.register();
                    self.emit(
                        node,
                        O::GetTuple {
                            dst,
                            tuple: value,
                            index: 0,
                        },
                    );
                    self.pattern(self.child(node, Role::Pattern), dst, mismatch)?;
                    return Ok(());
                }
                let (tag, has_payload) =
                    self.pattern_constructor(self.child(node, Role::Callee))?;
                let tag = self.constant(node, Constant::Atom(tag));
                let condition = self.register();
                self.emit(
                    node,
                    O::TaggedTagEquals {
                        dst: condition,
                        value,
                        tag,
                    },
                );
                self.emit(
                    node,
                    O::JumpIfFalse {
                        condition,
                        target: mismatch,
                    },
                );
                if has_payload {
                    let dst = self.register();
                    self.emit(node, O::GetTaggedPayload { dst, value });
                    self.pattern(self.child(node, Role::Pattern), dst, mismatch)?;
                }
            }
            HirKind::TuplePattern => {
                for (index, item) in self.children(node, Role::Item).into_iter().enumerate() {
                    let dst = self.register();
                    // Shape is already established by the closed type slots.
                    self.emit(
                        node,
                        O::GetTuple {
                            dst,
                            tuple: value,
                            index,
                        },
                    );
                    self.pattern(item, dst, mismatch)?;
                }
            }
            HirKind::StructPattern => {
                for field in self.children(node, Role::Field) {
                    let name = self.child(field, Role::Name);
                    let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                        unreachable!()
                    };
                    let name = name.clone();
                    let dst = self.register();
                    self.emit(
                        field,
                        O::GetField {
                            dst,
                            dict: value,
                            field: name,
                        },
                    );
                    self.pattern(self.child(field, Role::Pattern), dst, mismatch)?;
                }
            }
            _ => return Err(self.error(node, "pattern lowering is not implemented for this form")),
        }
        Ok(())
    }

    pub(super) fn pattern_branch(&mut self, node: HirId, tail: bool) -> Result<R, Diagnostic> {
        let value = self.expression(self.child(node, Role::Value))?;
        let dst = self.register();
        let done = self.label();
        let scope = self.locals.len();
        if matches!(self.mir.hir[node.index()].kind, HirKind::Match) {
            for arm in self.children(node, Role::Arm) {
                let next = self.label();
                self.pattern(self.child(arm, Role::Pattern), value, next)?;
                if let Some(guard) = self.children(arm, Role::Guard).first() {
                    let condition = self.expression(*guard)?;
                    self.emit(
                        arm,
                        O::JumpIfFalse {
                            condition,
                            target: next,
                        },
                    );
                }
                let result = self.expression_mode(self.child(arm, Role::Value), tail)?;
                self.emit(arm, O::Move { dst, src: result });
                self.emit(arm, O::Jump { target: done });
                self.locals.truncate(scope);
                self.mark(next);
            }
            let message = self.constant(node, Constant::String("non-exhaustive match".into()));
            self.emit(node, O::Panic { message });
        } else {
            let otherwise = self.label();
            self.pattern(self.child(node, Role::Pattern), value, otherwise)?;
            let role = if matches!(self.mir.hir[node.index()].kind, HirKind::IfLet) {
                Role::Then
            } else {
                Role::Body
            };
            let result = self.expression_mode(self.child(node, role), tail)?;
            self.emit(node, O::Move { dst, src: result });
            self.emit(node, O::Jump { target: done });
            self.locals.truncate(scope);
            self.mark(otherwise);
            let result = self.expression_mode(self.child(node, Role::Else), tail)?;
            self.emit(node, O::Move { dst, src: result });
        }
        self.mark(done);
        Ok(dst)
    }
}
