use super::*;
use std::collections::BTreeSet;

pub(super) struct Candidate {
    nodes: Vec<HirId>,
    dependencies: Vec<SymbolId>,
    captures: Vec<SymbolId>,
}

impl Solver<'_> {
    fn non_expansive(&self, mut node: HirId) -> bool {
        loop {
            node = match self.mir.hir[node.index()].kind {
                HirKind::Closure | HirKind::Variable(_) => return true,
                HirKind::Field => self.child(node, Role::Receiver).unwrap(),
                HirKind::TypeApply => self.child(node, Role::Callee).unwrap(),
                _ => return false,
            };
        }
    }

    pub(super) fn prepare_generalization(&mut self) {
        let count = self.mir.symbols.len();
        self.generalizations = (0..count).map(|_| None).collect();
        for index in 0..count {
            let symbol = &self.mir.symbols[index];
            if !matches!(symbol.kind, SymbolKind::Declaration(BindingKind::Let | BindingKind::Def))
                || symbol.declarations.len() != 1 || !self.mir.symbol_generics[index].is_empty() {
                continue;
            }
            let declaration = symbol.declarations[0];
            if self.child(declaration, Role::Annotation).is_some() { continue; }
            let Some(value) = self.child(declaration, Role::Value) else { continue; };
            // Constructor imports lower to ordinary member-valued bindings.
            // These non-expansive aliases need the same independent use-site
            // instances as closure literals; sharing their holes makes the
            // first call monomorphize every later use.
            if !self.non_expansive(value) { continue; }
            let mut nodes = vec![];
            let mut pending = vec![value];
            while let Some(node) = pending.pop() {
                nodes.push(node);
                pending.extend(self.mir.hir[node.index()].children.iter().map(|edge| edge.node));
            }
            self.generalizations[index] = Some(Candidate { nodes, dependencies: vec![], captures: vec![] });
        }
        let mut edges = vec![vec![]; count];
        for index in 0..count {
            // Expansive bindings can connect recursive functions too, e.g.
            // a -> b -> {call: a}. Keep them in the dependency graph even
            // though they cannot introduce an implicit scheme themselves.
            let extra_nodes;
            let nodes = if let Some(candidate) = &self.generalizations[index] {
                &candidate.nodes
            } else {
                let symbol = &self.mir.symbols[index];
                if !matches!(symbol.kind, SymbolKind::Declaration(BindingKind::Let | BindingKind::Def)) {
                    continue;
                }
                let mut pending = symbol.declarations.iter()
                    .filter_map(|&node| self.child(node, Role::Value)).collect::<Vec<_>>();
                let mut nodes = vec![];
                while let Some(node) = pending.pop() {
                    nodes.push(node);
                    pending.extend(self.mir.hir[node.index()].children.iter().map(|edge| edge.node));
                }
                extra_nodes = nodes;
                &extra_nodes
            };
            let owned = nodes.iter().copied().collect::<BTreeSet<_>>();
            let mut captures = BTreeSet::new();
            let mut dependencies = BTreeSet::new();
            for &node in nodes {
                // Complete nested schemes before a containing signature can escape.
                if let Some(symbol) = self.mir.hir_symbols[node.index()]
                    && symbol.index() != index && self.generalizations[symbol.index()].is_some() {
                    dependencies.insert(symbol);
                }
                if let Some(slot) = self.mir.hir[node.index()].resolution
                    && let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] {
                    if matches!(self.mir.symbols[symbol.index()].kind,
                        SymbolKind::Declaration(BindingKind::Let | BindingKind::Def)) {
                        dependencies.insert(symbol);
                    }
                    if !self.mir.symbols[symbol.index()].declarations.iter().any(|node| owned.contains(node)) {
                        captures.insert(symbol);
                    }
                }
            }
            edges[index] = dependencies.iter().map(|symbol| symbol.index()).collect();
            if let Some(candidate) = self.generalizations[index].as_mut() {
                candidate.captures = captures.into_iter().collect();
            }
        }
        for index in recursive_nodes(&edges) {
            // Recursive components retain the original shared monomorphic slots.
            self.generalizations[index] = None;
        }
        for index in 0..count {
            if self.generalizations[index].is_none() { continue; }
            let mut pending = edges[index].clone();
            let mut seen = BTreeSet::new();
            let mut dependencies = vec![];
            while let Some(target) = pending.pop() {
                if !seen.insert(target) { continue; }
                if self.generalizations[target].is_some() {
                    dependencies.push(SymbolId(target as u32));
                } else {
                    pending.extend(edges[target].iter().copied());
                }
            }
            self.generalizations[index].as_mut().unwrap().dependencies = dependencies;
        }
    }

    pub(super) fn unknown_leaves(&self, slot: TypeSlotId) -> Vec<TypeSlotId> {
        let mut pending = vec![slot];
        let mut seen = BTreeSet::new();
        let mut result = vec![];
        while let Some(slot) = pending.pop() {
            let slot = self.root(slot);
            if !seen.insert(slot) { continue; }
            if let Some(term) = self.term(slot) {
                pending.extend(term.arguments.iter().rev().copied());
            } else if self.mir.ty_slots[slot.index()] == TypeState::Unknown {
                result.push(slot);
            }
        }
        result
    }

    pub(super) fn generalize_ready(&mut self) -> bool {
        let mut blocked = BTreeSet::new();
        let mut constrained = BTreeSet::new();
        // Pending derived results are not independent holes. Solver-only
        // operand constraints likewise cannot become unconstrained binders.
        for task in &self.tasks {
            match task {
                Task::Numeric { operand, .. } | Task::Not { operand, .. } | Task::Ordered { operand, .. } => constrained.extend(self.unknown_leaves(*operand)),
                Task::Member { receiver, .. } | Task::Projection { receiver, .. } | Task::FieldProjection { receiver, .. } => constrained.extend(self.unknown_leaves(*receiver)),
                _ => {}
            }
            let slots = match task {
                Task::Interpreter { node, .. } => vec![node.ty()],
                Task::Numeric { node, operand } | Task::Not { node, operand } | Task::Ordered { node, operand } => vec![node.ty(), *operand],
                Task::Member { node, receiver, .. } | Task::Projection { node, receiver, .. }
                    | Task::FieldProjection { node, receiver } => vec![node.ty(), *receiver],
                Task::Fit { expected, actual, .. } | Task::ShapeEqual { left: expected, right: actual, .. }
                    | Task::ValueEqual { left: expected, right: actual, .. } => vec![*expected, *actual],
                Task::Instantiate { target, .. } | Task::RefineInstance { target, .. } => vec![*target],
                Task::TypeFacet { node, source } => vec![node.ty(), *source],
                Task::Call { node, callee, .. } => vec![node.ty(), *callee],
                Task::BoundContext { node, subject, bound } => vec![node.ty(), *subject, *bound],
                Task::DiagnosticInput { node, input } => vec![node.ty(), *input],
                Task::Unchecked { node, argument } => vec![node.ty(), *argument],
                Task::ConstructorPattern { node, constructor, payload } => {
                    let mut slots = vec![node.ty(), *constructor]; slots.extend(payload); slots
                }
                Task::PropagationBottom { success, .. } => vec![*success],
                Task::Reference { node, .. } | Task::TypeApply { node } | Task::Propagate { node }
                    | Task::TupleSpread { node } | Task::RecordSpread { node } | Task::StructUpdate { node, .. }
                    | Task::Join { node, .. } | Task::Block { node, .. } | Task::Tuple { node, .. } => vec![node.ty()],
            };
            for slot in slots { blocked.extend(self.unknown_leaves(slot)); }
        }
        for requirement in &self.mir.bound_requirements {
            blocked.extend(self.unknown_leaves(requirement.subject));
            constrained.extend(self.unknown_leaves(requirement.subject));
        }
        let ready = self.generalizations.iter().enumerate().filter_map(|(index, candidate)| {
            candidate.as_ref().filter(|candidate| candidate.dependencies.iter()
                .all(|symbol| self.generalizations[symbol.index()].is_none())).map(|_| index)
        }).collect::<Vec<_>>();
        if ready.is_empty() { return false; }
        for index in ready {
            let candidate = self.generalizations[index].take().unwrap();
            if !self.term(self.mir.symbol_types[index]).is_some_and(|term| match term.constructor {
                TypeConstructor::Function | TypeConstructor::Option | TypeConstructor::Result | TypeConstructor::FoldControl => true,
                // A nominal enum value must establish its family identity.
                // Missing phantom arguments cannot become an implicit value
                // scheme just because the selected variant has no payload.
                TypeConstructor::Nominal(symbol) => self.nominal_index[symbol.index()]
                    .is_some_and(|definition| self.mir.type_definitions[definition].operation == TypeOperation::Enum)
                    && self.mir.symbols[index].declarations.iter().any(|node|
                        matches!(self.mir.hir[node.index()].kind, HirKind::Binding { imported: Some(_), .. })),
                _ => false,
            }) {
                continue;
            }
            let mut excluded = blocked.clone();
            let mut captured = BTreeSet::new();
            for symbol in candidate.captures {
                // A completed scheme's parameters are already rigid; captured
                // monomorphic unknowns remain owned by the surrounding scope.
                captured.extend(self.unknown_leaves(self.mir.symbol_types[symbol.index()]));
            }
            let mut leaves = self.unknown_leaves(self.mir.symbol_types[index]);
            let value = candidate.nodes[0];
            let mut ordered = self.mir.type_instances[value.index()].iter()
                .flat_map(|(_, slot)| self.unknown_leaves(*slot)).collect::<Vec<_>>();
            if ordered.is_empty()
                && matches!(self.mir.member_selections[value.index()], Some(MemberSelection::EnumVariant { .. })) {
                // A variant inherits its family's parameter order. In
                // particular Err's payload mentions E before T, but explicit
                // application still follows Result(T, E), not (E, T).
                let signature = self.term(self.mir.symbol_types[index]).unwrap();
                let owner = if signature.constructor == TypeConstructor::Function {
                    *signature.arguments.last().unwrap()
                } else { self.mir.symbol_types[index] };
                ordered = self.unknown_leaves(owner);
            }
            ordered.retain(|slot| leaves.contains(slot));
            for slot in leaves.drain(..) {
                if !ordered.contains(&slot) { ordered.push(slot); }
            }
            let leaves = ordered;
            if leaves.iter().any(|slot| constrained.contains(slot) && !captured.contains(slot)) { continue; }
            excluded.extend(captured);
            for slot in leaves {
                if excluded.contains(&slot) { continue; }
                let parameter = SymbolId(self.mir.symbols.len() as u32);
                let ordinal = self.mir.symbol_generics[index].len();
                self.mir.symbols.push(Symbol {
                    native_type: None, module: self.mir.symbols[index].module,
                    name: if ordinal < 26 { ((b'A' + ordinal as u8) as char).to_string() } else { format!("T{ordinal}") }, kind: SymbolKind::TypeParameter,
                    declarations: vec![], scope: None, resolution: ResolveState::Bound(parameter),
                });
                self.mir.symbol_generics[index].push(parameter);
                self.mir.symbol_generics.push(vec![]);
                self.nominal_index.push(None);
                self.generalizations.push(None);
                let raw = self.structure(TypeConstructor::Parameter(parameter), vec![]);
                let meta = self.structure(TypeConstructor::Meta, vec![raw]);
                self.mir.symbol_types.push(meta);
                self.equal(slot, raw, None);
            }
        }
        self.revision += 1;
        true
    }
}

fn recursive_nodes(edges: &[Vec<usize>]) -> Vec<usize> {
    let mut reverse = vec![vec![]; edges.len()];
    for (from, targets) in edges.iter().enumerate() {
        for &to in targets { reverse[to].push(from); }
    }
    let mut seen = vec![false; edges.len()];
    let mut order = vec![];
    for root in 0..edges.len() {
        let mut pending = vec![(root, false)];
        while let Some((node, finished)) = pending.pop() {
            if finished { order.push(node); continue; }
            if seen[node] { continue; }
            seen[node] = true;
            pending.push((node, true));
            pending.extend(edges[node].iter().map(|&next| (next, false)));
        }
    }
    seen.fill(false);
    let mut result = vec![];
    for root in order.into_iter().rev() {
        if seen[root] { continue; }
        let mut component = vec![];
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if seen[node] { continue; }
            seen[node] = true;
            component.push(node);
            pending.extend(reverse[node].iter().copied());
        }
        if component.len() > 1 || edges[root].contains(&root) { result.extend(component); }
    }
    result
}
