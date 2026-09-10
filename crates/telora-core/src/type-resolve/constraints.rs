use super::*;

impl Solver<'_> {
    pub(super) fn finish_bottoms(&mut self) -> bool {
        let mut changed = false;
        for slot in std::mem::take(&mut self.bottom_candidates) {
            let root = self.root(slot);
            if self.mir.ty_slots[root.index()] == TypeState::Unknown {
                let never = self.structure(TypeConstructor::Never, vec![]);
                self.equal(root, never, None);
                changed = true;
            }
        }
        changed
    }

    pub(super) fn fit(&mut self, node: HirId, expected: TypeSlotId, actual: TypeSlotId) {
        self.tasks.push(Task::Fit {
            node,
            expected,
            actual,
        });
    }
    pub(super) fn solve_constraint(&mut self, task: Task) -> Result<Option<Task>, Task> {
        let result = match task {
            Task::Block { node, statements, result } => {
                let bottom = statements.iter().any(|&slot| self.term(slot).is_some_and(|term| term.constructor == TypeConstructor::Never));
                if bottom {
                    self.assign(node, TypeConstructor::Never, vec![]);
                } else if let Some(&slot) = statements.iter().find(|&&slot| matches!(self.mir.ty_slots[self.root(slot).index()], TypeState::Conflicted(_))) {
                    self.same(node, slot);
                } else if statements.iter().any(|&slot| self.term(slot).is_none()) {
                    return Ok(Some(Task::Block { node, statements, result }));
                } else {
                    self.same(node, result);
                }
                self.pending_blocks[node.index()] = false;
                self.revision += 1;
                None
            }
            Task::ShapeEqual { left, right, location } => { self.equal(left, right, location); None }
            Task::Unchecked { node, argument } => {
                let Some(term) = self.term(argument).cloned() else { return Ok(Some(Task::Unchecked { node, argument })); };
                match term.constructor {
                    TypeConstructor::Unchecked | TypeConstructor::Parameter(_) => {}
                    TypeConstructor::Nominal(symbol) if self.nominal_index[symbol.index()].is_some_and(|index| self.mir.type_definitions[index].operation == TypeOperation::Struct) => {}
                    _ => self.conflict(node.ty(), node.ty(), Some(self.mir.hir[node.index()].location), "Unchecked requires a named-field struct type".into()),
                }
                None
            }
            Task::RefineInstance { source, target, arguments, location, constructor } => {
                if self.term(source).is_some_and(|term| term.constructor == constructor) {
                    Some(Task::RefineInstance { source, target, arguments, location, constructor })
                } else {
                    self.substitute_term(source, target, arguments, location)
                }
            }
            Task::BoundContext {
                node,
                subject,
                bound,
            } => match (self.term(subject).cloned(), self.term(bound).cloned()) {
                (Some(subject_term), Some(bound_term))
                    if subject_term.constructor == TypeConstructor::Meta
                        && bound_term.constructor == TypeConstructor::Meta =>
                {
                    if let Some(raw) = self.term(bound_term.arguments[0]).cloned() {
                        match raw.constructor {
                            TypeConstructor::PropertyBound => {}
                            TypeConstructor::Nominal(symbol)
                                if self.is_trait(symbol) && raw.arguments.len() == 1 =>
                            {
                                self.equal(
                                    subject_term.arguments[0],
                                    raw.arguments[0],
                                    Some(self.mir.hir[node.index()].location),
                                );
                            }
                            _ => self.conflict(
                                node.ty(),
                                node.ty(),
                                Some(self.mir.hir[node.index()].location),
                                "generic bound must be a trait or Property(P)".into(),
                            ),
                        }
                        None
                    } else {
                        Some(Task::BoundContext {
                            node,
                            subject,
                            bound,
                        })
                    }
                }
                (Some(_), Some(_)) => {
                    self.conflict(
                        node.ty(),
                        node.ty(),
                        Some(self.mir.hir[node.index()].location),
                        "generic bound must be a type expression".into(),
                    );
                    None
                }
                _ => Some(Task::BoundContext {
                    node,
                    subject,
                    bound,
                }),
            },
            Task::DiagnosticInput { node, input } => {
                if matches!(
                    self.mir.ty_slots[self.root(input).index()],
                    TypeState::Conflicted(_)
                ) {
                    None
                } else if let Some(term) = self.term(input) {
                    if !matches!(
                        term.constructor,
                        TypeConstructor::String | TypeConstructor::Never
                    ) && term.constructor != TypeConstructor::Native(NativeTypeId::BLAME_ERROR)
                    {
                        self.conflict(
                            input,
                            input,
                            Some(self.mir.hir[node.index()].location),
                            "diagnostic error must be String or BlameError".into(),
                        );
                    }
                    None
                } else {
                    Some(Task::DiagnosticInput { node, input })
                }
            }
            Task::Instantiate {
                source,
                target,
                arguments,
                location,
            } => self.substitute_term(source, target, arguments, location),
            Task::Fit {
                node,
                expected,
                actual,
            } => {
                if self.pending_blocks.get(actual.index()).copied().unwrap_or(false) {
                    return Ok(Some(Task::Fit { node, expected, actual }));
                }
                if let (Some(expected_type), Some(actual_type)) = (self.term(expected).cloned(), self.term(actual).cloned())
                    && matches!(expected_type.constructor, TypeConstructor::Nominal(_))
                    && actual_type.constructor == TypeConstructor::Unchecked
                {
                    self.equal(expected, actual_type.arguments[0], Some(self.mir.hir[node.index()].location));
                    if actual.index() < self.mir.hir.len() {
                        self.mir.value_adjustments[actual.index()] = Some(expected);
                    } else {
                        self.conflict(expected, actual, Some(self.mir.hir[node.index()].location), "unchecked conversion requires a value boundary".into());
                    }
                    None
                } else if self
                    .term(actual)
                    .is_some_and(|t| t.constructor == TypeConstructor::Never)
                {
                    // Bottom supplies a fallback only after other evidence has
                    // reached a fixed point; it must not overwrite an annotation.
                    self.bottom_candidates.push(expected);
                    None
                } else if let (Some(expected_type), Some(actual_type)) =
                    (self.term(expected).cloned(), self.term(actual).cloned())
                    && expected_type.constructor == TypeConstructor::Function
                    && actual_type.constructor == TypeConstructor::Function
                    && expected_type.arguments.len() == actual_type.arguments.len()
                {
                    // A function returning Never fits a declared return type;
                    // that is not equality between the two signatures. Keep
                    // the declared skeleton, and check the return separately
                    // after the body's own constraints have contributed evidence.
                    let last = expected_type.arguments.len() - 1;
                    self.revision += 1;
                    for (index, (expected, actual)) in expected_type.arguments.into_iter()
                        .zip(actual_type.arguments).enumerate()
                    {
                        if index == last {
                            self.fit(node, expected, actual);
                        } else {
                            self.equal(expected, actual, Some(self.mir.hir[node.index()].location));
                        }
                    }
                    None
                } else {
                    self.equal(expected, actual, Some(self.mir.hir[node.index()].location));
                    None
                }
            }
            Task::Join { node, values } => {
                // Completion is directional, not equality between branch slots.
                // Prefer a checked branch as the join target, and leave each
                // unchecked source intact with an explicit value adjustment.
                if values.iter().any(|&value| self.term(value).is_some_and(|t| t.constructor == TypeConstructor::Unchecked)) {
                    if values.iter().any(|&value| self.term(value).is_none() && !matches!(self.mir.ty_slots[self.root(value).index()], TypeState::Conflicted(_))) {
                        return Ok(Some(Task::Join { node, values }));
                    }
                    if self.term(node.ty()).is_none() {
                        let target = values.iter().copied().find(|&value| self.term(value).is_some_and(|t| matches!(t.constructor, TypeConstructor::Nominal(_))))
                            .or_else(|| values.iter().copied().find(|&value| self.term(value).is_some_and(|t| t.constructor != TypeConstructor::Never)));
                        if let Some(target) = target { self.same(node, target); }
                    }
                    for value in values { self.fit(node, node.ty(), value); }
                    return Ok(None);
                }
                let mut unknown = false;
                let mut live = false;
                for &value in &values {
                    match self.term(value) {
                        Some(t) if t.constructor == TypeConstructor::Never => {}
                        Some(_) => {
                            self.same(node, value);
                            live = true;
                        }
                        None if matches!(
                            self.mir.ty_slots[self.root(value).index()],
                            TypeState::Conflicted(_)
                        ) =>
                        {
                            self.same(node, value);
                            live = true;
                        }
                        None => {
                            unknown = true;
                        }
                    }
                }
                if !unknown && !live {
                    self.assign(node, TypeConstructor::Never, vec![]);
                }
                if unknown {
                    Some(Task::Join { node, values })
                } else {
                    None
                }
            }
            Task::Call {
                node,
                callee,
                arguments,
            } => self.call(node, callee, arguments),
            Task::Projection {
                node,
                receiver,
                index,
            } => {
                let Some(term) = self.term(receiver).cloned() else {
                    return Ok(Some(Task::Projection {
                        node,
                        receiver,
                        index,
                    }));
                };
                match (index, &term.constructor) {
                    (Some(0), TypeConstructor::Nominal(symbol)) => {
                        if let Some((TypeOperation::Newtype, members)) =
                            self.nominal_members(*symbol, &term.arguments)
                            && let Some((_, Some(payload))) = members.first()
                        {
                            self.same(node, *payload);
                        } else {
                            self.conflict(
                                node.ty(), node.ty(),
                                Some(self.mir.hir[node.index()].location),
                                "invalid projection or index".into(),
                            );
                        }
                    }
                    (Some(i), TypeConstructor::Tuple | TypeConstructor::TupleLiteral) if i < term.arguments.len() => {
                        self.same(node, term.arguments[i])
                    }
                    (None, TypeConstructor::Array | TypeConstructor::ArrayLiteral | TypeConstructor::Dict) => {
                        let key = self.child(node, Role::Index).expect("index");
                        self.assign(
                            key,
                            if matches!(term.constructor, TypeConstructor::Array | TypeConstructor::ArrayLiteral) {
                                TypeConstructor::Int
                            } else {
                                TypeConstructor::String
                            },
                            vec![],
                        );
                        self.same(node, term.arguments[0]);
                    }
                    _ => self.conflict(
                        node.ty(),
                        node.ty(),
                        Some(self.mir.hir[node.index()].location),
                        "invalid projection or index".into(),
                    ),
                }
                None
            }
            Task::ConstructorPattern {
                node,
                constructor,
                payload,
            } => {
                let Some(term) = self.term(constructor).cloned() else {
                    return Ok(Some(Task::ConstructorPattern {
                        node,
                        constructor,
                        payload,
                    }));
                };
                if term.constructor == TypeConstructor::Function {
                    if term.arguments.len() != 2 || payload.is_none() {
                        self.conflict(
                            node.ty(),
                            node.ty(),
                            Some(self.mir.hir[node.index()].location),
                            "constructor pattern payload mismatch".into(),
                        );
                    } else {
                        self.equal(
                            payload.unwrap(),
                            term.arguments[0],
                            Some(self.mir.hir[node.index()].location),
                        );
                        self.same(node, term.arguments[1]);
                    }
                } else if let Some(payload) = payload {
                    // A newtype is callable through its type witness.
                    if term.constructor == TypeConstructor::Meta {
                        self.tasks.push(Task::Call {
                            node,
                            callee: constructor,
                            arguments: vec![payload],
                        });
                    } else {
                        self.conflict(
                            node.ty(),
                            node.ty(),
                            Some(self.mir.hir[node.index()].location),
                            "nullary constructor has no payload".into(),
                        );
                    }
                } else {
                    self.same(node, constructor);
                }
                None
            }
            task => return Err(task),
        };
        Ok(result)
    }

    fn call(
        &mut self,
        node: HirId,
        callee: TypeSlotId,
        arguments: Vec<TypeSlotId>,
    ) -> Option<Task> {
        if matches!(
            self.mir.ty_slots[self.root(callee).index()],
            TypeState::Conflicted(_)
        ) {
            self.same(node, callee);
            return None;
        }
        let Some(term) = self.term(callee).cloned() else {
            return Some(Task::Call {
                node,
                callee,
                arguments,
            });
        };
        match term.constructor {
            TypeConstructor::Function => {
                if term.arguments.len() != arguments.len() + 1 {
                    self.conflict(
                        node.ty(),
                        node.ty(),
                        Some(self.mir.hir[node.index()].location),
                        "function arity mismatch".into(),
                    );
                } else {
                    for (&expected, &actual) in term.arguments.iter().zip(&arguments) {
                        self.fit(node, expected, actual);
                    }
                    self.same(node, *term.arguments.last().unwrap());
                }
            }
            TypeConstructor::TypeFunction(function) => {
                if matches!(function, TypeFunction::Tuple | TypeFunction::Func)
                    && !arguments.is_empty()
                    && let Some(list) = self.term(arguments[0]).cloned()
                    && list.constructor == TypeConstructor::ArrayLiteral
                {
                    let mut raw = vec![];
                    for argument in list.arguments {
                        let ty = self.fresh();
                        let meta = self.structure(TypeConstructor::Meta, vec![ty]);
                        self.equal(argument, meta, Some(self.mir.hir[node.index()].location));
                        raw.push(ty);
                    }
                    let list = self.structure(TypeConstructor::TypeList, raw.clone());
                    let root = self.root(arguments[0]);
                    self.mir.ty_slots[root.index()] = TypeState::ProxyTo(list);
                    if function == TypeFunction::Func && arguments.len() == 2 {
                        let result = self.fresh();
                        let meta = self.structure(TypeConstructor::Meta, vec![result]);
                        self.equal(
                            arguments[1],
                            meta,
                            Some(self.mir.hir[node.index()].location),
                        );
                        raw.push(result);
                    } else if function != TypeFunction::Tuple || arguments.len() != 1 {
                        self.conflict(
                            node.ty(),
                            node.ty(),
                            Some(self.mir.hir[node.index()].location),
                            "type constructor arity mismatch".into(),
                        );
                        return None;
                    }
                    let ty = self.structure(
                        if function == TypeFunction::Tuple {
                            TypeConstructor::Tuple
                        } else {
                            TypeConstructor::Function
                        },
                        raw,
                    );
                    self.assign(node, TypeConstructor::Meta, vec![ty]);
                    return None;
                }
                let (cons, arity) = match function {
                    TypeFunction::Array => (TypeConstructor::Array, 1),
                    TypeFunction::Dict => (TypeConstructor::Dict, 1),
                    TypeFunction::Option => (TypeConstructor::Option, 1),
                    TypeFunction::Result => (TypeConstructor::Result, 2),
                    TypeFunction::FoldControl => (TypeConstructor::FoldControl, 2),
                    TypeFunction::TypeOf => (TypeConstructor::TypeOf, 1),
                    TypeFunction::Unchecked => (TypeConstructor::Unchecked, 1),
                    TypeFunction::Property => (TypeConstructor::PropertyBound, 1),
                    TypeFunction::Tuple => (TypeConstructor::Tuple, arguments.len()),
                    TypeFunction::Func => (TypeConstructor::Function, arguments.len()),
                };
                if arguments.len() != arity {
                    self.conflict(
                        node.ty(),
                        node.ty(),
                        Some(self.mir.hir[node.index()].location),
                        "type constructor arity mismatch".into(),
                    );
                } else {
                    let mut raw = vec![];
                    for argument in arguments {
                        let slot = self.fresh();
                        let meta = self.structure(TypeConstructor::Meta, vec![slot]);
                        self.equal(argument, meta, Some(self.mir.hir[node.index()].location));
                        raw.push(slot);
                    }
                    if function == TypeFunction::Unchecked {
                        self.tasks.push(Task::Unchecked { node, argument: raw[0] });
                    }
                    let ty = self.structure(cons, raw);
                    self.assign(node, TypeConstructor::Meta, vec![ty]);
                }
            }
            TypeConstructor::Meta => {
                // A generic alias can expand to any type constructor. Bind
                // its declared parameter instances, not the shape of its body.
                let instances = self.child(node, Role::Callee)
                    .map(|syntax| self.mir.type_instances[syntax.index()].clone())
                    .unwrap_or_default();
                if !instances.is_empty() && arguments.iter().any(|&argument| self.term(argument).is_none()) {
                    return Some(Task::Call { node, callee, arguments });
                }
                if !instances.is_empty() && arguments.len() == instances.len()
                    && arguments.iter().all(|&argument| self.term(argument).is_some_and(|ty| ty.constructor == TypeConstructor::Meta))
                {
                    for ((_, parameter), argument) in instances.into_iter().zip(arguments) {
                        let meta = self.structure(TypeConstructor::Meta, vec![parameter]);
                        self.equal(argument, meta, Some(self.mir.hir[node.index()].location));
                    }
                    self.same(node, callee);
                    return None;
                }
                let Some(raw) = self.term(term.arguments[0]).cloned() else {
                    return Some(Task::Call {
                        node,
                        callee,
                        arguments,
                    });
                };
                if let TypeConstructor::Nominal(symbol) = raw.constructor {
                    let definition =
                        &self.mir.type_definitions[self.nominal_index[symbol.index()].unwrap()];
                    if !definition.parameters.is_empty()
                        && arguments.len() == definition.parameters.len()
                        && arguments.iter().all(|&a| {
                            self.term(a)
                                .is_some_and(|t| t.constructor == TypeConstructor::Meta)
                        })
                    {
                        for (&parameter, &argument) in raw.arguments.iter().zip(&arguments) {
                            let meta = self.structure(TypeConstructor::Meta, vec![parameter]);
                            self.equal(argument, meta, Some(self.mir.hir[node.index()].location));
                        }
                        self.same(node, callee);
                    } else if definition.operation == TypeOperation::Newtype && arguments.len() == 1
                    {
                        let (_, members) = self.nominal_members(symbol, &raw.arguments).unwrap();
                        self.fit(node, members[0].1.unwrap(), arguments[0]);
                        self.same(node, term.arguments[0]);
                    } else if arguments.iter().any(|&a| self.term(a).is_none()) {
                        return Some(Task::Call {
                            node,
                            callee,
                            arguments,
                        });
                    } else {
                        self.conflict(
                            node.ty(),
                            node.ty(),
                            Some(self.mir.hir[node.index()].location),
                            "invalid nominal type application".into(),
                        );
                    }
                } else {
                    self.conflict(
                        node.ty(),
                        node.ty(),
                        Some(self.mir.hir[node.index()].location),
                        "type is not a constructor".into(),
                    );
                }
            }
            _ => self.conflict(
                node.ty(),
                node.ty(),
                Some(self.mir.hir[node.index()].location),
                "value is not callable".into(),
            ),
        }
        None
    }

    /// Contextual record construction and metadata widening. This is static
    /// evidence, not a runtime conversion or a second attempt at inference.
    pub(super) fn compatible_structure(
        &mut self,
        left: TypeSlotId,
        right: TypeSlotId,
        location: Option<Location>,
    ) -> bool {
        let a = self.term(left).unwrap().clone();
        let b = self.term(right).unwrap().clone();
        if (a.constructor == TypeConstructor::Tuple && b.constructor == TypeConstructor::TupleLiteral)
            || (b.constructor == TypeConstructor::Tuple && a.constructor == TypeConstructor::TupleLiteral)
        {
            let (expected, actual, target, items) = if a.constructor == TypeConstructor::Tuple {
                (left, right, a.arguments, b.arguments)
            } else { (right, left, b.arguments, a.arguments) };
            if target.len() != items.len() { return false; }
            for (expected, actual) in target.into_iter().zip(items) {
                if actual.index() < self.mir.hir.len() {
                    self.fit(HirId(actual.0), expected, actual);
                } else { self.equal(expected, actual, location); }
            }
            self.mir.ty_slots[actual.index()] = TypeState::ProxyTo(expected);
            self.revision += 1;
            return true;
        }
        if a.constructor == TypeConstructor::ArrayLiteral
            && b.constructor == TypeConstructor::ArrayLiteral
        {
            let element = self.fresh();
            for item in a.arguments.into_iter().chain(b.arguments) {
                self.equal(element, item, location);
            }
            let array = self.structure(TypeConstructor::Array, vec![element]);
            self.mir.ty_slots[left.index()] = TypeState::ProxyTo(array);
            self.mir.ty_slots[right.index()] = TypeState::ProxyTo(array);
            self.revision += 1;
            return true;
        }
        if (a.constructor == TypeConstructor::Array
            && b.constructor == TypeConstructor::ArrayLiteral)
            || (b.constructor == TypeConstructor::Array
                && a.constructor == TypeConstructor::ArrayLiteral)
        {
            let (expected, actual, element, items) = if a.constructor == TypeConstructor::Array {
                (left, right, a.arguments[0], b.arguments)
            } else {
                (right, left, b.arguments[0], a.arguments)
            };
            for item in items {
                if item.index() < self.mir.hir.len() {
                    self.fit(HirId(item.0), element, item);
                } else { self.equal(element, item, location); }
            }
            self.mir.ty_slots[actual.index()] = TypeState::ProxyTo(expected);
            self.revision += 1;
            return true;
        }
        if a.constructor == TypeConstructor::Never || b.constructor == TypeConstructor::Never {
            return true;
        }
        if (a.constructor == TypeConstructor::Type && b.constructor == TypeConstructor::TypeOf)
            || (b.constructor == TypeConstructor::Type && a.constructor == TypeConstructor::TypeOf)
        {
            return true;
        }
        let (expected, actual, nominal, record) =
            if matches!(b.constructor, TypeConstructor::Record(_)) {
                (left, right, a, b)
            } else if matches!(a.constructor, TypeConstructor::Record(_)) {
                (right, left, b, a)
            } else {
                return false;
            };
        let TypeConstructor::Record(names) = record.constructor else {
            unreachable!()
        };
        let fields = match nominal.constructor {
            TypeConstructor::Unchecked => {
                let Some(owner) = self.term(nominal.arguments[0]).cloned() else {
                    self.tasks.push(Task::ShapeEqual { left, right, location });
                    return true;
                };
                let TypeConstructor::Nominal(symbol) = owner.constructor else { return false; };
                let Some((TypeOperation::Struct, members)) = self.nominal_members(symbol, &owner.arguments) else { return false; };
                members.into_iter().map(|(name, ty)| (name, ty.unwrap())).collect::<Vec<_>>()
            }
            TypeConstructor::Nominal(symbol) => {
                let Some((TypeOperation::Struct, members)) =
                    self.nominal_members(symbol, &nominal.arguments)
                else {
                    return false;
                };
                members
                    .into_iter()
                    .map(|(n, t)| (n, t.unwrap()))
                    .collect::<Vec<_>>()
            }
            TypeConstructor::Dict => names
                .iter()
                .map(|name| (name.clone(), nominal.arguments[0]))
                .collect(),
            _ => return false,
        };
        if fields.len() != names.len() || fields.iter().any(|(name, _)| !names.contains(name)) {
            return false;
        }
        for (name, ty) in fields {
            let index = names.iter().position(|n| n == &name).unwrap();
            let value = record.arguments[index];
            if value.index() < self.mir.hir.len() {
                self.fit(HirId(value.0), ty, value);
            } else { self.equal(ty, value, location); }
        }
        self.mir.ty_slots[actual.index()] = TypeState::ProxyTo(expected);
        self.revision += 1;
        true
    }

    pub(super) fn finish_literals(&mut self) -> bool {
        let mut changed = false;
        let count = self.mir.ty_slots.len();
        for index in 0..count {
            let slot = TypeSlotId(index as u32);
            if self.root(slot) != slot {
                continue;
            }
            let Some(term) = self.term(slot).cloned() else {
                continue;
            };
            if term.constructor == TypeConstructor::TupleLiteral {
                let tuple = self.structure(TypeConstructor::Tuple, term.arguments);
                self.mir.ty_slots[index] = TypeState::ProxyTo(tuple);
                self.revision += 1;
                changed = true;
                continue;
            }
            if term.constructor != TypeConstructor::ArrayLiteral {
                continue;
            }
            let element = if term.arguments.is_empty() {
                self.structure(TypeConstructor::Never, vec![])
            } else {
                self.fresh()
            };
            let location = self
                .mir
                .hir
                .iter()
                .enumerate()
                .find(|(i, _)| self.root(TypeSlotId(*i as u32)) == slot)
                .map(|(_, n)| n.location);
            for item in term.arguments {
                self.equal(element, item, location);
            }
            let array = self.structure(TypeConstructor::Array, vec![element]);
            self.mir.ty_slots[index] = TypeState::ProxyTo(array);
            self.revision += 1;
            changed = true;
        }
        changed
    }
}
