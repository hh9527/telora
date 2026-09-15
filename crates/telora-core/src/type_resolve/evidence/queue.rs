//! Append-only evidence graph shared by source obligations and instances.
use super::*;
use std::collections::VecDeque;

impl Solver<'_> {
    pub(in crate::type_resolve) fn request_evidence(&mut self, subject: TypeId, bound: TypeId) -> usize {
        *self.evidence_indices.entry((subject, bound)).or_insert_with(|| {
            let id = self.mir.evidence.len();
            self.mir.evidence.push(EvidenceNode {
                subject, bound, state: BoundState::Pending, implementation: None,
                arguments: vec![], dependencies: vec![],
            });
            id
        })
    }

    pub(in crate::type_resolve) fn solve_pending_evidence(
        &mut self, canonical: &mut Canonical, origin: Option<Location>,
    ) -> bool {
        let start = self.evidence_cursor;
        while self.evidence_cursor < self.mir.evidence.len() {
            if !self.check_type_expansion(origin) {
                for node in &mut self.mir.evidence[start..] {
                    if node.state == BoundState::Pending { node.state = BoundState::Unresolved; }
                }
                return false;
            }
            let index = self.evidence_cursor;
            self.evidence_cursor += 1;
            let (subject, bound) = (self.mir.evidence[index].subject, self.mir.evidence[index].bound);
            if let Some(state) = self.direct_evidence(subject, bound) {
                self.mir.evidence[index].state = state;
                continue;
            }
            let Some(raw) = self.meta_type(bound) else {
                self.mir.evidence[index].state = BoundState::Rejected;
                continue;
            };
            let mut candidates = vec![];
            for implementation in &self.mir.trait_implementations {
                let mut substitutions = BTreeMap::new();
                if self.match_type(implementation.trait_type, raw, &mut substitutions) {
                    candidates.push((implementation.clone(), substitutions));
                }
            }
            if candidates.iter().any(|(implementation, _)| self.concrete_implementation(implementation)) {
                candidates.retain(|(implementation, _)| self.concrete_implementation(implementation));
            }
            if candidates.len() != 1 {
                self.mir.evidence[index].state = if candidates.is_empty() {
                    BoundState::Rejected
                } else { BoundState::Ambiguous };
                continue;
            }
            let (implementation, substitutions) = candidates.pop().unwrap();
            self.mir.evidence[index].implementation = Some(implementation.symbol);
            self.mir.evidence[index].arguments = substitutions.iter().map(|(&p, &t)| (p, t)).collect();
            for (parameter, bound) in implementation.requirements {
                let Some(&subject) = substitutions.get(&parameter) else {
                    self.mir.evidence[index].state = BoundState::Unresolved;
                    continue;
                };
                let bound = self.substitute_resolved(bound, &substitutions, canonical);
                let dependency = self.request_evidence(subject, bound);
                self.mir.evidence[index].dependencies.push(dependency);
            }
        }
        // A closed old node cannot acquire new dependencies. Propagate only
        // through this appended batch; cycles cannot establish their own proof.
        let count = self.mir.evidence.len() - start;
        let mut waiting = vec![0usize; count];
        let mut dependents = vec![vec![]; count];
        let mut ready = VecDeque::new();
        for index in start..self.mir.evidence.len() {
            let node = &self.mir.evidence[index];
            if node.state != BoundState::Pending { continue; }
            for &dependency in &node.dependencies {
                if !self.mir.evidence[dependency].state.is_proven() {
                    waiting[index - start] += 1;
                    if dependency >= start { dependents[dependency - start].push(index); }
                }
            }
            if waiting[index - start] == 0 { ready.push_back(index); }
        }
        while let Some(index) = ready.pop_front() {
            let Some(implementation) = self.mir.evidence[index].implementation else { continue; };
            self.mir.evidence[index].state = BoundState::Implementation(implementation);
            for &dependent in &dependents[index - start] {
                waiting[dependent - start] -= 1;
                if waiting[dependent - start] == 0 { ready.push_back(dependent); }
            }
        }
        for node in &mut self.mir.evidence[start..] {
            if node.state == BoundState::Pending { node.state = BoundState::Rejected; }
        }
        self.check_type_expansion(origin)
    }
}
