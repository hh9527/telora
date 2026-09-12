//! Static admission of executable HIR and already selected instances.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExecutionRoot {
    pub node: HirId,
    pub instance: Option<GenericInstanceId>,
}

impl SealedMir<'_> {
    /// Check executable roots without evaluating code or selecting new instances.
    /// Callers include their initialization/property roots as well as the entry.
    pub fn validate_execution_roots(&self, roots: &[ExecutionRoot]) -> Result<(), Vec<Diagnostic>> {
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
        if diagnostics.is_empty() { Ok(()) } else { Err(diagnostics) }
    }
}
