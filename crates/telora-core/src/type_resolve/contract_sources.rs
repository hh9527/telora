//! Declaration provenance belongs to a use edge, not a unified type identity.
use super::*;

impl Solver<'_> {
    pub(super) fn annotation_sources(mir: &Mir) -> Vec<Option<HirId>> {
        let symbols = mir
            .symbols
            .iter()
            .map(|symbol| {
                symbol.declarations.iter().find_map(|node| {
                    mir.hir[node.index()]
                        .children
                        .iter()
                        .find(|edge| edge.role == Role::Annotation)
                        .map(|edge| edge.node)
                })
            })
            .collect::<Vec<_>>();
        let mut sources = vec![None; mir.hir.len()];
        for (index, symbol) in mir.symbols.iter().enumerate() {
            for node in &symbol.declarations {
                sources[node.index()] = symbols[index];
            }
        }
        for (index, node) in mir.hir.iter().enumerate() {
            if let Some(slot) = node.resolution
                && let ResolveState::Bound(symbol) = mir.resolve_slots[slot.index()]
            {
                sources[index] = symbols[symbol.index()];
            }
        }
        sources
    }

    pub(super) fn contract_source(&self, mut node: HirId) -> Option<HirId> {
        loop {
            if let Some(source) = self.annotation_sources[node.index()] {
                return Some(source);
            }
            if let Some(annotation) = self.child(node, Role::Annotation) {
                return Some(annotation);
            }
            // Calls and explicit type applications consume the resolved callee,
            // including an imported/reexported declaration's original identity.
            node = self.child(node, Role::Callee)?;
        }
    }

    pub(super) fn fit_source(&mut self, node: HirId, actual: TypeSlotId) -> Option<HirId> {
        let contract = self
            .constraint_contract
            .or_else(|| self.contract_source(node));
        if let Some(contract) = contract
            && let Some(uses) = self.contract_uses.get_mut(actual.index())
            && !uses.contains(&contract)
        {
            // A deferred producer can reject expected evidence after the Fit
            // has run. Retain its use edge even if its type slot is unified.
            uses.push(contract);
        }
        contract
    }

    pub(super) fn failed_contract_sources(&self) -> Vec<HirId> {
        if let Some(contract) = self.constraint_contract {
            return vec![contract];
        }
        self.constraint_origin
            .map(|node| self.contract_uses[node.index()].clone())
            .unwrap_or_default()
    }
}
