use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

impl Solver<'_> {
    pub(super) fn fresh(&mut self) -> TypeSlotId {
        let id = TypeSlotId(
            self.mir
                .ty_slots
                .len()
                .try_into()
                .expect("type slot capacity"),
        );
        self.mir.ty_slots.push(TypeState::Unknown);
        self.value_slots.push(false);
        self.contract_slots.push(false);
        id
    }
    pub(super) fn structure(
        &mut self,
        constructor: TypeConstructor,
        arguments: Vec<TypeSlotId>,
    ) -> TypeSlotId {
        if constructor == TypeConstructor::Function {
            for &argument in &arguments {
                let root = self.root(argument);
                if !self.value_slots[root.index()] {
                    self.value_slots[root.index()] = true;
                    self.revision += 1;
                }
            }
        }
        if constructor == TypeConstructor::Unchecked
            && arguments.len() == 1
            && self
                .term(arguments[0])
                .is_some_and(|term| term.constructor == TypeConstructor::Unchecked)
        {
            return arguments[0];
        }
        let slot = self.fresh();
        let term = TypeTermId(
            self.mir
                .type_terms
                .len()
                .try_into()
                .expect("type term capacity"),
        );
        self.mir.type_terms.push(TypeTerm {
            constructor,
            arguments,
        });
        self.mir.ty_slots[slot.index()] = TypeState::Structure(term);
        slot
    }
    pub(super) fn root(&self, mut slot: TypeSlotId) -> TypeSlotId {
        while let TypeState::ProxyTo(next) = self.mir.ty_slots[slot.index()] {
            slot = next;
        }
        slot
    }
    fn find(&mut self, mut slot: TypeSlotId) -> TypeSlotId {
        let root = self.root(slot);
        while let TypeState::ProxyTo(next) = self.mir.ty_slots[slot.index()] {
            self.mir.ty_slots[slot.index()] = TypeState::ProxyTo(root);
            slot = next;
        }
        root
    }
    pub(super) fn term(&self, slot: TypeSlotId) -> Option<&TypeTerm> {
        let TypeState::Structure(id) = self.mir.ty_slots[self.root(slot).index()] else {
            return None;
        };
        Some(&self.mir.type_terms[id.index()])
    }
    fn occurs(&self, needle: TypeSlotId, value: TypeSlotId) -> bool {
        let mut pending = vec![value];
        let mut visited = BTreeSet::new();
        while let Some(slot) = pending.pop() {
            let slot = self.root(slot);
            if slot == needle {
                return true;
            }
            if !visited.insert(slot) {
                continue;
            }
            if let Some(term) = self.term(slot) {
                pending.extend(&term.arguments);
            }
        }
        false
    }
    pub(super) fn conflict(
        &mut self,
        left: TypeSlotId,
        right: TypeSlotId,
        location: Option<Location>,
        message: String,
    ) {
        let left = self.find(left);
        let right = self.find(right);
        if let (TypeState::Conflicted(id), _) | (_, TypeState::Conflicted(id)) = (
            self.mir.ty_slots[left.index()],
            self.mir.ty_slots[right.index()],
        ) {
            if left == right {
                return;
            }
            let value_slot = self.value_slots[left.index()] || self.value_slots[right.index()];
            self.value_slots[left.index()] = value_slot;
            self.value_slots[right.index()] = value_slot;
            self.inherit_conflict(left, right, id);
            self.revision += 1;
            return;
        }
        let id = self.record_conflict(left, right, location, message);
        for slot in [left, right] {
            if !self.contract_slots[slot.index()] {
                self.mir.ty_slots[slot.index()] = TypeState::Conflicted(id);
            }
        }
        self.revision += 1;
    }

    fn inherit_conflict(&mut self, left: TypeSlotId, right: TypeSlotId, id: TypeConflictId) {
        for slot in [left, right] {
            if !self.contract_slots[slot.index()] {
                self.mir.ty_slots[slot.index()] = TypeState::Conflicted(id);
            }
        }
    }

    /// A failed relation does not invalidate either operand's type identity.
    /// Explicitly invalid inference nodes still use `conflict` above.
    fn record_conflict(
        &mut self,
        left: TypeSlotId,
        right: TypeSlotId,
        location: Option<Location>,
        message: String,
    ) -> TypeConflictId {
        let id = TypeConflictId(
            self.mir
                .type_conflicts
                .len()
                .try_into()
                .expect("type conflict capacity"),
        );
        let contracts = self.failed_contract_sources();
        self.mir.type_conflicts.push(TypeConflict {
            origin: self.constraint_origin,
            contracts: contracts.clone(),
            left,
            right,
            location,
            message: message.clone(),
            resolve_origin: None,
            diagnostic: Some(self.mir.diagnostics.len()),
        });
        if let Some(location) = location {
            self.mir
                .diagnostics
                .push(Diagnostic::error(message, location));
        } else {
            self.mir.diagnostics.push(Diagnostic {
                severity: crate::source::Severity::Error,
                message,
                labels: vec![],
                notes: vec![],
            });
        }
        for annotation in contracts {
            let location = self.mir.hir[annotation.index()].location;
            let diagnostic = self.mir.diagnostics.last_mut().unwrap();
            if !diagnostic
                .labels
                .iter()
                .any(|label| label.location == location)
            {
                diagnostic.labels.push(crate::source::Label {
                    location,
                    message: "type contract declared here".into(),
                    primary: false,
                });
            }
        }
        id
    }
    pub(super) fn equal(
        &mut self,
        left: TypeSlotId,
        right: TypeSlotId,
        location: Option<Location>,
    ) {
        let initial_conflicts = self.mir.type_conflicts.len();
        let mut structures = vec![];
        let mut queue = VecDeque::from([(left, right)]);
        while let Some((left, right)) = queue.pop_front() {
            let left = self.find(left);
            let right = self.find(right);
            if left == right {
                continue;
            }
            let value_slot = self.value_slots[left.index()] || self.value_slots[right.index()];
            self.value_slots[left.index()] = value_slot;
            self.value_slots[right.index()] = value_slot;
            let a = self.mir.ty_slots[left.index()];
            let b = self.mir.ty_slots[right.index()];
            match (a, b) {
                (TypeState::Conflicted(id), _) | (_, TypeState::Conflicted(id)) => {
                    // Inherit the existing failed evidence only into inference
                    // state. A declaration remains inspectable even when its
                    // implementation or a consumer already failed.
                    self.inherit_conflict(left, right, id);
                }
                (TypeState::Unknown, _) => {
                    if self.occurs(left, right) {
                        self.conflict(left, right, location, "infinite structural type".into());
                    } else {
                        self.mir.ty_slots[left.index()] = TypeState::ProxyTo(right);
                    }
                }
                (_, TypeState::Unknown) => {
                    if self.occurs(right, left) {
                        self.conflict(left, right, location, "infinite structural type".into());
                    } else {
                        self.mir.ty_slots[right.index()] = TypeState::ProxyTo(left);
                    }
                }
                (TypeState::Structure(a), TypeState::Structure(b)) => {
                    let a = &self.mir.type_terms[a.index()];
                    let b = &self.mir.type_terms[b.index()];
                    if a.constructor == TypeConstructor::ArrayLiteral
                        && b.constructor == TypeConstructor::ArrayLiteral
                    {
                        // Array literal children are element evidence, not
                        // positional type arguments, even at equal lengths.
                        self.compatible_structure(left, right, location, &mut queue);
                        continue;
                    }
                    if a.constructor != b.constructor || a.arguments.len() != b.arguments.len() {
                        if self.compatible_structure(left, right, location, &mut queue) {
                            continue;
                        }
                        let message = format!(
                            "type mismatch between {} and {}",
                            self.diagnostic_type(left),
                            self.diagnostic_type(right)
                        );
                        self.record_conflict(left, right, location, message);
                    } else {
                        queue.extend(a.arguments.iter().copied().zip(b.arguments.iter().copied()));
                        structures.push((left, right));
                    }
                }
                _ => unreachable!("only provisional states exist during solving"),
            }
            self.revision += 1;
        }
        // Only equal structures may share a representative. Merging parents
        // before comparing children erases the actual type on a failed fit.
        if self.mir.type_conflicts.len() == initial_conflicts {
            for (left, right) in structures {
                let left = self.find(left);
                let right = self.find(right);
                if left != right {
                    self.contract_slots[left.index()] |= self.contract_slots[right.index()];
                    self.mir.ty_slots[right.index()] = TypeState::ProxyTo(left);
                }
            }
        }
    }
    pub(super) fn finalize(&mut self) {
        // Normalize the provisional DAG into canonical TypeIds. Scans continue
        // only while a constructor becomes known or inherits a child conflict.
        let mut canonical = BTreeMap::<(TypeConstructor, Vec<TypeId>), TypeId>::new();
        loop {
            let mut changed = false;
            for index in 0..self.mir.ty_slots.len() {
                let slot = TypeSlotId(index as u32);
                let root = self.find(slot);
                if root != slot {
                    continue;
                }
                let TypeState::Structure(term) = self.mir.ty_slots[index] else {
                    continue;
                };
                let term = &self.mir.type_terms[term.index()];
                let mut arguments = Vec::with_capacity(term.arguments.len());
                let mut conflict = None;
                for &child in &term.arguments {
                    match self.mir.ty_slots[self.root(child).index()] {
                        TypeState::Known(id) => arguments.push(id),
                        TypeState::Conflicted(id) => {
                            conflict = Some(id);
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(id) = conflict {
                    self.mir.ty_slots[index] = TypeState::Conflicted(id);
                    changed = true;
                } else if arguments.len() == term.arguments.len() {
                    let key = (term.constructor.clone(), arguments);
                    let id = *canonical.entry(key.clone()).or_insert_with(|| {
                        let id = TypeId(self.mir.types.len().try_into().expect("type capacity"));
                        self.mir.types.push(ResolvedType {
                            constructor: key.0,
                            arguments: key.1,
                        });
                        id
                    });
                    self.mir.ty_slots[index] = TypeState::Known(id);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for index in 0..self.mir.ty_slots.len() {
            let root = self.find(TypeSlotId(index as u32));
            self.mir.ty_slots[index] = match self.mir.ty_slots[root.index()] {
                TypeState::Structure(_) => TypeState::Unknown,
                state => state,
            };
        }
        let mut reported_unknowns = self
            .mir
            .diagnostics
            .iter()
            .flat_map(|diagnostic| {
                diagnostic
                    .labels
                    .iter()
                    .filter(|label| label.primary)
                    .map(|label| label.location)
            })
            .collect::<BTreeSet<_>>();
        for (index, required) in self.mir.required_types.iter().enumerate() {
            if *required && self.mir.ty_slots[index] == TypeState::Unknown {
                self.mir.type_unknowns.push(TypeSlotId(index as u32));
                let location = self.mir.hir[index].location;
                if reported_unknowns.insert(location) {
                    self.mir
                        .diagnostics
                        .push(Diagnostic::error("unknown type", location));
                }
            }
        }
        for &slot in &self.mir.symbol_types {
            if self.mir.ty_slots[slot.index()] == TypeState::Unknown {
                self.mir.type_unknowns.push(slot);
            }
        }
    }
}
