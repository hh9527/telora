use super::*;
use std::collections::BTreeMap;

impl Solver<'_> {
    pub(super) fn native_type(&mut self, symbol: SymbolId) -> Option<TypeSlotId> {
        let declaration = &self.mir.symbols[symbol.index()];
        let id = declaration.native_type?;
        let native = self.mir.modules[declaration.module?.index()]
            .native
            .as_ref()?;
        let rule = native
            .types
            .iter()
            .find(|(slot, _)| *slot == id.slot)?
            .1
            .clone();
        Some(match rule {
            NativeTypeRule::Constructor(function) => {
                self.structure(TypeConstructor::TypeFunction(function), vec![])
            }
            NativeTypeRule::Primitive(constructor) => {
                let raw = self.structure(constructor, vec![]);
                self.structure(TypeConstructor::Meta, vec![raw])
            }
            NativeTypeRule::Opaque => {
                let raw = self.structure(TypeConstructor::Native(id), vec![]);
                self.structure(TypeConstructor::Meta, vec![raw])
            }
        })
    }
    pub(super) fn prepare_definitions(&mut self) {
        for symbol in &self.mir.symbols {
            if symbol.kind != SymbolKind::Export { continue; }
            for &declaration in &symbol.declarations {
                if matches!(self.mir.hir[declaration.index()].kind, HirKind::DictField)
                    && let Some(value) = self.child(declaration, Role::Value) {
                    // The synthetic module export record publishes a scheme;
                    // it is not a monomorphic use of the exported function.
                    self.scheme_references[value.index()] = true;
                }
            }
        }
        for index in 0..self.mir.hir.len() {
            if matches!(
                self.mir.hir[index].kind,
                HirKind::Binding {
                    kind: BindingKind::Export | BindingKind::OpenImport,
                    ..
                }
            ) {
                let mut pending = vec![HirId(index as u32)];
                while let Some(node) = pending.pop() {
                    self.administrative[node.index()] = true;
                    pending.extend(self.mir.hir[node.index()].children.iter().map(|e| e.node));
                }
            }
        }
        self.mir
            .symbol_generics
            .resize_with(self.mir.symbols.len(), Vec::new);
        for index in 0..self.mir.symbols.len() {
            let declarations = self.mir.symbols[index].declarations.clone();
            for declaration in declarations {
                let parameters = self
                    .children(declaration, Role::TypeParameter)
                    .into_iter()
                    .filter_map(|node| self.mir.hir_symbols[node.index()])
                    .collect::<Vec<_>>();
                if !parameters.is_empty() {
                    self.mir.symbol_generics[index] = parameters.clone();
                }
                if let Some(value) = self.child(declaration, Role::Value)
                    && let HirKind::TypeOperation(
                        operation @ (TypeOperation::Struct
                        | TypeOperation::Enum
                        | TypeOperation::Newtype),
                    ) = self.mir.hir[value.index()].kind
                {
                    let symbol = SymbolId(index as u32);
                    let mut members = vec![];
                    for field in self.children(value, Role::Field) {
                        let HirKind::TypeMember { name, nullary } =
                            &self.mir.hir[field.index()].kind
                        else {
                            unreachable!()
                        };
                        let name = name.clone();
                        let payload = if *nullary { None } else { Some(self.fresh()) };
                        members.push(TypeMember {
                            name,
                            syntax: field,
                            payload,
                        });
                    }
                    members.sort_by(|a, b| a.name.cmp(&b.name));
                    self.nominal_index[index] = Some(self.mir.type_definitions.len());
                    self.nominal_owner[value.index()] = Some(symbol);
                    self.mir.type_definitions.push(TypeDefinition {
                        symbol,
                        operation,
                        parameters: parameters.clone(),
                        members,
                    });
                    let arguments = parameters
                        .iter()
                        .map(|&p| self.structure(TypeConstructor::Parameter(p), vec![]))
                        .collect();
                    let raw = self.structure(TypeConstructor::Nominal(symbol), arguments);
                    self.assign(value, TypeConstructor::Meta, vec![raw]);
                }
            }
        }
        for index in 0..self.mir.symbols.len() {
            if let ResolveState::Bound(target) = self.mir.symbols[index].resolution {
                self.mir.symbol_generics[index] = self.mir.symbol_generics[target.index()].clone();
            }
        }
        // Parent relationships are syntax facts; returns never search environments.
        self.mir.propagation_boundaries = vec![None; self.mir.hir.len()];
        let mut pending = self
            .mir
            .modules
            .iter()
            .filter_map(|m| match m.state {
                ModuleState::Source { body, .. } | ModuleState::Data { body } => Some((body, None, body)),
                _ => None,
            })
            .collect::<Vec<_>>();
        while let Some((node, inherited, boundary)) = pending.pop() {
            let boundary = if matches!(self.mir.hir[node.index()].kind, HirKind::Closure) { node } else { boundary };
            let target = if matches!(self.mir.hir[node.index()].kind, HirKind::Closure) {
                self.child(node, Role::ReturnType).map(HirId::ty)
            } else {
                inherited
            };
            self.return_slots[node.index()] = target;
            if matches!(self.mir.hir[node.index()].kind, HirKind::Propagate) {
                self.mir.propagation_boundaries[node.index()] = Some(boundary);
            }
            pending.extend(
                self.mir.hir[node.index()]
                    .children
                    .iter()
                    .map(|edge| (edge.node, target, boundary)),
            );
        }
    }

    pub(super) fn reference_type(&mut self, node: HirId, symbol: SymbolId) {
        if self.scheme_references[node.index()] {
            self.same(node, self.mir.symbol_types[symbol.index()]);
            return;
        }
        if self.generalizations.get(symbol.index()).is_some_and(Option::is_some) {
            self.tasks.push(Task::Reference { node, symbol });
            return;
        }
        let parameters = self.mir.symbol_generics[symbol.index()].clone();
        if parameters.is_empty() {
            self.same(node, self.mir.symbol_types[symbol.index()]);
            return;
        }
        let arguments = parameters
            .into_iter()
            .map(|p| (p, self.fresh()))
            .collect::<Vec<_>>();
        self.mir.type_instances[node.index()] = arguments.clone();
        for &(parameter, subject) in &arguments {
            let declarations = self.mir.symbols[parameter.index()].declarations.clone();
            for declaration in declarations {
                for bound in self.children(declaration, Role::Bound) {
                    let bound = self.instantiate(
                        bound.ty(),
                        &arguments,
                        Some(self.mir.hir[node.index()].location),
                    );
                    self.mir.bound_requirements.push(BoundRequirement {
                        subject,
                        bound,
                        reference: node,
                        state: BoundState::Pending,
                        evidence: None,
                    });
                }
            }
        }
        self.tasks.push(Task::Instantiate {
            source: self.mir.symbol_types[symbol.index()],
            target: node.ty(),
            arguments,
            location: Some(self.mir.hir[node.index()].location),
        });
    }

    pub(super) fn instantiate(
        &mut self,
        source: TypeSlotId,
        arguments: &[(SymbolId, TypeSlotId)],
        location: Option<Location>,
    ) -> TypeSlotId {
        let target = self.fresh();
        self.tasks.push(Task::Instantiate {
            source,
            target,
            arguments: arguments.to_vec(),
            location,
        });
        target
    }

    pub(super) fn nominal_members(
        &mut self,
        symbol: SymbolId,
        arguments: &[TypeSlotId],
    ) -> Option<(TypeOperation, Vec<(String, Option<TypeSlotId>)>)> {
        let definition = &self.mir.type_definitions[self.nominal_index[symbol.index()]?];
        let operation = definition.operation;
        let substitutions = definition
            .parameters
            .iter()
            .copied()
            .zip(arguments.iter().copied())
            .collect::<Vec<_>>();
        let members = definition
            .members
            .iter()
            .map(|m| {
                (
                    m.name.clone(),
                    m.payload,
                    self.mir.hir[m.syntax.index()].location,
                )
            })
            .collect::<Vec<_>>();
        Some((
            operation,
            members
                .into_iter()
                .map(|(name, payload, location)| {
                    (
                        name,
                        payload.map(|slot| self.instantiate(slot, &substitutions, Some(location))),
                    )
                })
                .collect(),
        ))
    }

    pub(super) fn type_operation(&mut self, node: HirId) {
        let Some(owner) = self.nominal_owner[node.index()] else {
            self.unsupported(node);
            return;
        };
        let definition = &self.mir.type_definitions[self.nominal_index[owner.index()].unwrap()];
        let members = definition
            .members
            .iter()
            .map(|m| (m.syntax, m.payload))
            .collect::<Vec<_>>();
        for (syntax, payload) in members {
            if let Some(payload) = payload {
                let annotation = self
                    .child(syntax, Role::Annotation)
                    .expect("payload annotation");
                self.assign(annotation, TypeConstructor::Meta, vec![payload]);
                self.same(syntax, annotation.ty());
            } else {
                self.assign(syntax, TypeConstructor::Tuple, vec![]);
            }
        }
    }

    pub(super) fn substitute_term(
        &mut self,
        source: TypeSlotId,
        target: TypeSlotId,
        arguments: Vec<(SymbolId, TypeSlotId)>,
        location: Option<Location>,
    ) -> Option<Task> {
        if let TypeState::Conflicted(id) = self.mir.ty_slots[self.root(source).index()] {
            let root = self.root(target);
            self.mir.ty_slots[root.index()] = TypeState::Conflicted(id);
            self.revision += 1;
            return None;
        }
        let Some(term) = self.term(source).cloned() else {
            return Some(Task::Instantiate {
                source,
                target,
                arguments,
                location,
            });
        };
        if let TypeConstructor::Parameter(parameter) = term.constructor {
            if let Some((_, argument)) = arguments.iter().find(|(p, _)| *p == parameter) {
                self.equal(target, *argument, location);
                return None;
            }
        }
        // Preserve sharing within this constructor; child templates are filled later.
        let mut children = BTreeMap::new();
        let mut result = vec![];
        for child in term.arguments {
            let root = self.root(child);
            let next = if let Some(&slot) = children.get(&root) {
                slot
            } else {
                let slot = self.instantiate(root, &arguments, location);
                children.insert(root, slot);
                slot
            };
            result.push(next);
        }
        let provisional = matches!(term.constructor, TypeConstructor::Record(_) | TypeConstructor::ArrayLiteral | TypeConstructor::TupleLiteral);
        let constructor = term.constructor;
        let instance = self.structure(constructor.clone(), result);
        self.equal(target, instance, location);
        // An inferred record/array literal can later receive its declared
        // nominal/collection identity. Keep that evidence edge alive: copying
        // only its initial fields loses phantom generic arguments on refinement.
        provisional.then_some(Task::RefineInstance { source, target, arguments, location, constructor })
    }
}
