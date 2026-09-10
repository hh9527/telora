use super::*;

impl Solver<'_> {
    /// Transparent aliases must have a finite expansion before generic
    /// substitution starts. Nominal definitions are boundaries, not aliases.
    pub(super) fn reject_alias_cycles(&mut self) {
        let count = self.mir.symbols.len();
        let aliases = self.mir.symbols.iter().enumerate().map(|(index, symbol)| {
            symbol.kind == SymbolKind::Declaration(BindingKind::Type)
                && self.nominal_index[index].is_none()
        }).collect::<Vec<_>>();
        let mut edges = vec![vec![]; count];
        let mut reverse = vec![vec![]; count];
        for index in 0..count {
            if !aliases[index] { continue; }
            for &declaration in &self.mir.symbols[index].declarations {
                let Some(value) = self.child(declaration, Role::Value) else { continue; };
                let mut pending = vec![value];
                while let Some(node) = pending.pop() {
                    let node = &self.mir.hir[node.index()];
                    if let Some(slot) = node.resolution
                        && let ResolveState::Bound(target) = self.mir.resolve_slots[slot.index()]
                        && aliases[target.index()] {
                            edges[index].push(target.index());
                            reverse[target.index()].push(index);
                        }
                    pending.extend(node.children.iter().map(|edge| edge.node));
                }
            }
        }
        // Iterative SCC traversal keeps stack use independent of alias depth.
        let mut seen = vec![false; count];
        let mut order = vec![];
        for root in 0..count {
            if !aliases[root] || seen[root] { continue; }
            let mut stack = vec![(root, false)];
            while let Some((node, finished)) = stack.pop() {
                if finished { order.push(node); continue; }
                if seen[node] { continue; }
                seen[node] = true;
                stack.push((node, true));
                stack.extend(edges[node].iter().filter(|&&next| !seen[next]).map(|&next| (next, false)));
            }
        }
        seen.fill(false);
        for root in order.into_iter().rev() {
            if seen[root] { continue; }
            let mut component = vec![];
            let mut stack = vec![root];
            seen[root] = true;
            while let Some(node) = stack.pop() {
                component.push(node);
                for &next in &reverse[node] {
                    if !seen[next] { seen[next] = true; stack.push(next); }
                }
            }
            if component.len() == 1 && !edges[root].contains(&root) { continue; }
            for index in component {
                let slot = self.mir.symbol_types[index];
                let location = self.mir.symbols[index].declarations.first()
                    .map(|node| self.mir.hir[node.index()].location);
                self.conflict(slot, slot, location, "recursive type alias component".into());
            }
        }
    }
}
