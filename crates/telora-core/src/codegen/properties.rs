use super::*;

pub(super) fn native_abi(mir: &Mir, symbol: SymbolId) -> Option<(u32, &str)> {
    let symbol = &mir.symbols[symbol.index()];
    if symbol.kind != SymbolKind::Declaration(BindingKind::Native) {
        return None;
    }
    Some((
        mir.modules[symbol.module?.index()].native.as_ref()?.id,
        &symbol.name,
    ))
}

impl Emitter<'_> {
    pub(super) fn property_call(&mut self, node: HirId) -> Result<Option<R>, Diagnostic> {
        let callee = self.child(node, Role::Callee);
        let Some(slot) = self.mir.hir[callee.index()].resolution else {
            return Ok(None);
        };
        let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] else {
            return Ok(None);
        };
        if native_abi(self.mir, symbol) != Some((25, "get_type_prop")) {
            return Ok(None);
        }
        let arguments = self.children(node, Role::Argument);
        if arguments.len() != 2 {
            return Ok(None);
        }
        let represented = |n| -> Option<TypeId> {
            let ty = &self.mir.types[self.ty(n).ok()?.index()];
            (ty.constructor == TypeConstructor::TypeOf).then(|| ty.arguments[0])
        };
        let (Some(owner_type), Some(property_type)) =
            (represented(arguments[0]), represented(arguments[1]))
        else {
            return Ok(None);
        };
        // Argument expressions keep their normal eager semantics even when the
        // optional query itself can be reduced from static presence evidence.
        let owner = self.expression(arguments[0])?;
        let property = self.expression(arguments[1])?;
        let key = crate::execution_graph::PropertyKey {
            owner: owner_type,
            site: PropertySite::Type,
            property: property_type,
        };
        Ok(Some(if self.graph.property(key).is_some() {
            let dst = self.register();
            self.emit(
                node,
                O::GetTypeProp {
                    dst,
                    owner,
                    property,
                },
            );
            self.emit(node, O::MakeSome { dst, value: dst });
            dst
        } else {
            self.constant(
                node,
                Constant::Atom(crate::Atom::builtin(crate::BuiltinAtom::None)),
            )
        }))
    }

    fn call(&mut self, node: HirId, callee: R, arguments: &[R]) -> R {
        let base = self.register();
        self.emit(
            node,
            O::Move {
                dst: base,
                src: callee,
            },
        );
        for &src in arguments {
            let dst = self.register();
            self.emit(node, O::Move { dst, src });
        }
        self.emit(
            node,
            O::Call {
                base,
                argument_count: arguments.len() as u32,
            },
        );
        base
    }

    pub(super) fn property_thunk(&mut self, record: &PropertyRecord) -> Result<(), Diagnostic> {
        let node = record.providers[0];
        let mut thunk = Self::new(
            self.mir,
            self.graph,
            format!(
                "property:{}:{}",
                record.owner.index(),
                record.property.index()
            ),
        );
        thunk.instance = record.instance;
        let references = record
            .providers
            .iter()
            .flat_map(|n| referenced_globals(self.mir, *n))
            .collect::<std::collections::BTreeSet<_>>();
        let mut captures = vec![];
        for symbol in references {
            if let Some(value) = self.lookup(symbol) {
                let register = thunk.register();
                thunk.locals.push((symbol, register));
                captures.push(value);
            }
        }
        thunk.function.capture_count = captures.len() as u32;
        if let Some(PropertyAdmission::Require { capability, targets }) = record.admission {
            let capability = &self.mir.properties[capability.index()];
            let dependency = self.graph.property(crate::execution_graph::PropertyKey {
                owner: capability.owner, site: capability.site, property: capability.property,
            }).expect("sealed capability task");
            let value = thunk.register();
            thunk.emit(node, O::Demand { dst: value, node: dependency });
            let bits = thunk.register();
            thunk.emit(node, O::GetField { dst: bits, dict: value, field: "bits".into() });
            let mask = thunk.constant(node, Constant::Int(targets));
            let accepted = thunk.register();
            thunk.emit(node, O::BitAnd { dst: accepted, left: bits, right: mask });
            let zero = thunk.constant(node, Constant::Int(0));
            let rejected = thunk.register();
            thunk.emit(node, O::Equal { dst: rejected, left: accepted, right: zero });
            let ready = thunk.label();
            thunk.emit(node, O::JumpIfFalse { condition: rejected, target: ready });
            let message = thunk.constant(node, Constant::String("property type does not support this decorator target".into()));
            let failed = thunk.register();
            thunk.emit(node, O::Raise { action: crate::ast::BlameAction::Fail, dst: failed, message, subjects: vec![] });
            thunk.mark(ready);
        }
        let owner = thunk.constant(node, Constant::SolvedType(record.owner));
        let context = match record.site {
            PropertySite::Type => owner,
            PropertySite::Field(index) | PropertySite::Variant(index) => {
                let TypeConstructor::Nominal(symbol) =
                    self.mir.types[record.owner.index()].constructor
                else {
                    return Err(
                        self.error(node, "member property owner must have a nominal skeleton")
                    );
                };
                let member = self
                    .mir
                    .type_definitions
                    .iter()
                    .find(|d| d.symbol == symbol)
                    .and_then(|d| d.members.get(index as usize))
                    .ok_or_else(|| self.error(node, "missing member property skeleton"))?;
                let position = thunk.constant(node, Constant::Int(index as i64));
                let name = thunk.constant(node, Constant::String(member.name.clone().into()));
                let mut fields = vec![
                    ("owner".into(), owner),
                    ("index".into(), position),
                    ("name".into(), name),
                ];
                let payload = self.mir.type_layouts[record.owner.index()]
                    .as_ref().expect("sealed property owner layout").members[index as usize];
                if matches!(record.site, PropertySite::Field(_)) {
                    let ty = payload.ok_or_else(|| self.error(node, "field has no solved type"))?;
                    fields.push(("ty".into(), thunk.constant(node, Constant::SolvedType(ty))));
                } else {
                    let value = if let Some(ty) = payload {
                        let value = thunk.constant(node, Constant::SolvedType(ty));
                        let dst = thunk.register();
                        thunk.emit(node, O::MakeSome { dst, value });
                        dst
                    } else {
                        thunk.constant(
                            node,
                            Constant::Atom(crate::Atom::builtin(crate::BuiltinAtom::None)),
                        )
                    };
                    fields.push(("payload".into(), value));
                }
                let dst = thunk.register();
                thunk.emit(node, O::MakeDict { dst, fields });
                dst
            }
        };
        let mut previous = thunk.constant(
            node,
            Constant::Atom(crate::Atom::builtin(crate::BuiltinAtom::None)),
        );
        let mut result = previous;
        for (index, &provider) in record.providers.iter().enumerate() {
            let mut callee = thunk.expression(thunk.child(provider, Role::Callee))?;
            if matches!(
                self.mir.hir[provider.index()].kind,
                HirKind::Decorator { configured: true }
            ) {
                let arguments = thunk
                    .children(provider, Role::Argument)
                    .into_iter()
                    .map(|a| thunk.expression(a))
                    .collect::<Result<Vec<_>, _>>()?;
                callee = thunk.call(provider, callee, &arguments);
            }
            result = thunk.call(provider, callee, &[context, previous]);
            if index + 1 < record.providers.len() {
                previous = thunk.register();
                thunk.emit(
                    provider,
                    O::MakeSome {
                        dst: previous,
                        value: result,
                    },
                );
            }
        }
        thunk.emit(node, O::Return { src: result });
        let dst = self.register();
        self.emit(
            node,
            O::MakeClosure {
                dst,
                function: Box::new(thunk.function),
                captures,
            },
        );
        let slot = self
            .graph
            .property(crate::execution_graph::PropertyKey {
                owner: record.owner,
                site: record.site,
                property: record.property,
            })
            .expect("sealed property slot");
        self.emit(
            node,
            O::InstallTask {
                node: slot,
                src: dst,
            },
        );
        Ok(())
    }

    /// Native ABI adapters are selected by admitted module identity and ABI key,
    /// never by spelling at a source call site. Aliases/first-class uses work alike.
    pub(super) fn property_native(
        &mut self,
        node: HirId,
        symbol: SymbolId,
    ) -> Result<Option<R>, Diagnostic> {
        let Some(abi) = native_abi(self.mir, symbol) else {
            return Ok(None);
        };
        let expected = match abi {
            (25, "get_type_prop" | "evidence") => Some(2),
            (25, "get_field_prop" | "get_variant_prop") => Some(3),
            (18, "property") => Some(1),
            _ => None,
        };
        if let Some(arity) = expected {
            let signature = &self.mir.types[self.ty(node)?.index()];
            if signature.constructor != TypeConstructor::Function
                || signature.arguments.len() != arity + 1
            {
                return Err(self.error(
                    node,
                    "property native ABI arity does not match its solved signature",
                ));
            }
        }
        let mut adapter = Self::new(self.mir, self.graph, format!("native:{}", symbol.index()));
        let result = match abi {
            (25, "get_type_prop") => {
                adapter.function.parameter_count = 2;
                let owner = adapter.register();
                let property = adapter.register();
                let dst = adapter.register();
                let condition = adapter.register();
                let absent = adapter.label();
                let done = adapter.label();
                adapter.emit(
                    node,
                    O::HasTypeProp {
                        dst: condition,
                        owner,
                        property,
                    },
                );
                adapter.emit(
                    node,
                    O::JumpIfFalse {
                        condition,
                        target: absent,
                    },
                );
                adapter.emit(
                    node,
                    O::GetTypeProp {
                        dst,
                        owner,
                        property,
                    },
                );
                adapter.emit(node, O::MakeSome { dst, value: dst });
                adapter.emit(node, O::Jump { target: done });
                adapter.mark(absent);
                let none = adapter.constant(
                    node,
                    Constant::Atom(crate::Atom::builtin(crate::BuiltinAtom::None)),
                );
                adapter.emit(node, O::Move { dst, src: none });
                adapter.mark(done);
                dst
            }
            (25, "get_field_prop" | "get_variant_prop") => {
                adapter.function.parameter_count = 3;
                let owner = adapter.register();
                let index = adapter.register();
                let property = adapter.register();
                let dst = adapter.register();
                let condition = adapter.register();
                let absent = adapter.label();
                let done = adapter.label();
                adapter.emit(
                    node,
                    O::HasMemberProp {
                        dst: condition,
                        owner,
                        index,
                        property,
                        variant: abi.1 == "get_variant_prop",
                    },
                );
                adapter.emit(
                    node,
                    O::JumpIfFalse {
                        condition,
                        target: absent,
                    },
                );
                adapter.emit(
                    node,
                    O::GetMemberProp {
                        dst,
                        owner,
                        index,
                        property,
                        variant: abi.1 == "get_variant_prop",
                    },
                );
                adapter.emit(node, O::MakeSome { dst, value: dst });
                adapter.emit(node, O::Jump { target: done });
                adapter.mark(absent);
                let none = adapter.constant(
                    node,
                    Constant::Atom(crate::Atom::builtin(crate::BuiltinAtom::None)),
                );
                adapter.emit(node, O::Move { dst, src: none });
                adapter.mark(done);
                dst
            }
            (25, "evidence") => {
                // Property(P) was proved before sealing. Demand the VM-owned
                // value directly; a failed provider propagates its cached failure.
                adapter.function.parameter_count = 2;
                let owner = adapter.register();
                let property = adapter.register();
                let dst = adapter.register();
                adapter.emit(node, O::GetTypeProp { dst, owner, property });
                dst
            }
            (18, "property") => {
                let factory = &self.mir.types[self.ty(node)?.index()];
                let provider_signature = &self.mir.types[factory.arguments[1].index()];
                if provider_signature.constructor != TypeConstructor::Function || provider_signature.arguments.len() != 3 {
                    return Err(self.error(node, "property native ABI requires a two-argument provider"));
                }
                let attribute_type = provider_signature.arguments[2];
                adapter.function.parameter_count = 1;
                let target = adapter.register();
                let bits = adapter.register();
                let ready = adapter.label();
                for (index, name) in crate::type_image::PROPERTY_TARGET_VARIANTS.iter().enumerate() {
                    let tag = adapter.constant(node, Constant::Atom(crate::Atom::named(*name)));
                    let condition = adapter.register();
                    let next = adapter.label();
                    adapter.emit(node, O::TaggedTagEquals { dst: condition, value: target, tag });
                    adapter.emit(node, O::JumpIfFalse { condition, target: next });
                    let mask = adapter.constant(node, Constant::Int(crate::type_image::PROPERTY_TARGET_MASKS[index]));
                    adapter.emit(node, O::Move { dst: bits, src: mask });
                    adapter.emit(node, O::Jump { target: ready });
                    adapter.mark(next);
                }
                let message = adapter.constant(node, Constant::String("invalid PropertyTarget variant".into()));
                adapter.emit(node, O::Panic { message });
                adapter.mark(ready);
                let mut provider = Self::new(self.mir, self.graph, "<property marker>".into());
                provider.function.parameter_count = 2;
                provider.function.capture_count = 1;
                let _owner = provider.register();
                let previous = provider.register();
                let captured_bits = provider.register();
                let merged = provider.register();
                provider.emit(
                    node,
                    O::Move {
                        dst: merged,
                        src: captured_bits,
                    },
                );
                let none = provider.constant(
                    node,
                    Constant::Atom(crate::Atom::builtin(crate::BuiltinAtom::None)),
                );
                let condition = provider.register();
                provider.emit(
                    node,
                    O::Equal {
                        dst: condition,
                        left: previous,
                        right: none,
                    },
                );
                let reduce = provider.label();
                let done = provider.label();
                provider.emit(
                    node,
                    O::JumpIfFalse {
                        condition,
                        target: reduce,
                    },
                );
                provider.emit(node, O::Jump { target: done });
                provider.mark(reduce);
                let payload = provider.register();
                provider.emit(
                    node,
                    O::GetTaggedPayload {
                        dst: payload,
                        value: previous,
                    },
                );
                let old_bits = provider.register();
                provider.emit(
                    node,
                    O::GetField {
                        dst: old_bits,
                        dict: payload,
                        field: "bits".into(),
                    },
                );
                provider.emit(
                    node,
                    O::BitOr {
                        dst: merged,
                        left: old_bits,
                        right: captured_bits,
                    },
                );
                provider.mark(done);
                let result = provider.register();
                provider.emit(
                    node,
                    O::MakeDict {
                        dst: result,
                        fields: vec![("bits".into(), merged)],
                    },
                );
                provider.emit(node, O::StampType { dst: result, src: result, ty: attribute_type });
                provider.emit(node, O::Return { src: result });
                let dst = adapter.register();
                adapter.emit(
                    node,
                    O::MakeClosure {
                        dst,
                        function: Box::new(provider.function),
                        captures: vec![bits],
                    },
                );
                dst
            }
            _ => return Ok(None),
        };
        adapter.emit(node, O::Return { src: result });
        let dst = self.register();
        self.emit(
            node,
            O::MakeClosure {
                dst,
                function: Box::new(adapter.function),
                captures: vec![],
            },
        );
        self.locals.push((symbol, dst));
        Ok(Some(dst))
    }
}
