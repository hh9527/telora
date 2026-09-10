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
        for index in 0..self.mir.hir.len() {
            if let Some(key) =
                self.instance_key(HirId(index as u32), &BTreeMap::new(), &mut canonical)
            {
                self.mir.reference_instances[index] =
                    self.admit_instance(key, &mut indices, &mut canonical);
            }
        }
        let mut next = 0;
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
            let mut translated = BTreeMap::new();
            for node in nodes {
                if let TypeState::Known(ty) = self.mir.ty_slots[node.ty().index()] {
                    let ty = *translated.entry(ty).or_insert_with(|| {
                        self.substitute_resolved(ty, &substitutions, &mut canonical)
                    });
                    types.push((node, ty));
                }
                if let Some(key) = self.instance_key(node, &substitutions, &mut canonical)
                    && let Some(instance) = self.admit_instance(key, &mut indices, &mut canonical)
                {
                    references.push((node, instance));
                }
            }
            self.mir.generic_instances[next].types = types;
            self.mir.generic_instances[next].references = references;
            next += 1;
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
        self.mir.generic_instances.push(GenericInstance {
            symbol: key.0,
            arguments: key.1.clone(),
            signature,
            types: vec![],
            references: vec![],
        });
        indices.insert(key, id);
        Some(id)
    }
}
