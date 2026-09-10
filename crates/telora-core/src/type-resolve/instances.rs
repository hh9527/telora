//! Close generic declaration instances while the static pass still owns MIR.
//! This substitutes solved IDs; it neither re-resolves symbols nor evaluates
//! source code. Codegen receives the completed per-instance node type table.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

type Key = (SymbolId, Vec<(SymbolId, TypeId)>);
type Canonical = BTreeMap<(TypeConstructor, Vec<TypeId>), TypeId>;

impl Solver<'_> {
    pub(super) fn materialize_instances(&mut self) {
        self.mir
            .reference_instances
            .resize(self.mir.hir.len(), None);
        let mut canonical: Canonical = self
            .mir
            .types
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                (
                    (ty.constructor.clone(), ty.arguments.clone()),
                    TypeId(i as u32),
                )
            })
            .collect();
        let mut indices = BTreeMap::<Key, GenericInstanceId>::new();
        self.mir
            .implementation_instances
            .resize(self.mir.hir.len(), None);
        for index in 0..self.mir.bound_requirements.len() {
            let requirement = &self.mir.bound_requirements[index];
            let reference = requirement.reference;
            if !matches!(
                self.mir.member_selections[reference.index()],
                Some(MemberSelection::TraitMember { .. })
            ) {
                continue;
            }
            let Some(evidence) = requirement.evidence.map(|id| &self.mir.evidence[id]) else {
                continue;
            };
            let Some(symbol) = evidence.implementation else {
                continue;
            };
            if self.mir.symbol_generics[symbol.index()].is_empty() {
                continue;
            }
            let key = (symbol, evidence.arguments.clone());
            self.mir.implementation_instances[reference.index()] =
                self.admit_instance(key, &mut indices, &mut canonical);
        }
        for index in 0..self.mir.hir.len() {
            if let Some(key) =
                self.instance_key(HirId(index as u32), &BTreeMap::new(), &mut canonical)
            {
                self.mir.reference_instances[index] =
                    self.admit_instance(key, &mut indices, &mut canonical);
            }
        }
        let mut next = 0;
        let mut next_type = 0;
        let mut check_templates = BTreeMap::<SymbolId, Vec<ConstructionCheck>>::new();
        let mut property_templates = BTreeMap::<SymbolId, Vec<PropertyRecord>>::new();
        for record in &self.mir.properties {
            if !record.concrete
                && let TypeConstructor::Nominal(symbol) = self.mir.types[record.owner.index()].constructor {
                property_templates.entry(symbol).or_default().push(record.clone());
            }
        }
        for check in &self.mir.construction_checks {
            if !check.concrete {
                if let TypeConstructor::Nominal(symbol) =
                    self.mir.types[check.owner.index()].constructor
                {
                    check_templates
                        .entry(symbol)
                        .or_default()
                        .push(check.clone());
                }
            }
        }
        loop {
            // Applied member skeletons can discover decorated types that do not
            // occur directly in source references (e.g. Envelope(Int).item).
            let previous_types = self.mir.types.len();
            self.materialize_layouts();
            if self.mir.type_layouts.len() != self.mir.types.len() {
                return;
            }
            for index in previous_types..self.mir.types.len() {
                let ty = &self.mir.types[index];
                canonical.insert(
                    (ty.constructor.clone(), ty.arguments.clone()),
                    TypeId(index as u32),
                );
            }
            while next_type < self.mir.types.len() {
                let owner = TypeId(next_type as u32);
                next_type += 1;
                let TypeConstructor::Nominal(symbol) = self.mir.types[owner.index()].constructor
                else {
                    continue;
                };
                if !check_templates.contains_key(&symbol) && !property_templates.contains_key(&symbol) {
                    continue;
                }
                if self.contains_parameter(owner) {
                    continue;
                }
                let definition = &self.mir.type_definitions
                    [self.nominal_index[symbol.index()].expect("decorated owner definition")];
                let arguments = definition
                    .parameters
                    .iter()
                    .copied()
                    .zip(self.mir.types[owner.index()].arguments.iter().copied())
                    .collect::<Vec<_>>();
                let Some(instance) =
                    self.admit_instance((symbol, arguments.clone()), &mut indices, &mut canonical)
                else {
                    continue;
                };
                let substitutions = arguments.into_iter().collect();
                for record in property_templates.get(&symbol).into_iter().flatten() {
                    let property = self.substitute_resolved(record.property, &substitutions, &mut canonical);
                    self.mir.properties.push(PropertyRecord {
                        owner,
                        property,
                        concrete: !self.contains_parameter(property),
                        instance: Some(instance),
                        ..record.clone()
                    });
                }
                for check in check_templates.get(&symbol).into_iter().flatten() {
                    let signature =
                        self.substitute_resolved(check.signature, &substitutions, &mut canonical);
                    self.mir.construction_checks.push(ConstructionCheck {
                        owner,
                        signature,
                        concrete: true,
                        instance: Some(instance),
                        ..check.clone()
                    });
                }
            }
            if next == self.mir.generic_instances.len() {
                // Property/check signatures may have appended types after the layout pass.
                if self.mir.type_layouts.len() == self.mir.types.len() {
                    break;
                }
                continue;
            }
            while next < self.mir.generic_instances.len() {
                let symbol = self.mir.generic_instances[next].symbol;
                let substitutions = self.mir.generic_instances[next]
                    .arguments
                    .iter()
                    .copied()
                    .collect();
                let mut pending = self.mir.symbols[symbol.index()].declarations.clone();
                let mut nodes = BTreeSet::new();
                while let Some(node) = pending.pop() {
                    if !nodes.insert(node) {
                        continue;
                    }
                    pending.extend(self.mir.hir[node.index()].children.iter().map(|e| e.node));
                }
                let mut types = vec![];
                let mut references = vec![];
                let mut implementations = vec![];
                let mut adjustments = vec![];
                let mut translated = BTreeMap::new();
                for node in nodes {
                    if matches!(self.mir.member_selections[node.index()], Some(MemberSelection::TraitMember { .. })) {
                        if let Some(template) = self.mir.bound_requirements.iter().find(|r| r.reference == node)
                            .and_then(|r| r.evidence).map(|id| (self.mir.evidence[id].subject, self.mir.evidence[id].bound))
                        {
                            let subject = self.substitute_resolved(template.0, &substitutions, &mut canonical);
                            let bound = self.substitute_resolved(template.1, &substitutions, &mut canonical);
                            if let Some((symbol, arguments)) = self.mir.evidence.iter().find(|e| e.subject == subject && e.bound == bound && e.state.is_proven())
                                .and_then(|e| e.implementation.map(|symbol| (symbol, e.arguments.clone())))
                                && let Some(instance) = self.admit_instance((symbol, arguments), &mut indices, &mut canonical)
                            {
                                implementations.push((node, instance));
                            } else if self.mir.generic_instances[next].concrete {
                                self.mir.diagnostics.push(Diagnostic::error("generic trait member has no closed implementation evidence", self.mir.hir[node.index()].location));
                            }
                        }
                    }
                    if let Some(slot) = self.mir.value_adjustments[node.index()] {
                        if let TypeState::Known(ty) = self.mir.ty_slots[slot.index()] {
                            adjustments.push((node, self.substitute_resolved(ty, &substitutions, &mut canonical)));
                        }
                    }
                    if let TypeState::Known(ty) = self.mir.ty_slots[node.ty().index()] {
                        let ty = *translated.entry(ty).or_insert_with(|| {
                            self.substitute_resolved(ty, &substitutions, &mut canonical)
                        });
                        types.push((node, ty));
                    }
                    if let Some(key) = self.instance_key(node, &substitutions, &mut canonical)
                        && let Some(instance) =
                            self.admit_instance(key, &mut indices, &mut canonical)
                    {
                        references.push((node, instance));
                    }
                }
                self.mir.generic_instances[next].types = types;
                self.mir.generic_instances[next].references = references;
                self.mir.generic_instances[next].implementations = implementations;
                self.mir.generic_instances[next].adjustments = adjustments;
                next += 1;
            }
        }
    }

    fn instance_key(
        &mut self,
        node: HirId,
        substitutions: &BTreeMap<SymbolId, TypeId>,
        canonical: &mut Canonical,
    ) -> Option<Key> {
        if self.mir.type_instances[node.index()].is_empty() {
            return None;
        }
        let slot = self.mir.hir[node.index()].resolution?;
        let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] else {
            return None;
        };
        let mut arguments = vec![];
        for (parameter, slot) in self.mir.type_instances[node.index()].clone() {
            let TypeState::Known(ty) = self.mir.ty_slots[slot.index()] else {
                // Keep the original Unknown/Conflicted outcome. It must prevent
                // publication, never become a downstream inference request.
                if self.mir.ty_slots[slot.index()] == TypeState::Unknown
                    && !self.mir.type_unknowns.contains(&slot)
                {
                    self.mir.type_unknowns.push(slot);
                    self.mir.diagnostics.push(Diagnostic::error(
                        "unknown generic argument",
                        self.mir.hir[node.index()].location,
                    ));
                }
                return None;
            };
            arguments.push((
                parameter,
                self.substitute_resolved(ty, substitutions, canonical),
            ));
        }
        let declaration = &self.mir.symbols[symbol.index()];
        if declaration.module.is_some_and(|module| declaration.scope != self.mir.module_scopes[module.index()]) {
            // Local instances also close captured enclosing binders. Preserve
            // these substitutions in the static instance key, never in codegen.
            for (&parameter, &ty) in substitutions {
                if !arguments.iter().any(|(existing, _)| *existing == parameter) {
                    arguments.push((parameter, ty));
                }
            }
            arguments.sort_by_key(|(parameter, _)| *parameter);
        }
        Some((symbol, arguments))
    }

    fn admit_instance(
        &mut self,
        key: Key,
        indices: &mut BTreeMap<Key, GenericInstanceId>,
        canonical: &mut Canonical,
    ) -> Option<GenericInstanceId> {
        if let Some(&id) = indices.get(&key) {
            return Some(id);
        }
        // Bound pathological polymorphic recursion just as the syntax/type
        // passes bound other compiler resources. Never publish a truncated graph.
        if indices.len() >= 4096 {
            if indices.len() == 4096
                && !self
                    .mir
                    .diagnostics
                    .iter()
                    .any(|d| d.message == "generic instance graph exceeds static expansion limit")
            {
                let node = self.mir.symbols[key.0.index()].declarations[0];
                self.mir.diagnostics.push(Diagnostic::error(
                    "generic instance graph exceeds static expansion limit",
                    self.mir.hir[node.index()].location,
                ));
            }
            return None;
        }
        let TypeState::Known(signature) =
            self.mir.ty_slots[self.mir.symbol_types[key.0.index()].index()]
        else {
            return None;
        };
        let signature =
            self.substitute_resolved(signature, &key.1.iter().copied().collect(), canonical);
        let id = GenericInstanceId(self.mir.generic_instances.len() as u32);
        let concrete = key.1.iter().all(|(_, ty)| !self.contains_parameter(*ty));
        self.mir.generic_instances.push(GenericInstance {
            symbol: key.0,
            concrete,
            arguments: key.1.clone(),
            signature,
            types: vec![],
            references: vec![],
            implementations: vec![],
            adjustments: vec![],
        });
        indices.insert(key, id);
        Some(id)
    }
}
