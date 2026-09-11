//! Value-restricted quantification after ordinary use-site evidence settles.
//! A quantified function is a value contract, not an arbitrary monomorphic
//! choice for its otherwise unconstrained instance arguments.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

impl Solver<'_> {
    fn function_bound_key(&self, slot: TypeSlotId, parameters: &[TypeSlotId]) -> Option<Vec<(TypeConstructor, usize)>> {
        let mut key = vec![];
        let mut pending = vec![(slot, false)];
        let mut active = BTreeSet::new();
        while let Some((slot, leaving)) = pending.pop() {
            let slot = self.root(slot);
            if leaving { active.remove(&slot); continue; }
            if !active.insert(slot) { return None; }
            pending.push((slot, true));
            if let Some(index) = parameters.iter().position(|parameter| *parameter == slot) {
                key.push((TypeConstructor::Bound(index as u32), 0));
            } else {
                let term = self.term(slot)?;
                key.push((term.constructor.clone(), term.arguments.len()));
                pending.extend(term.arguments.iter().rev().map(|&child| (child, false)));
            }
        }
        Some(key)
    }

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
        let mut transferred = BTreeSet::new();
        for (root, (parameters, mut references)) in groups {
            let parameters = parameters.into_iter().map(|slot| self.root(slot)).collect::<Vec<_>>();
            if parameters.iter().any(|slot| self.mir.ty_slots[slot.index()] != TypeState::Unknown) { continue; }
            let Some(signature) = self.function_bound_key(root, &parameters) else { continue; };
            // Shape evidence can equate the children of two function terms
            // without unioning their roots. Quantify the entire equivalent
            // contract, including intermediate argument slots, at once.
            let roots = (0..self.mir.ty_slots.len()).map(|index| TypeSlotId(index as u32))
                .filter(|&slot| self.root(slot) == slot
                    && self.term(slot).is_some_and(|term| term.constructor == TypeConstructor::Function)
                    && self.unknown_leaves(slot).contains(&parameters[0])
                    && self.function_bound_key(slot, &parameters).as_ref() == Some(&signature))
                .collect::<BTreeSet<_>>();
            for index in 0..self.mir.hir.len() {
                let node = HirId(index as u32);
                if roots.contains(&self.root(node.ty())) && !self.mir.type_instances[index].is_empty()
                    && !references.contains(&node) {
                    references.push(node);
                }
            }
            let leaves = parameters.iter().copied().collect::<BTreeSet<_>>();
            let touches = |slot| self.unknown_leaves(slot).iter().any(|slot| leaves.contains(slot));
            // Pending operational constraints and missing bound evidence must
            // not become unconstrained binders merely to make a value close.
            if self.tasks.iter().any(|task| match task {
                Task::Numeric { operand, .. } | Task::Not { operand, .. } | Task::Ordered { operand, .. } => touches(*operand),
                Task::Member { receiver, .. } | Task::Projection { receiver, .. } | Task::FieldProjection { receiver, .. } => touches(*receiver),
                _ => false,
            }) { continue; }
            // Only declaration bounds belonging to these uninstantiated
            // references can move into the value contract. A constraint from
            // a call or a member use remains an obligation to prove.
            let mut bounds = BTreeMap::new();
            let mut reference_bounds = BTreeMap::<HirId, BTreeSet<_>>::new();
            let mut obligations = vec![];
            let mut blocked = false;
            for (index, requirement) in self.mir.bound_requirements.iter().enumerate() {
                if !touches(requirement.subject) && !touches(requirement.bound) { continue; }
                if !references.contains(&requirement.reference)
                    || !parameters.contains(&self.root(requirement.subject)) {
                    blocked = true; break;
                }
                let Some(subject) = self.function_bound_key(requirement.subject, &parameters) else { blocked = true; break; };
                let Some(bound) = self.function_bound_key(requirement.bound, &parameters) else { blocked = true; break; };
                let key = (subject, bound);
                reference_bounds.entry(requirement.reference).or_default().insert(key.clone());
                bounds.entry(key).or_insert((requirement.subject, requirement.bound));
                obligations.push(index);
            }
            if blocked || references.iter().any(|reference|
                reference_bounds.get(reference).map_or(0, BTreeSet::len) != bounds.len()) { continue; }
            let mut body_nodes = BTreeSet::new();
            for index in 0..self.mir.hir.len() {
                if matches!(self.mir.hir[index].kind, HirKind::Closure) && roots.contains(&self.root(TypeSlotId(index as u32))) {
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
                    if roots.contains(&slot) || !seen.insert(slot) { continue; }
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
            let mut contract = vec![body];
            for (_, (subject, bound)) in bounds {
                contract.push(self.structure(TypeConstructor::Tuple, vec![subject, bound]));
            }
            let quantified = self.structure(TypeConstructor::Quantified(parameters.len() as u32), contract);
            for root in roots { self.mir.ty_slots[root.index()] = TypeState::ProxyTo(quantified); }
            for node in references {
                self.mir.type_instances[node.index()].clear();
                self.scheme_references[node.index()] = true;
            }
            self.revision += 1;
            transferred.extend(obligations);
            changed = true;
        }
        let mut index = 0;
        self.mir.bound_requirements.retain(|_| {
            let keep = !transferred.contains(&index);
            index += 1;
            keep
        });
        changed
    }
}
