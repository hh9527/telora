//! Finite, conservative parameter-flow proof before instance/evidence expansion.
//! An edge records the change in constructor depth of an originating parameter.
//! No positive-weight cycle means a finite upper bound on normalized type depth;
//! with a finite constructor vocabulary, only finitely many instance keys exist.
//! Pattern matching can remove constructors, so those edges have negative weight.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
struct Flow {
    from: usize,
    to: usize,
    depth: i64,
    location: Location,
}

impl Solver<'_> {
    fn parameter_depths(&self, ty: TypeId) -> Vec<(SymbolId, i64)> {
        let mut result = vec![];
        let mut pending = vec![(ty, 0)];
        let mut seen = BTreeSet::new();
        while let Some((ty, depth)) = pending.pop() {
            if !seen.insert((ty, depth)) { continue; }
            let ty = &self.mir.types[ty.index()];
            if let TypeConstructor::Parameter(parameter) = ty.constructor {
                result.push((parameter, depth));
            }
            // Repeated Unchecked wrappers collapse during canonicalization.
            let step = i64::from(ty.constructor != TypeConstructor::Unchecked);
            pending.extend(ty.arguments.iter().map(|&child| (child, depth + step)));
        }
        result
    }

    fn argument_flows(&self, target: SymbolId, argument: TypeId, location: Location, flows: &mut Vec<Flow>) {
        flows.extend(self.parameter_depths(argument).into_iter().map(|(from, depth)| Flow {
            from: from.index(), to: target.index(), depth, location,
        }));
    }

    /// Overapproximate every implementation that can match a symbolic obligation.
    /// We do not execute a trait selector or assume abstract parameters are concrete.
    fn implementation_flows(&self, pattern: TypeId, actual: TypeId, location: Location) -> Option<Vec<Flow>> {
        let fixed = instance_patterns::fixed_parameters(&self.mir.types, pattern, actual)?;
        let mut pending = vec![(pattern, actual)];
        let mut seen = BTreeSet::new();
        let mut flows = vec![];
        while let Some((pattern, actual)) = pending.pop() {
            if !seen.insert((pattern, actual)) { continue; }
            let a = &self.mir.types[actual.index()];
            let p = &self.mir.types[pattern.index()];
            if let TypeConstructor::Parameter(target) = p.constructor {
                self.argument_flows(target, actual, location, &mut flows);
            } else if let TypeConstructor::Parameter(from) = a.constructor {
                flows.extend(self.parameter_depths(pattern).into_iter().map(|(target, depth)| Flow {
                    from: from.index(), to: target.index(), depth: -depth, location,
                }));
            } else {
                if a.constructor != p.constructor || a.arguments.len() != p.arguments.len() { return None; }
                pending.extend(p.arguments.iter().copied().zip(a.arguments.iter().copied()));
            }
        }
        flows.retain(|edge| !fixed.contains(&SymbolId(edge.from as u32)));
        Some(flows)
    }

    pub(super) fn reject_nonconvergent_instances(&mut self) {
        let mut flows = vec![];
        // All reference substitutions are already solved. This includes local
        // generic declarations: enclosing binders retain their stable SymbolIds.
        for (index, arguments) in self.mir.type_instances.iter().enumerate() {
            let location = self.mir.hir[index].location;
            for &(parameter, slot) in arguments {
                if let TypeState::Known(ty) = self.mir.ty_slots[slot.index()] {
                    self.argument_flows(parameter, ty, location, &mut flows);
                }
            }
        }
        // Layouts and property/check signatures also discover nominal instances
        // without an explicit source-level reference to the applied type.
        for ty in &self.mir.types {
            let TypeConstructor::Nominal(symbol) = ty.constructor else { continue; };
            let Some(index) = self.nominal_index[symbol.index()] else { continue; };
            let definition = &self.mir.type_definitions[index];
            if definition.members.iter().any(|member| member.payload.is_some_and(|slot|
                !matches!(self.mir.ty_slots[slot.index()], TypeState::Known(_)))) { continue; }
            let location = self.mir.hir[self.mir.symbols[symbol.index()].declarations[0].index()].location;
            for (&parameter, &argument) in definition.parameters.iter().zip(&ty.arguments) {
                self.argument_flows(parameter, argument, location, &mut flows);
            }
        }
        let mut obligations = vec![];
        for requirement in &self.mir.bound_requirements {
            if let Some(bound) = self.known(requirement.bound).and_then(|ty| self.meta_type(ty)) {
                obligations.push((bound, self.mir.hir[requirement.reference.index()].location));
            }
        }
        for implementation in &self.mir.trait_implementations {
            let location = self.mir.hir[self.mir.symbols[implementation.symbol.index()].declarations[0].index()].location;
            for &(_, bound) in &implementation.requirements {
                if let Some(bound) = self.meta_type(bound) { obligations.push((bound, location)); }
            }
        }
        for (bound, location) in obligations {
            for implementation in &self.mir.trait_implementations {
                if let Some(edges) = self.implementation_flows(implementation.trait_type, bound, location) {
                    flows.extend(edges);
                }
            }
        }
        let mut edges = vec![vec![]; self.mir.symbols.len()];
        for edge in &flows { edges[edge.from].push(edge.to); }
        let components = family_cycles::components(&edges);
        let mut groups = BTreeMap::<usize, Vec<Flow>>::new();
        for edge in flows {
            if components[edge.from] == components[edge.to] {
                groups.entry(components[edge.from]).or_default().push(edge);
            }
        }
        // Components have disjoint vertices, so these arrays need allocating
        // only once; no per-component copy or reset of the symbol table.
        let mut distances = vec![0i64; self.mir.symbols.len()];
        let mut previous = vec![None; self.mir.symbols.len()];
        for flows in groups.values() {
            let vertices = flows.iter().flat_map(|edge| [edge.from, edge.to]).collect::<BTreeSet<_>>();
            let mut changed = None;
            // Bellman-Ford with maximum distances: an improvement after |V|
            // rounds witnesses a positive cycle in this finite abstraction.
            for _ in 0..vertices.len() {
                changed = None;
                for (index, edge) in flows.iter().enumerate() {
                    if distances[edge.to] < distances[edge.from] + edge.depth {
                        distances[edge.to] = distances[edge.from] + edge.depth;
                        previous[edge.to] = Some(index);
                        changed = Some(edge.to);
                    }
                }
                if changed.is_none() { break; }
            }
            let Some(mut node) = changed else { continue; };
            for _ in 0..vertices.len() { node = flows[previous[node].unwrap()].from; }
            let start = node;
            let mut cycle = vec![];
            loop {
                let edge = flows[previous[node].unwrap()];
                cycle.push(edge);
                node = edge.from;
                if node == start { break; }
            }
            let mut diagnostic = Diagnostic::error(
                "cannot establish finite generic instance expansion: recursive parameter flow can grow",
                cycle[0].location,
            );
            for edge in cycle.iter().rev() {
                diagnostic = diagnostic.with_secondary(format!("{} -> {}: constructor depth {:+}",
                    self.mir.symbols[edge.from].name, self.mir.symbols[edge.to].name, edge.depth), edge.location);
            }
            self.mir.diagnostics.push(diagnostic);
            for (index, parameters) in self.mir.symbol_generics.iter().enumerate() {
                if parameters.iter().any(|parameter| vertices.contains(&parameter.index())) {
                    self.nonconvergent_instances.insert(SymbolId(index as u32));
                }
            }
        }
    }
}
