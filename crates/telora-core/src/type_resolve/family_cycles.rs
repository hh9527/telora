//! Detect unbounded nominal argument growth before materializing layouts.
//! A permutation is finite; a constructor-growing parameter edge in a cycle
//! is not. Constant substitutions break parameter flow and remain legal.
use super::*;
use std::collections::BTreeSet;

impl Solver<'_> {
    pub(super) fn reject_expanding_families(&mut self) {
        let mut edges = vec![vec![]; self.mir.symbols.len()];
        let mut growing = vec![];
        for definition in &self.mir.type_definitions {
            let parameters = definition.parameters.iter().copied().collect::<BTreeSet<_>>();
            if parameters.is_empty() { continue; }
            for member in &definition.members {
                let Some(slot) = member.payload else { continue; };
                let TypeState::Known(root) = self.mir.ty_slots[slot.index()] else { continue; };
                let mut pending = vec![root];
                let mut seen = BTreeSet::new();
                while let Some(ty) = pending.pop() {
                    if !seen.insert(ty) { continue; }
                    let source = &self.mir.types[ty.index()];
                    pending.extend(source.arguments.iter().copied());
                    let TypeConstructor::Nominal(target) = source.constructor else { continue; };
                    let Some(target) = self.nominal_index[target.index()] else { continue; };
                    for (&parameter, &argument) in self.mir.type_definitions[target].parameters.iter().zip(&source.arguments) {
                        let mut arguments = vec![(argument, false)];
                        let mut visited = BTreeSet::new();
                        while let Some((argument, grows)) = arguments.pop() {
                            if !visited.insert((argument, grows)) { continue; }
                            let argument = &self.mir.types[argument.index()];
                            if let TypeConstructor::Parameter(origin) = argument.constructor
                                && parameters.contains(&origin) {
                                edges[origin.index()].push(parameter.index());
                                if grows { growing.push((origin.index(), parameter.index(), slot, member.syntax)); }
                            }
                            // Unchecked(Unchecked(T)) normalizes to Unchecked(T),
                            // so that wrapper alone does not grow the graph.
                            let grows = grows || argument.constructor != TypeConstructor::Unchecked;
                            arguments.extend(argument.arguments.iter().map(|&child| (child, grows)));
                        }
                    }
                }
            }
        }
        let components = components(&edges);
        let mut reported = BTreeSet::new();
        for (from, to, slot, syntax) in growing {
            if components[from] == components[to] && reported.insert(slot) {
                self.conflict(slot, slot, Some(self.mir.hir[syntax.index()].location),
                    "recursive type family expands its type arguments without a finite layout graph".into());
            }
        }
    }
}

fn components(edges: &[Vec<usize>]) -> Vec<usize> {
    let mut reverse = vec![vec![]; edges.len()];
    for (from, targets) in edges.iter().enumerate() {
        for &to in targets { reverse[to].push(from); }
    }
    let mut visited = vec![false; edges.len()];
    let mut order = vec![];
    for root in 0..edges.len() {
        let mut pending = vec![(root, false)];
        while let Some((node, ready)) = pending.pop() {
            if ready { order.push(node); continue; }
            if visited[node] { continue; }
            visited[node] = true;
            pending.push((node, true));
            pending.extend(edges[node].iter().map(|&next| (next, false)));
        }
    }
    let mut components = vec![usize::MAX; edges.len()];
    for root in order.into_iter().rev() {
        if components[root] != usize::MAX { continue; }
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if components[node] != usize::MAX { continue; }
            components[node] = root;
            pending.extend(reverse[node].iter().copied());
        }
    }
    components
}
