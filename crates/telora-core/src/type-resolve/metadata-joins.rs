use super::*;
use std::collections::BTreeSet;

impl Solver<'_> {
    /// Observe represented-type identity without equating evidence from
    /// different branches. Unknown children postpone the decision.
    fn same_represented_type(&self, left: TypeSlotId, right: TypeSlotId) -> Option<bool> {
        let mut pending = vec![(left, right)];
        let mut seen = BTreeSet::new();
        let mut unknown = false;
        while let Some((left, right)) = pending.pop() {
            let pair = (self.root(left), self.root(right));
            if pair.0 == pair.1 || !seen.insert(pair) { continue; }
            let (Some(left), Some(right)) = (self.term(pair.0), self.term(pair.1)) else {
                unknown = true;
                continue;
            };
            if left.constructor != right.constructor || left.arguments.len() != right.arguments.len() { return Some(false); }
            pending.extend(left.arguments.iter().copied().zip(right.arguments.iter().copied()));
        }
        (!unknown).then_some(true)
    }

    pub(super) fn metadata_join(&mut self, node: HirId, values: Vec<TypeSlotId>) -> Option<Task> {
        let mut represented = None;
        let mut broad = false;
        for &value in &values {
            if self.pending_blocks.get(value.index()).copied().unwrap_or(false) {
                return Some(Task::Join { node, values });
            }
            if matches!(self.mir.ty_slots[self.root(value).index()], TypeState::Conflicted(_)) {
                self.same(node, value);
                return None;
            }
            let Some(term) = self.term(value) else { return Some(Task::Join { node, values }); };
            match term.constructor {
                TypeConstructor::Never => {}
                TypeConstructor::Type => broad = true,
                TypeConstructor::TypeOf => {
                    let next = term.arguments[0];
                    if let Some(first) = represented {
                        match self.same_represented_type(first, next) {
                            Some(false) => broad = true,
                            None if !broad => return Some(Task::Join { node, values }),
                            _ => {}
                        }
                    } else { represented = Some(next); }
                }
                _ => {
                    self.conflict(node.ty(), value, Some(self.mir.hir[node.index()].location),
                        "metadata and ordinary values have no common branch type".into());
                    return None;
                }
            }
        }
        if broad {
            if self.term(node.ty()).is_some_and(|term| term.constructor == TypeConstructor::TypeOf) {
                self.conflict(node.ty(), node.ty(), Some(self.mir.hir[node.index()].location),
                    "metadata branches do not establish one TypeOf witness".into());
            } else { self.assign(node, TypeConstructor::Type, vec![]); }
        } else if let Some(represented) = represented {
            self.assign(node, TypeConstructor::TypeOf, vec![represented]);
        }
        None
    }
}
