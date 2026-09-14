use super::*;
use std::collections::BTreeMap;

impl Mir {
    pub(super) fn valid_generic_references(&self) -> bool {
        self.generic_references.iter().enumerate().all(|(index, reference)| {
            let Some(slot) = self.hir[index].resolution else { return reference.is_none(); };
            let Some(ResolveState::Bound(target)) = self.resolve_slots.get(slot.index()) else { return reference.is_none(); };
            let Some(reference) = reference else {
                return !self.required_types[index] || self.symbol_schemes[target.index()].is_none();
            };
            match reference {
                GenericReference::Scheme { symbol, scheme } => {
                    symbol == target
                        && self.symbol_schemes.get(symbol.index()) == Some(&Some(*scheme))
                        && self.type_instances[index].is_empty()
                        && self.ty_slots.get(index) == self.symbol_types.get(symbol.index())
                            .and_then(|slot| self.ty_slots.get(slot.index()))
                }
                GenericReference::Instance(id) => {
                    self.generic_instances.get(id.index()).is_some_and(|instance| {
                        instance.symbol == *target
                            && !self.type_instances[index].is_empty()
                            && self.type_instances[index].iter().all(|(parameter, slot)| {
                                instance.arguments.iter().any(|(p, ty)| parameter == p
                                    && self.ty_slots.get(slot.index()) == Some(&TypeState::Known(*ty)))
                            })
                    })
                }
            }
        })
    }

    fn scheme_bounds(&self, parameters: &[SymbolId]) -> Option<Vec<(u32, TypeId)>> {
        let mut bounds = vec![];
        for (index, parameter) in parameters.iter().enumerate() {
            for declaration in &self.symbols[parameter.index()].declarations {
                for edge in &self.hir[declaration.index()].children {
                    if edge.role != Role::Bound {
                        continue;
                    }
                    let TypeState::Known(ty) = self.ty_slots[edge.node.ty().index()] else {
                        return None;
                    };
                    bounds.push((index as u32, ty));
                }
            }
        }
        Some(bounds)
    }

    pub(super) fn valid_type_schemes(&self) -> bool {
        if self.symbol_schemes.len() != self.symbols.len()
            || self
                .scheme_nodes
                .iter()
                .enumerate()
                .any(|(index, node)| match node {
                    SchemeNode::Known(ty) => ty.index() >= self.types.len(),
                    SchemeNode::Apply { arguments, .. } => {
                        arguments.iter().any(|child| child.index() >= index)
                    }
                    SchemeNode::Bound(_) => false,
                })
        {
            return false;
        }
        for (index, id) in self.symbol_schemes.iter().enumerate() {
            let expected = !self.symbol_generics[index].is_empty()
                && matches!(
                    self.symbols[index].kind,
                    SymbolKind::Declaration(
                        BindingKind::Def
                            | BindingKind::Let
                            | BindingKind::Native
                            | BindingKind::Decl
                    )
                );
            let Some(id) = id else {
                if expected {
                    return false;
                } else {
                    continue;
                }
            };
            if !expected {
                return false;
            }
            let Some(scheme) = self.type_schemes.get(id.index()) else {
                return false;
            };
            let parameters = &self.symbol_generics[index];
            if scheme.parameter_count as usize != parameters.len() {
                return false;
            }
            let TypeState::Known(signature) = self.ty_slots[self.symbol_types[index].index()]
            else {
                return false;
            };
            if !self.scheme_matches(parameters, signature, scheme.body) {
                return false;
            }
            let Some(bounds) = self.scheme_bounds(parameters) else {
                return false;
            };
            if scheme.bounds.windows(2).any(|pair| pair[0] >= pair[1]) {
                return false;
            }
            if bounds.iter().any(|&(parameter, ty)| {
                !scheme.bounds.iter().any(|&(other, id)| {
                    parameter == other && self.scheme_matches(parameters, ty, id)
                })
            }) || scheme.bounds.iter().any(|&(parameter, id)| {
                !bounds.iter().any(|&(other, ty)| {
                    parameter == other && self.scheme_matches(parameters, ty, id)
                })
            }) {
                return false;
            }
        }
        true
    }

    fn scheme_matches(
        &self,
        parameters: &[SymbolId],
        signature: TypeId,
        body: SchemeNodeId,
    ) -> bool {
        let mut pending = vec![(signature, body)];
        let mut seen = std::collections::BTreeSet::new();
        while let Some((ty, id)) = pending.pop() {
            if !seen.insert((ty, id)) {
                continue;
            }
            let Some(source) = self.types.get(ty.index()) else {
                return false;
            };
            match self.scheme_nodes.get(id.index()) {
                Some(SchemeNode::Bound(index)) => {
                    if !parameters.get(*index as usize).is_some_and(|&parameter| {
                        source.constructor == TypeConstructor::Parameter(parameter)
                    }) {
                        return false;
                    }
                }
                Some(SchemeNode::Apply {
                    constructor,
                    arguments,
                }) => {
                    if source.constructor != *constructor
                        || source.arguments.len() != arguments.len()
                    {
                        return false;
                    }
                    pending.extend(
                        source
                            .arguments
                            .iter()
                            .copied()
                            .zip(arguments.iter().copied()),
                    );
                }
                Some(SchemeNode::Known(known)) if *known == ty => {
                    // A supposedly closed subtree cannot hide a binder.
                    let mut subtypes = vec![ty];
                    let mut visited = std::collections::BTreeSet::new();
                    while let Some(ty) = subtypes.pop() {
                        if !visited.insert(ty) {
                            continue;
                        }
                        let source = &self.types[ty.index()];
                        if let TypeConstructor::Parameter(parameter) = source.constructor
                            && parameters.contains(&parameter)
                        {
                            return false;
                        }
                        subtypes.extend(source.arguments.iter().copied());
                    }
                }
                _ => return false,
            }
        }
        true
    }

    /// Build principal schemes once, in stable symbol order. Only the part of
    /// a signature depending on its binders needs scheme application nodes;
    /// independent subtypes retain their canonical TypeId.
    pub(crate) fn build_type_schemes(&mut self) {
        self.symbol_schemes = vec![None; self.symbols.len()];
        self.type_schemes.clear();
        self.scheme_nodes.clear();
        let mut nodes = BTreeMap::new();
        let mut schemes = BTreeMap::new();
        for index in 0..self.symbols.len() {
            if self.symbol_generics[index].is_empty()
                || !matches!(
                    self.symbols[index].kind,
                    SymbolKind::Declaration(
                        BindingKind::Def
                            | BindingKind::Let
                            | BindingKind::Native
                            | BindingKind::Decl
                    )
                )
            {
                continue;
            }
            let TypeState::Known(signature) = self.ty_slots[self.symbol_types[index].index()]
            else {
                continue;
            };
            let Some(bounds) = self.scheme_bounds(&self.symbol_generics[index]) else {
                continue;
            };
            let binders = self.symbol_generics[index]
                .iter()
                .enumerate()
                .map(|(index, &symbol)| (symbol, index as u32))
                .collect::<BTreeMap<_, _>>();
            let mut translated = BTreeMap::new();
            let mut pending = vec![(signature, false)];
            pending.extend(bounds.iter().map(|&(_, ty)| (ty, false)));
            while let Some((ty, ready)) = pending.pop() {
                if translated.contains_key(&ty) {
                    continue;
                }
                let source = &self.types[ty.index()];
                if !ready {
                    pending.push((ty, true));
                    pending.extend(source.arguments.iter().rev().map(|&child| (child, false)));
                    continue;
                }
                let node = if let TypeConstructor::Parameter(parameter) = source.constructor
                    && let Some(&index) = binders.get(&parameter)
                {
                    SchemeNode::Bound(index)
                } else if source.arguments.iter().all(|child| {
                    let id: SchemeNodeId = translated[child];
                    matches!(self.scheme_nodes[id.index()], SchemeNode::Known(_))
                }) {
                    SchemeNode::Known(ty)
                } else {
                    SchemeNode::Apply {
                        constructor: source.constructor.clone(),
                        arguments: source
                            .arguments
                            .iter()
                            .map(|child| translated[child])
                            .collect(),
                    }
                };
                let next = SchemeNodeId(
                    self.scheme_nodes
                        .len()
                        .try_into()
                        .expect("scheme arena capacity"),
                );
                let id = *nodes.entry(node.clone()).or_insert_with(|| {
                    self.scheme_nodes.push(node);
                    next
                });
                translated.insert(ty, id);
            }
            let mut bounds = bounds
                .iter()
                .map(|&(parameter, ty)| (parameter, translated[&ty]))
                .collect::<Vec<_>>();
            bounds.sort_unstable();
            bounds.dedup();
            let scheme = TypeScheme {
                parameter_count: binders.len() as u32,
                body: translated[&signature],
                bounds,
            };
            let next = TypeSchemeId(self.type_schemes.len().try_into().expect("scheme capacity"));
            let id = *schemes.entry(scheme.clone()).or_insert_with(|| {
                self.type_schemes.push(scheme);
                next
            });
            self.symbol_schemes[index] = Some(id);
        }
    }
}
