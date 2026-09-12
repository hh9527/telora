//! Static admission of executable HIR and already selected instances.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExecutionRoot {
    pub node: HirId,
    pub instance: Option<GenericInstanceId>,
}

/// Deterministic dependency closure; all admitted nodes have concrete value types.
pub struct ExecutionClosure {
    nodes: Vec<ExecutionRoot>,
}

impl ExecutionClosure {
    pub fn nodes(&self) -> &[ExecutionRoot] { &self.nodes }
}

/// Publication capability for one entry and its closed initialization graph.
/// This owns the type image and borrows immutable MIR; backends only consume IDs.
pub struct SealedExecutable<'a> {
    sealed: SealedMir<'a>,
    root: HirId,
    globals: Vec<SymbolId>,
    instances: BTreeSet<GenericInstanceId>,
    properties: Vec<usize>,
    checks: Vec<usize>,
    closure: ExecutionClosure,
}

impl<'a> SealedExecutable<'a> {
    pub fn sealed_mir(&self) -> &SealedMir<'a> { &self.sealed }
    pub fn root(&self) -> HirId { self.root }
    pub fn globals(&self) -> &[SymbolId] { &self.globals }
    pub fn instances(&self) -> &BTreeSet<GenericInstanceId> { &self.instances }
    pub fn properties(&self) -> &[usize] { &self.properties }
    pub fn checks(&self) -> &[usize] { &self.checks }
    pub fn closure(&self) -> &ExecutionClosure { &self.closure }
}

impl Mir {
    pub fn seal_export(&self, export: SymbolId) -> Result<SealedExecutable<'_>, Vec<Diagnostic>> {
        self.seal()?.seal_export(export)
    }
}

impl<'a> SealedMir<'a> {
    /// Final publication after entry selection; no later pass may add instances.
    pub fn seal_export(self, export: SymbolId) -> Result<SealedExecutable<'a>, Vec<Diagnostic>> {
        let mir = self.mir();
        let failure = |message: &str| vec![Diagnostic { severity: crate::source::Severity::Error,
            message: message.into(), labels: vec![], notes: vec![] }];
        let symbol = mir.symbols.get(export.index()).ok_or_else(|| failure("execution export has no symbol"))?;
        let ResolveState::Bound(target) = symbol.resolution else { return Err(failure("execution export is not resolved")); };
        let node = *mir.symbols[target.index()].declarations.last().ok_or_else(|| failure("execution export has no declaration"))?;
        let mut roots = vec![ExecutionRoot { node, instance: None }];
        // Metadata initialization retains the session-wide semantics. Only
        // ordinary value exports are pruned by the selected entry at present.
        let properties = mir.properties.iter().enumerate().filter(|(_, property)| property.concrete).map(|(index, property)| {
            roots.extend(property.providers.iter().map(|&node| ExecutionRoot { node, instance: property.instance }));
            index
        }).collect();
        let checks = mir.construction_checks.iter().enumerate().filter(|(_, check)| check.concrete).map(|(index, check)| {
            roots.push(ExecutionRoot { node: check.checker, instance: check.instance });
            index
        }).collect();
        self.publish_execution(node, roots, properties, checks)
    }

    /// Module checking initializes every concrete declaration in its module set.
    pub fn seal_modules(self, modules: &[ModuleId]) -> Result<SealedExecutable<'a>, Vec<Diagnostic>> {
        let mir = self.mir();
        let modules = modules.iter().copied().collect::<BTreeSet<_>>();
        let mut globals = BTreeSet::new();
        for (index, symbol) in mir.symbols.iter().enumerate() {
            if matches!(symbol.kind, SymbolKind::Declaration(BindingKind::Let | BindingKind::Def | BindingKind::Decl | BindingKind::Native))
                && symbol.module.is_some_and(|module| modules.contains(&module)
                    && symbol.scope.is_some() && symbol.scope == mir.module_scopes[module.index()]) {
                globals.insert(SymbolId(index as u32));
            }
        }
        let mut roots = vec![];
        for &symbol in &globals {
            if mir.symbol_generics[symbol.index()].is_empty() {
                roots.extend(mir.symbols[symbol.index()].declarations.iter().map(|&node| ExecutionRoot { node, instance: None }));
            }
        }
        let Some(node) = roots.first().map(|root| root.node) else {
            return Err(vec![Diagnostic { severity: crate::source::Severity::Error,
                message: "module execution plan has no concrete root".into(), labels: vec![], notes: vec![] }]);
        };
        for (id, instance) in mir.generic_instances() {
            if instance.concrete && globals.contains(&instance.symbol) {
                roots.extend(mir.symbols[instance.symbol.index()].declarations.iter().map(|&node|
                    ExecutionRoot { node, instance: Some(id) }));
            }
        }
        let properties = mir.properties.iter().enumerate().filter(|(_, property)| property.concrete
            && property.providers.iter().any(|node| modules.contains(&mir.hir[node.index()].module))).map(|(index, property)| {
            roots.extend(property.providers.iter().map(|&node| ExecutionRoot { node, instance: property.instance }));
            index
        }).collect();
        let checks = mir.construction_checks.iter().enumerate().filter(|(_, check)| check.concrete
            && modules.contains(&mir.hir[check.checker.index()].module)).map(|(index, check)| {
            roots.push(ExecutionRoot { node: check.checker, instance: check.instance });
            index
        }).collect();
        self.publish_execution(node, roots, properties, checks)
    }

    fn publish_execution(self, node: HirId, roots: Vec<ExecutionRoot>, properties: Vec<usize>, checks: Vec<usize>) -> Result<SealedExecutable<'a>, Vec<Diagnostic>> {
        let mir = self.mir();
        let closure = self.execution_closure(&roots)?;
        let mut globals = BTreeSet::new();
        let mut instances = BTreeSet::new();
        for root in closure.nodes() {
            if let Some(instance) = root.instance { instances.insert(instance); }
            let Some(symbol) = mir.hir_symbols[root.node.index()] else { continue; };
            let definition = &mir.symbols[symbol.index()];
            if matches!(mir.hir[root.node.index()].kind, HirKind::Binding { .. })
                && definition.module.is_some_and(|module| definition.scope.is_some() && definition.scope == mir.module_scopes[module.index()]) {
                globals.insert(symbol);
            }
        }
        Ok(SealedExecutable { sealed: self, root: node, globals: globals.into_iter().collect(), instances, properties, checks, closure })
    }
}

impl SealedMir<'_> {
    /// Check executable roots without evaluating code or selecting new instances.
    /// Callers include their initialization/property roots as well as the entry.
    pub fn validate_execution_roots(&self, roots: &[ExecutionRoot]) -> Result<(), Vec<Diagnostic>> {
        self.execution_closure(roots).map(|_| ())
    }

    pub fn execution_closure(&self, roots: &[ExecutionRoot]) -> Result<ExecutionClosure, Vec<Diagnostic>> {
        let mir = self.mir();
        let mut pending = roots.to_vec();
        let mut seen = BTreeSet::new();
        let mut closed_types = BTreeSet::new();
        let mut diagnostics = vec![];
        while let Some(root) = pending.pop() {
            if !seen.insert(root) { continue; }
            let Some(node) = mir.hir.get(root.node.index()) else {
                diagnostics.push(Diagnostic { severity: crate::source::Severity::Error,
                    message: "execution root has no HIR node".into(), labels: vec![], notes: vec![] });
                continue;
            };
            let instance = match root.instance {
                Some(id) => match mir.generic_instances.get(id.index()).filter(|instance| instance.concrete) {
                    Some(instance) => Some(instance),
                    None => {
                        diagnostics.push(Diagnostic::error("execution requires a concrete generic instance", node.location));
                        continue;
                    }
                },
                None => None,
            };
            if mir.required_types[root.node.index()] {
                let ty = if let Some(instance) = instance { instance.ty(root.node) } else {
                    match mir.ty_slots[root.node.index()] { TypeState::Known(ty) => Some(ty), _ => None }
                };
                let closed = ty.is_some_and(|ty| {
                    if closed_types.contains(&ty) { return true; }
                    let mut types = vec![ty];
                    let mut visited = BTreeSet::new();
                    while let Some(ty) = types.pop() {
                        if closed_types.contains(&ty) || !visited.insert(ty) { continue; }
                        let Some(shape) = mir.types.get(ty.index()) else { return false; };
                        if matches!(shape.constructor, TypeConstructor::Parameter(_) | TypeConstructor::Bound(_) | TypeConstructor::Quantified(_)) {
                            return false;
                        }
                        types.extend(shape.arguments.iter().copied());
                        if let Some(layout) = mir.type_layouts.get(ty.index()).and_then(Option::as_ref) {
                            types.push(layout.body);
                            types.extend(layout.members.iter().flatten().copied());
                        }
                    }
                    closed_types.extend(visited);
                    true
                });
                if !closed {
                    diagnostics.push(Diagnostic::error("executable value requires a fully determined type", node.location));
                    continue;
                }
            }
            let reference = if let Some(instance) = instance { instance.reference(root.node) } else {
                mir.generic_references[root.node.index()].and_then(GenericReference::instance)
            }.or_else(|| if let Some(instance) = instance { instance.implementation(root.node) } else {
                mir.implementation_instances[root.node.index()]
            });
            if let Some(id) = reference {
                if let Some(selected) = mir.generic_instances.get(id.index()) {
                    for &node in &mir.symbols[selected.symbol.index()].declarations {
                        pending.push(ExecutionRoot { node, instance: Some(id) });
                    }
                } else {
                    diagnostics.push(Diagnostic::error("execution reference has no generic instance", node.location));
                }
            } else if let Some(slot) = node.resolution
                && let ResolveState::Bound(symbol) = mir.resolve_slots[slot.index()] {
                let symbol = &mir.symbols[symbol.index()];
                if symbol.module.is_some_and(|module| symbol.scope.is_some() && symbol.scope == mir.module_scopes[module.index()])
                    && matches!(symbol.kind, SymbolKind::Declaration(BindingKind::Let | BindingKind::Def | BindingKind::Native | BindingKind::Decl | BindingKind::Impl)) {
                    pending.extend(symbol.declarations.iter().map(|&node| ExecutionRoot { node, instance: None }));
                }
            }
            if let Some(MemberSelection::TraitMember { implementation: Some(symbol), .. }) = mir.member_selections[root.node.index()]
                && reference.is_none() {
                pending.extend(mir.symbols[symbol.index()].declarations.iter().map(|&node|
                    ExecutionRoot { node, instance: None }));
            }
            // These nodes contain static syntax, not child value computations.
            if matches!(node.kind, HirKind::TypeMetadata | HirKind::TypeOperation(_) | HirKind::TypeSyntax
                | HirKind::Binding { kind: BindingKind::Native | BindingKind::Decl, .. }) { continue; }
            if matches!(mir.member_selections[root.node.index()], Some(MemberSelection::NewtypeConstructor
                | MemberSelection::EnumVariant { .. } | MemberSelection::TraitMember { .. } | MemberSelection::Boolean(_))) { continue; }
            for edge in &node.children {
                if matches!(edge.role, Role::Annotation | Role::TypeParameter | Role::Bound | Role::ReturnType
                    | Role::Decorator | Role::Name | Role::Target)
                    || matches!(node.kind, HirKind::TypeApply) && edge.role != Role::Callee { continue; }
                // A template declaration is visited through its selected instances.
                if edge.role == Role::Binding && mir.hir_symbols[edge.node.index()]
                    .is_some_and(|symbol| !mir.symbol_generics[symbol.index()].is_empty()) { continue; }
                pending.push(ExecutionRoot { node: edge.node, instance: root.instance });
            }
        }
        if diagnostics.is_empty() { Ok(ExecutionClosure { nodes: seen.into_iter().collect() }) } else { Err(diagnostics) }
    }
}
