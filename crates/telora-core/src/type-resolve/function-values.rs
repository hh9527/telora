//! Value-restricted quantification after ordinary use-site evidence settles.
//! A quantified function is a value contract, not an arbitrary monomorphic
//! choice for its otherwise unconstrained instance arguments.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

impl Solver<'_> {
    pub(super) fn generalize_function_values(&mut self) -> bool {
        let mut groups = BTreeMap::<TypeSlotId, (Vec<TypeSlotId>, Vec<HirId>)>::new();
        for index in 0..self.mir.hir.len() {
            let node = HirId(index as u32);
            let root = self.root(node.ty());
            if !self.term(root).is_some_and(|term| term.constructor == TypeConstructor::Function) { continue; }
            let leaves = self.unknown_leaves(root);
            if leaves.is_empty() { continue; }
            let arguments = &self.mir.type_instances[index];
            if !arguments.is_empty() {
                let parameters = arguments.iter().map(|(_, slot)| self.root(*slot)).collect::<Vec<_>>();
                if parameters.iter().any(|slot| self.mir.ty_slots[slot.index()] != TypeState::Unknown)
                    || leaves.iter().copied().collect::<BTreeSet<_>>() != parameters.iter().copied().collect() {
                    continue;
                }
                let group = groups.entry(root).or_insert_with(|| (parameters.clone(), vec![]));
                if group.0 == parameters { group.1.push(node); }
            } else if matches!(self.mir.hir[index].kind, HirKind::Closure) {
                groups.entry(root).or_insert_with(|| (leaves, vec![]));
            }
        }
        let mut changed = false;
        for (root, (parameters, references)) in groups {
            let parameters = parameters.into_iter().map(|slot| self.root(slot)).collect::<Vec<_>>();
            if parameters.iter().any(|slot| self.mir.ty_slots[slot.index()] != TypeState::Unknown) { continue; }
            let leaves = parameters.iter().copied().collect::<BTreeSet<_>>();
            let touches = |slot| self.unknown_leaves(slot).iter().any(|slot| leaves.contains(slot));
            // Pending operational constraints and missing bound evidence must
            // not become unconstrained binders merely to make a value close.
            if self.tasks.iter().any(|task| match task {
                Task::Numeric { operand, .. } | Task::Not { operand, .. } | Task::Ordered { operand, .. } => touches(*operand),
                Task::Member { receiver, .. } | Task::Projection { receiver, .. } | Task::FieldProjection { receiver, .. } => touches(*receiver),
                _ => false,
            }) || self.mir.bound_requirements.iter().any(|r| touches(r.subject) || touches(r.bound)) { continue; }
            let mut body_nodes = BTreeSet::new();
            for index in 0..self.mir.hir.len() {
                if matches!(self.mir.hir[index].kind, HirKind::Closure) && self.root(TypeSlotId(index as u32)) == root {
                    let mut pending = vec![HirId(index as u32)];
                    while let Some(node) = pending.pop() {
                        if !body_nodes.insert(node) { continue; }
                        pending.extend(self.mir.hir[node.index()].children.iter().map(|edge| edge.node));
                    }
                }
            }
            // A binder may occur in this function's body and contract. It may
            // not escape as an unknown result of a call or an outer capture.
            let escapes = self.mir.required_types.iter().enumerate().any(|(index, required)| {
                if !required || body_nodes.contains(&HirId(index as u32)) { return false; }
                let mut pending = vec![TypeSlotId(index as u32)];
                let mut seen = BTreeSet::new();
                while let Some(slot) = pending.pop() {
                    let slot = self.root(slot);
                    if slot == root || !seen.insert(slot) { continue; }
                    if leaves.contains(&slot) { return true; }
                    if let Some(term) = self.term(slot) { pending.extend(term.arguments.iter().copied()); }
                }
                false
            });
            if escapes { continue; }
            let term = self.term(root).unwrap().clone();
            for (index, &parameter) in parameters.iter().enumerate() {
                let bound = self.structure(TypeConstructor::Bound(index as u32), vec![]);
                self.equal(parameter, bound, None);
            }
            let body = self.structure(TypeConstructor::Function, term.arguments);
            let quantified = self.structure(TypeConstructor::Quantified(parameters.len() as u32), vec![body]);
            self.mir.ty_slots[root.index()] = TypeState::ProxyTo(quantified);
            for node in references {
                self.mir.type_instances[node.index()].clear();
                self.scheme_references[node.index()] = true;
            }
            self.revision += 1;
            changed = true;
        }
        changed
    }
}
