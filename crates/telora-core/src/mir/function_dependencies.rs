//! Behavioral dependencies extracted from resolved, closed function bodies.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FunctionDependencyFallback {
    /// The body invokes a function-valued parameter. Concrete targets are
    /// propagated over the closed executable in a later analysis step.
    Parameter { symbol: SymbolId, signature: TypeId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FunctionBodyDependency {
    Function(FuncId),
    TopLevel(SymbolId),
    Property(PropertyId),
    Conservative(FunctionDependencyFallback),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedFunction {
    pub id: FuncId,
    pub prototype: FuncProtoId,
    pub root: ExecutionRoot,
    pub dependencies: Vec<FunctionBodyDependency>,
}

/// Deterministic behavioral graph. Data edges remain owned by value layouts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionBodyDependencyGraph {
    functions: Vec<SealedFunction>,
}

impl FunctionBodyDependencyGraph {
    pub fn functions(&self) -> &[SealedFunction] {
        &self.functions
    }

    pub fn function(&self, id: FuncId) -> Option<&SealedFunction> {
        self.functions.get(id.index())
    }

    /// Canonical, stable representation for tests and backend diagnostics.
    pub fn dump(&self) -> String {
        use std::fmt::Write;

        let mut output = String::new();
        for function in &self.functions {
            let _ = writeln!(
                output,
                "func {} proto {} root {} instance {}",
                function.id.index(),
                function.prototype.index(),
                function.root.node.index(),
                function
                    .root
                    .instance
                    .map_or_else(|| "-".to_owned(), |id| id.index().to_string())
            );
            for dependency in &function.dependencies {
                match dependency {
                    FunctionBodyDependency::Function(id) => {
                        let _ = writeln!(output, "  function {}", id.index());
                    }
                    FunctionBodyDependency::TopLevel(symbol) => {
                        let _ = writeln!(output, "  top-level {}", symbol.index());
                    }
                    FunctionBodyDependency::Property(property) => {
                        let _ = writeln!(output, "  property {}", property.index());
                    }
                    FunctionBodyDependency::Conservative(
                        FunctionDependencyFallback::Parameter { symbol, signature },
                    ) => {
                        let _ = writeln!(
                            output,
                            "  conservative parameter {} signature {}",
                            symbol.index(),
                            signature.index()
                        );
                    }
                }
            }
        }
        output
    }

    /// Sorted transitive behavioral closure, including `root` itself.
    pub fn reachable_functions(&self, root: FuncId) -> Vec<FuncId> {
        let mut pending = vec![root];
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(function) = self.function(id) else {
                continue;
            };
            pending.extend(function.dependencies.iter().filter_map(
                |dependency| match dependency {
                    FunctionBodyDependency::Function(target) => Some(*target),
                    _ => None,
                },
            ));
        }
        seen.into_iter().collect()
    }

    /// Sorted demand/property dependencies reachable through function bodies.
    pub fn reachable_state(
        &self,
        root: FuncId,
    ) -> (
        Vec<SymbolId>,
        Vec<PropertyId>,
        Vec<FunctionDependencyFallback>,
    ) {
        let mut globals = BTreeSet::new();
        let mut properties = BTreeSet::new();
        let mut conservative = BTreeSet::new();
        for id in self.reachable_functions(root) {
            let Some(function) = self.function(id) else {
                continue;
            };
            for dependency in &function.dependencies {
                match dependency {
                    FunctionBodyDependency::TopLevel(symbol) => {
                        globals.insert(*symbol);
                    }
                    FunctionBodyDependency::Property(property) => {
                        properties.insert(*property);
                    }
                    FunctionBodyDependency::Conservative(fallback) => {
                        conservative.insert(*fallback);
                    }
                    FunctionBodyDependency::Function(_) => {}
                }
            }
        }
        (
            globals.into_iter().collect(),
            properties.into_iter().collect(),
            conservative.into_iter().collect(),
        )
    }

    pub(crate) fn build(
        mir: &Mir,
        closure: &ExecutionClosure,
        globals: &BTreeSet<SymbolId>,
        properties: &[usize],
    ) -> Self {
        let roots = closure
            .nodes()
            .iter()
            .copied()
            .filter(|root| is_function_body(mir, root.node))
            .collect::<Vec<_>>();
        let ids = roots
            .iter()
            .enumerate()
            .map(|(index, root)| (*root, FuncId(index as u32)))
            .collect::<BTreeMap<_, _>>();
        let prototypes = roots
            .iter()
            .map(|root| root.node)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .enumerate()
            .map(|(index, node)| (node, FuncProtoId(index as u32)))
            .collect::<BTreeMap<_, _>>();
        let admitted_properties = properties.iter().copied().collect::<BTreeSet<_>>();
        let property_nodes = mir
            .bound_requirements
            .iter()
            .filter_map(|requirement| match requirement.state {
                BoundState::Property(index) if admitted_properties.contains(&index) => {
                    Some((requirement.reference, PropertyId(index as u32)))
                }
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let owners = function_owners(mir, &roots);
        let call_callees = mir
            .hir
            .iter()
            .filter(|node| matches!(node.kind, HirKind::Call))
            .flat_map(|node| {
                node.children
                    .iter()
                    .filter(|edge| edge.role == Role::Callee)
                    .map(|edge| edge.node)
            })
            .collect::<BTreeSet<_>>();
        let mut functions = Vec::with_capacity(roots.len());
        for (index, root) in roots.iter().copied().enumerate() {
            let mut dependencies = BTreeSet::new();
            let mut pending = body_roots(mir, root.node);
            let mut seen = BTreeSet::new();
            while let Some(node) = pending.pop() {
                if !seen.insert(node) {
                    continue;
                }
                if node != root.node && is_function_body(mir, node) {
                    if let Some(&target) = ids.get(&ExecutionRoot {
                        node,
                        instance: root.instance,
                    }) {
                        dependencies.insert(FunctionBodyDependency::Function(target));
                    }
                    continue;
                }
                if let Some(&property) = property_nodes.get(&node) {
                    dependencies.insert(FunctionBodyDependency::Property(property));
                }
                if let Some(symbol) = resolved_symbol(mir, node) {
                    if globals.contains(&symbol) {
                        dependencies.insert(FunctionBodyDependency::TopLevel(symbol));
                    }
                    if matches!(mir.symbols[symbol.index()].kind, SymbolKind::Parameter)
                        && call_callees.contains(&node)
                        && let Some(signature) = effective_type(mir, root.instance, node)
                        && mir.types[signature.index()].constructor == TypeConstructor::Function
                    {
                        dependencies.insert(FunctionBodyDependency::Conservative(
                            FunctionDependencyFallback::Parameter { symbol, signature },
                        ));
                    }
                    let selected = selected_instance(mir, root.instance, node);
                    for (target_root, &target) in &ids {
                        if owners.get(target_root).copied() == Some(symbol)
                            && (selected.is_none() || target_root.instance == selected)
                        {
                            dependencies.insert(FunctionBodyDependency::Function(target));
                        }
                    }
                }
                pending.extend(runtime_children(mir, node));
            }
            let fallback_signatures = dependencies
                .iter()
                .filter_map(|dependency| match dependency {
                    FunctionBodyDependency::Conservative(
                        FunctionDependencyFallback::Parameter { signature, .. },
                    ) => Some(*signature),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            for signature in fallback_signatures {
                for (target_root, &target) in &ids {
                    if effective_type(mir, target_root.instance, target_root.node)
                        == Some(signature)
                    {
                        dependencies.insert(FunctionBodyDependency::Function(target));
                    }
                }
            }
            functions.push(SealedFunction {
                id: FuncId(index as u32),
                prototype: prototypes[&root.node],
                root,
                dependencies: dependencies.into_iter().collect(),
            });
        }
        Self { functions }
    }
}

fn is_function_body(mir: &Mir, node: HirId) -> bool {
    matches!(
        mir.hir[node.index()].kind,
        HirKind::Closure | HirKind::Interpreter
    )
}

fn body_roots(mir: &Mir, node: HirId) -> Vec<HirId> {
    let body = mir.hir[node.index()]
        .children
        .iter()
        .find(|edge| edge.role == Role::Body)
        .map(|edge| edge.node);
    body.into_iter().collect()
}

fn runtime_children(mir: &Mir, node: HirId) -> impl Iterator<Item = HirId> + '_ {
    mir.hir[node.index()]
        .children
        .iter()
        .filter_map(move |edge| {
            (!matches!(
                edge.role,
                Role::Annotation
                    | Role::TypeParameter
                    | Role::Bound
                    | Role::ReturnType
                    | Role::Decorator
                    | Role::Name
                    | Role::Target
            ) && !(matches!(mir.hir[node.index()].kind, HirKind::TypeApply)
                && edge.role != Role::Callee))
                .then_some(edge.node)
        })
}

fn resolved_symbol(mir: &Mir, node: HirId) -> Option<SymbolId> {
    let slot = mir.hir[node.index()].resolution?;
    match mir.resolve_slots[slot.index()] {
        ResolveState::Bound(symbol) => Some(symbol),
        _ => None,
    }
}

fn selected_instance(
    mir: &Mir,
    context: Option<GenericInstanceId>,
    node: HirId,
) -> Option<GenericInstanceId> {
    context
        .and_then(|id| mir.generic_instances[id.index()].reference(node))
        .or_else(|| mir.generic_references[node.index()].and_then(GenericReference::instance))
        .or_else(|| {
            context
                .and_then(|id| mir.generic_instances[id.index()].implementation(node))
                .or(mir.implementation_instances[node.index()])
        })
}

fn effective_type(mir: &Mir, context: Option<GenericInstanceId>, node: HirId) -> Option<TypeId> {
    if let Some(instance) = context {
        mir.generic_instances[instance.index()].ty(node)
    } else {
        match mir.ty_slots[node.index()] {
            TypeState::Known(ty) => Some(ty),
            _ => None,
        }
    }
}

fn function_owners(mir: &Mir, roots: &[ExecutionRoot]) -> BTreeMap<ExecutionRoot, SymbolId> {
    let declarations = mir
        .symbols
        .iter()
        .enumerate()
        .flat_map(|(index, symbol)| {
            symbol
                .declarations
                .iter()
                .copied()
                .map(move |node| (node, SymbolId(index as u32)))
        })
        .collect::<BTreeMap<_, _>>();
    let mut parents = BTreeMap::<HirId, Vec<HirId>>::new();
    for (index, node) in mir.hir.iter().enumerate() {
        let parent = HirId(index as u32);
        for edge in &node.children {
            parents.entry(edge.node).or_default().push(parent);
        }
    }
    for values in parents.values_mut() {
        values.sort_unstable();
        values.dedup();
    }
    let mut result = BTreeMap::new();
    for &root in roots {
        let mut pending = vec![(root.node, 0usize)];
        let mut seen = BTreeSet::new();
        let mut nearest = None;
        let mut candidates = Vec::new();
        while let Some((node, distance)) = pending.pop() {
            if !seen.insert(node) || nearest.is_some_and(|nearest| distance > nearest) {
                continue;
            }
            if let Some(&symbol) = declarations.get(&node) {
                nearest = Some(distance);
                candidates.push(symbol);
                continue;
            }
            pending.extend(
                parents
                    .get(&node)
                    .into_iter()
                    .flatten()
                    .map(|&parent| (parent, distance + 1)),
            );
        }
        if let Some(symbol) = candidates.into_iter().min() {
            result.insert(root, symbol);
        }
    }
    result
}
