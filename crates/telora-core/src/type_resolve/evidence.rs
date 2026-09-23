use super::*;
use std::collections::BTreeMap;

type Canonical = BTreeMap<(TypeConstructor, Vec<TypeId>), TypeId>;
mod queue;
#[cfg(test)]
mod tests;

impl Solver<'_> {
    fn from_dyn_fields_trait(&self, trait_type: TypeId) -> bool {
        let TypeConstructor::Nominal(symbol) = self.mir.types[trait_type.index()].constructor
        else {
            return false;
        };
        let definition = &self.mir.symbols[symbol.index()];
        definition.name == "FromDynFields"
            && definition.module.is_some_and(|module| {
                self.mir.modules[module.index()]
                    .native
                    .as_ref()
                    .is_some_and(|native| native.id == 2)
            })
    }

    fn from_dyn_fields_subject(&self, subject: TypeId) -> bool {
        let TypeConstructor::Nominal(symbol) = self.mir.types[subject.index()].constructor else {
            return false;
        };
        self.mir.type_definitions.iter().any(|definition| {
            definition.symbol == symbol
                && matches!(
                    definition.operation,
                    TypeOperation::Struct | TypeOperation::Newtype
                )
        })
    }

    pub(super) fn is_trait(&self, symbol: SymbolId) -> bool {
        self.mir.symbols[symbol.index()].kind == SymbolKind::Declaration(BindingKind::Trait)
    }

    fn collect_implementations(&mut self) {
        for index in 0..self.mir.symbols.len() {
            if self.mir.symbols[index].kind != SymbolKind::Declaration(BindingKind::Impl) {
                continue;
            }
            let Some(trait_type) = self.known(self.mir.symbol_types[index]) else {
                continue;
            };
            let valid = matches!(self.mir.types[trait_type.index()].constructor, TypeConstructor::Nominal(symbol) if self.is_trait(symbol));
            if !valid {
                let declaration = self.mir.symbols[index].declarations[0];
                self.mir.diagnostics.push(Diagnostic::error(
                    "impl target must be a trait application",
                    self.mir.hir[declaration.index()].location,
                ));
                continue;
            }
            if self.from_dyn_fields_trait(trait_type)
                && self.mir.symbols[index].module.is_none_or(|module| {
                    self.mir.modules[module.index()]
                        .native
                        .as_ref()
                        .is_none_or(|native| native.id != 2)
                })
            {
                let declaration = self.mir.symbols[index].declarations[0];
                self.mir.diagnostics.push(Diagnostic::error(
                    "FromDynFields is a compiler-owned trait and cannot be implemented by user code",
                    self.mir.hir[declaration.index()].location,
                ));
                continue;
            }
            let mut requirements = vec![];
            for &parameter in &self.mir.symbol_generics[index] {
                for &declaration in &self.mir.symbols[parameter.index()].declarations {
                    for bound in self.children(declaration, Role::Bound) {
                        if let Some(bound) = self.known(bound.ty()) {
                            requirements.push((parameter, bound));
                        }
                    }
                }
            }
            let compiler_fallback =
                self.compiler_codec_fallback(SymbolId(index as u32), trait_type, &requirements);
            self.mir.trait_implementations.push(TraitImplementation {
                symbol: SymbolId(index as u32),
                trait_type,
                requirements,
                compiler_fallback,
            });
        }
        for (index, implementation) in self.mir.trait_implementations.iter().enumerate() {
            for other in &self.mir.trait_implementations[..index] {
                if self.fallback_rank(implementation) == self.fallback_rank(other)
                    && self.overlapping_patterns(implementation.trait_type, other.trait_type)
                {
                    let here = self.mir.symbols[implementation.symbol.index()].declarations[0];
                    let previous = self.mir.symbols[other.symbol.index()].declarations[0];
                    self.mir.diagnostics.push(
                        Diagnostic::error(
                            format!(
                                "overlapping trait implementations: {} and {}",
                                self.diagnostic_type(
                                    self.mir.symbol_types[implementation.symbol.index()]
                                ),
                                self.diagnostic_type(self.mir.symbol_types[other.symbol.index()])
                            ),
                            self.mir.hir[here.index()].location,
                        )
                        .with_secondary(
                            "other implementation",
                            self.mir.hir[previous.index()].location,
                        ),
                    );
                }
            }
        }
    }

    pub(super) fn prove_bounds(&mut self) {
        self.collect_implementations();
        for index in 0..self.mir.hir.len() {
            if !matches!(self.mir.hir[index].kind, HirKind::TypeParameter) {
                continue;
            }
            let mut seen = BTreeMap::new();
            for bound in self.children(HirId(index as u32), Role::Bound) {
                if let Some(ty) = self.known(bound.ty())
                    && let Some(previous) = seen.insert(ty, bound)
                {
                    self.mir.diagnostics.push(
                        Diagnostic::error(
                            format!(
                                "duplicate generic bound {}",
                                self.diagnostic_bound(bound.ty())
                            ),
                            self.mir.hir[bound.index()].location,
                        )
                        .with_secondary("previous bound", self.mir.hir[previous.index()].location),
                    );
                }
            }
        }
        let mut canonical = self
            .mir
            .types
            .iter()
            .enumerate()
            .map(|(i, t)| {
                (
                    (t.constructor.clone(), t.arguments.clone()),
                    TypeId(i as u32),
                )
            })
            .collect::<Canonical>();
        let requirements = self
            .mir
            .bound_requirements
            .iter()
            .map(|r| self.known(r.subject).zip(self.known(r.bound)))
            .collect::<Vec<_>>();
        let roots = requirements
            .into_iter()
            .map(|types| types.map(|(subject, bound)| self.request_evidence(subject, bound)))
            .collect::<Vec<_>>();
        self.solve_pending_evidence(&mut canonical, None);
        for (index, root) in roots.into_iter().enumerate() {
            let state = root.map_or(BoundState::Unresolved, |root| self.mir.evidence[root].state);
            self.mir.bound_requirements[index].state = state;
            self.mir.bound_requirements[index].evidence = root;
            let reference = self.mir.bound_requirements[index].reference;
            if let Some(MemberSelection::TraitMember { index, .. }) =
                self.mir.member_selections[reference.index()]
            {
                self.mir.member_selections[reference.index()] =
                    Some(MemberSelection::TraitMember {
                        index,
                        implementation: root
                            .and_then(|root| self.mir.evidence[root].implementation),
                    });
            }
            if matches!(
                state,
                BoundState::Rejected | BoundState::Unresolved | BoundState::Ambiguous
            ) && !self.expansion_exhausted
            {
                let requirement = &self.mir.bound_requirements[index];
                let subject = self.diagnostic_type(requirement.subject);
                let bound = self.diagnostic_bound(requirement.bound);
                let property_bound = self
                    .known(requirement.bound)
                    .and_then(|ty| self.meta_type(ty))
                    .is_some_and(|ty| {
                        matches!(
                            self.mir.types[ty.index()].constructor,
                            TypeConstructor::PropertyBound | TypeConstructor::OptionalPropertyBound
                        )
                    });
                let message = match state {
                    BoundState::Rejected if property_bound => {
                        format!("{subject} does not satisfy {bound}: no static evidence")
                    }
                    BoundState::Rejected => {
                        format!("{subject} does not implement {bound}: no static evidence")
                    }
                    BoundState::Unresolved => {
                        format!("cannot establish {bound} for {subject}: unresolved type evidence")
                    }
                    BoundState::Ambiguous => {
                        format!("{subject} has overlapping implementations of {bound}")
                    }
                    _ => unreachable!(),
                };
                let node = requirement.reference;
                self.mir.diagnostics.push(Diagnostic::error(
                    message,
                    self.mir.hir[node.index()].location,
                ));
            }
        }
    }

    /// Exact substitution of already resolved IDs during static evidence
    /// elaboration. No fresh inference variable or value evaluation is involved.
    pub(super) fn substitute_resolved(
        &mut self,
        ty: TypeId,
        substitutions: &BTreeMap<SymbolId, TypeId>,
        canonical: &mut Canonical,
    ) -> TypeId {
        self.mir
            .substitute_resolved_type(ty, substitutions, canonical)
    }

    pub(super) fn contains_parameter(&self, ty: TypeId) -> bool {
        let mut pending = vec![(ty, None)];
        let mut seen = BTreeSet::new();
        while let Some((ty, binder)) = pending.pop() {
            if !seen.insert((ty, binder)) {
                continue;
            }
            let ty = &self.mir.types[ty.index()];
            let binder = match ty.constructor {
                TypeConstructor::Parameter(_) => return true,
                TypeConstructor::Bound(index) => {
                    if binder.is_none_or(|count| index >= count) {
                        return true;
                    }
                    continue;
                }
                TypeConstructor::Quantified(count) => Some(count),
                _ => binder,
            };
            pending.extend(ty.arguments.iter().map(|&child| (child, binder)));
        }
        false
    }

    fn concrete_implementation(&self, implementation: &TraitImplementation) -> bool {
        !self.contains_parameter(implementation.trait_type)
    }

    fn property_blanket(&self, implementation: &TraitImplementation) -> bool {
        let ty = &self.mir.types[implementation.trait_type.index()];
        let required = implementation
            .requirements
            .iter()
            .filter(|(_, bound)| {
                !self.meta_type(*bound).is_some_and(|raw| {
                    self.mir.types[raw.index()].constructor
                        == TypeConstructor::OptionalPropertyBound
                })
            })
            .collect::<Vec<_>>();
        ty.arguments.len() == 1
            && matches!(
                self.mir.types[ty.arguments[0].index()].constructor,
                TypeConstructor::Parameter(_)
            )
            && !required.is_empty()
            && required.into_iter().any(|(_, bound)| {
                self.meta_type(*bound).is_some_and(|raw| {
                    self.mir.types[raw.index()].constructor == TypeConstructor::PropertyBound
                })
            })
    }

    fn compiler_codec_fallback(
        &self,
        symbol: SymbolId,
        trait_type: TypeId,
        requirements: &[(SymbolId, TypeId)],
    ) -> bool {
        if !requirements.is_empty() {
            return false;
        }
        let Some(module) = self.mir.symbols[symbol.index()].module else {
            return false;
        };
        if self.mir.modules[module.index()]
            .native
            .as_ref()
            .map(|native| native.id)
            != Some(13)
        {
            return false;
        }
        let ty = &self.mir.types[trait_type.index()];
        let TypeConstructor::Nominal(trait_symbol) = ty.constructor else {
            return false;
        };
        matches!(
            self.mir.symbols[trait_symbol.index()].name.as_str(),
            "Encode" | "Decode"
        ) && ty.arguments.len() == 1
            && matches!(
                self.mir.types[ty.arguments[0].index()].constructor,
                TypeConstructor::Parameter(_)
            )
    }

    fn fallback_rank(&self, implementation: &TraitImplementation) -> u8 {
        if implementation.compiler_fallback {
            0
        } else if self.property_blanket(implementation) {
            1
        } else {
            2
        }
    }

    fn pattern_root(&self, mut ty: TypeId, substitutions: &BTreeMap<SymbolId, TypeId>) -> TypeId {
        while let TypeConstructor::Parameter(parameter) = self.mir.types[ty.index()].constructor {
            let Some(&next) = substitutions.get(&parameter) else {
                break;
            };
            ty = next;
        }
        ty
    }

    fn pattern_occurs(
        &self,
        parameter: SymbolId,
        ty: TypeId,
        substitutions: &BTreeMap<SymbolId, TypeId>,
    ) -> bool {
        let mut pending = vec![ty];
        let mut seen = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            let ty = self.pattern_root(ty, substitutions);
            if !seen.insert(ty) {
                continue;
            }
            let ty = &self.mir.types[ty.index()];
            if ty.constructor == TypeConstructor::Parameter(parameter) {
                return true;
            }
            pending.extend(ty.arguments.iter().copied());
        }
        false
    }

    /// Pure overlap predicate over two declaration skeletons. This scratch
    /// substitution never touches the inference graph or its solved evidence.
    fn overlapping_patterns(&self, left: TypeId, right: TypeId) -> bool {
        let mut substitutions = BTreeMap::new();
        let mut pending = vec![(left, right)];
        while let Some((left, right)) = pending.pop() {
            let left = self.pattern_root(left, &substitutions);
            let right = self.pattern_root(right, &substitutions);
            if left == right {
                continue;
            }
            let a = &self.mir.types[left.index()];
            let b = &self.mir.types[right.index()];
            let binding = match (&a.constructor, &b.constructor) {
                (TypeConstructor::Parameter(p), _) => Some((*p, right)),
                (_, TypeConstructor::Parameter(p)) => Some((*p, left)),
                _ => None,
            };
            if let Some((parameter, ty)) = binding {
                if self.pattern_occurs(parameter, ty, &substitutions) {
                    return false;
                }
                substitutions.insert(parameter, ty);
            } else {
                if a.constructor != b.constructor || a.arguments.len() != b.arguments.len() {
                    return false;
                }
                pending.extend(a.arguments.iter().copied().zip(b.arguments.iter().copied()));
            }
        }
        true
    }
}
