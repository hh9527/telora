use super::*;
use std::collections::BTreeMap;

type Canonical = BTreeMap<(TypeConstructor, Vec<TypeId>), TypeId>;

fn evidence_slot(
    nodes: &mut Vec<EvidenceNode>,
    indices: &mut BTreeMap<(TypeId, TypeId), usize>,
    subject: TypeId,
    bound: TypeId,
) -> usize {
    *indices.entry((subject, bound)).or_insert_with(|| {
        let id = nodes.len();
        nodes.push(EvidenceNode {
            subject,
            bound,
            state: BoundState::Pending,
            implementation: None,
            arguments: vec![],
            dependencies: vec![],
        });
        id
    })
}

impl Solver<'_> {
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
            self.mir.trait_implementations.push(TraitImplementation {
                symbol: SymbolId(index as u32),
                trait_type,
                requirements,
            });
        }
        for (index, implementation) in self.mir.trait_implementations.iter().enumerate() {
            for other in &self.mir.trait_implementations[..index] {
                if !self.exact_property_pair(implementation, other)
                    && self.overlapping_patterns(implementation.trait_type, other.trait_type)
                {
                    let here = self.mir.symbols[implementation.symbol.index()].declarations[0];
                    let previous = self.mir.symbols[other.symbol.index()].declarations[0];
                    self.mir.diagnostics.push(
                        Diagnostic::error(
                            "overlapping trait implementations",
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
                            "duplicate generic bound",
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
        let mut nodes = vec![];
        let mut indices = BTreeMap::new();
        let roots = self
            .mir
            .bound_requirements
            .iter()
            .map(|r| {
                self.known(r.subject)
                    .zip(self.known(r.bound))
                    .map(|(subject, bound)| evidence_slot(&mut nodes, &mut indices, subject, bound))
            })
            .collect::<Vec<_>>();
        let mut index = 0;
        while index < nodes.len() {
            let (subject, bound) = (nodes[index].subject, nodes[index].bound);
            if let Some(state) = self.direct_evidence(subject, bound) {
                nodes[index].state = state;
                index += 1;
                continue;
            }
            let Some(raw) = self.meta_type(bound) else {
                nodes[index].state = BoundState::Rejected;
                index += 1;
                continue;
            };
            let mut candidates = vec![];
            for implementation in &self.mir.trait_implementations {
                let mut substitutions = BTreeMap::new();
                if self.match_type(implementation.trait_type, raw, &mut substitutions) {
                    candidates.push((implementation.clone(), substitutions));
                }
            }
            // RFC 0260 defines exact concrete precedence over a property
            // blanket. All other overlap remains a declaration diagnostic.
            if candidates
                .iter()
                .any(|(implementation, _)| self.concrete_implementation(implementation))
            {
                candidates
                    .retain(|(implementation, _)| self.concrete_implementation(implementation));
            }
            match candidates.len() {
                0 => nodes[index].state = BoundState::Rejected,
                1 => {
                    let (implementation, substitutions) = candidates.pop().unwrap();
                    nodes[index].implementation = Some(implementation.symbol);
                    nodes[index].arguments = substitutions.iter().map(|(&p, &t)| (p, t)).collect();
                    for (parameter, bound) in implementation.requirements {
                        let Some(&subject) = substitutions.get(&parameter) else {
                            nodes[index].state = BoundState::Unresolved;
                            continue;
                        };
                        let bound = self.substitute_resolved(bound, &substitutions, &mut canonical);
                        let dependency = evidence_slot(&mut nodes, &mut indices, subject, bound);
                        nodes[index].dependencies.push(dependency);
                    }
                }
                _ => nodes[index].state = BoundState::Ambiguous,
            }
            index += 1;
        }
        // Least fixed point over one obligation graph. Cycles cannot prove
        // themselves; there is no speculative solve, rollback or recovery.
        loop {
            let mut changed = false;
            for index in 0..nodes.len() {
                if nodes[index].state != BoundState::Pending {
                    continue;
                }
                if let Some(implementation) = nodes[index].implementation
                    && nodes[index]
                        .dependencies
                        .iter()
                        .all(|&d| nodes[d].state.is_proven())
                {
                    nodes[index].state = BoundState::Implementation(implementation);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for node in &mut nodes {
            if node.state == BoundState::Pending {
                node.state = BoundState::Rejected;
            }
        }
        for (index, root) in roots.into_iter().enumerate() {
            let state = root.map_or(BoundState::Unresolved, |root| nodes[root].state);
            self.mir.bound_requirements[index].state = state;
            self.mir.bound_requirements[index].evidence = root;
            let reference = self.mir.bound_requirements[index].reference;
            if let Some(MemberSelection::TraitMember { index, .. }) =
                self.mir.member_selections[reference.index()]
            {
                self.mir.member_selections[reference.index()] =
                    Some(MemberSelection::TraitMember {
                        index,
                        implementation: root.and_then(|root| nodes[root].implementation),
                    });
            }
            let message = match state {
                BoundState::Rejected => {
                    Some("generic bound has no static evidence (including unproven cycles)")
                }
                BoundState::Unresolved => Some("generic bound remains unresolved"),
                BoundState::Ambiguous => Some("generic bound has overlapping implementations"),
                _ => None,
            };
            if let Some(message) = message {
                let node = self.mir.bound_requirements[index].reference;
                self.mir.diagnostics.push(Diagnostic::error(
                    message,
                    self.mir.hir[node.index()].location,
                ));
            }
        }
        self.mir.evidence = nodes;
    }

    /// Exact substitution of already resolved IDs during static evidence
    /// elaboration. No fresh inference variable or value evaluation is involved.
    pub(super) fn substitute_resolved(
        &mut self,
        ty: TypeId,
        substitutions: &BTreeMap<SymbolId, TypeId>,
        canonical: &mut Canonical,
    ) -> TypeId {
        let template = self.mir.types[ty.index()].clone();
        if let TypeConstructor::Parameter(parameter) = template.constructor {
            return substitutions.get(&parameter).copied().unwrap_or(ty);
        }
        let arguments = template
            .arguments
            .into_iter()
            .map(|a| self.substitute_resolved(a, substitutions, canonical))
            .collect::<Vec<_>>();
        let key = (template.constructor, arguments);
        *canonical.entry(key.clone()).or_insert_with(|| {
            let id = TypeId(self.mir.types.len() as u32);
            self.mir.types.push(ResolvedType {
                constructor: key.0,
                arguments: key.1,
            });
            id
        })
    }

    fn contains_parameter(&self, ty: TypeId) -> bool {
        let ty = &self.mir.types[ty.index()];
        matches!(ty.constructor, TypeConstructor::Parameter(_))
            || ty.arguments.iter().any(|&a| self.contains_parameter(a))
    }

    fn concrete_implementation(&self, implementation: &TraitImplementation) -> bool {
        !self.contains_parameter(implementation.trait_type)
    }

    fn property_blanket(&self, implementation: &TraitImplementation) -> bool {
        let ty = &self.mir.types[implementation.trait_type.index()];
        ty.arguments.len() == 1
            && matches!(
                self.mir.types[ty.arguments[0].index()].constructor,
                TypeConstructor::Parameter(_)
            )
            && !implementation.requirements.is_empty()
            && implementation.requirements.iter().all(|(_, bound)| {
                self.meta_type(*bound).is_some_and(|raw| {
                    self.mir.types[raw.index()].constructor == TypeConstructor::PropertyBound
                })
            })
    }

    fn exact_property_pair(&self, left: &TraitImplementation, right: &TraitImplementation) -> bool {
        (self.concrete_implementation(left) && self.property_blanket(right))
            || (self.concrete_implementation(right) && self.property_blanket(left))
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
        let ty = &self.mir.types[self.pattern_root(ty, substitutions).index()];
        ty.constructor == TypeConstructor::Parameter(parameter)
            || ty
                .arguments
                .iter()
                .any(|&arg| self.pattern_occurs(parameter, arg, substitutions))
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
