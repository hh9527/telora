//! Failed uses are separate from the type identities available for queries.
use super::*;

impl Mir {
    /// Failed type constraints owned by this syntax subtree. References do not
    /// inherit the failures of their targets: those retain their own origins.
    pub fn type_conflicts_in(&self, node: HirId) -> Vec<TypeConflictId> {
        let mut pending = vec![node];
        let mut nodes = std::collections::BTreeSet::new();
        while let Some(node) = pending.pop() {
            if !nodes.insert(node) {
                continue;
            }
            pending.extend(self.hir[node.index()].children.iter().map(|edge| edge.node));
        }
        self.type_conflicts
            .iter()
            .enumerate()
            .filter_map(|(index, conflict)| {
                conflict
                    .origin
                    .filter(|node| nodes.contains(node))
                    .map(|_| TypeConflictId(index as u32))
            })
            .collect()
    }
}
