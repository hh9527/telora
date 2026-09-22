//! Second MIR pass: syntax-only declaration indexing and reference closure.
use crate::mir::*;
use crate::source::Diagnostic;
use crate::syntax::kinds::{BindingKind, DeclaredInitializerKind};
use std::collections::BTreeMap;
mod closure;
mod queries;
mod schedule;
use schedule::Scheduler;

#[cfg(test)]
mod tests;

/// All names come from declarations and imports, including the prelude.
pub fn resolve(mir: &mut Mir) {
    resolve_with_scheduler(mir, Scheduler::default());
}

fn resolve_with_scheduler(mir: &mut Mir, scheduler: Scheduler) {
    assert!(
        !mir.symbols_closed && mir.symbols.is_empty(),
        "symbol pass runs once"
    );
    mir.hir_scopes.resize(mir.hir.len(), None);
    mir.hir_symbols.resize(mir.hir.len(), None);
    mir.module_scopes.resize(mir.modules.len(), None);
    mir.exports.resize_with(mir.modules.len(), Vec::new);
    let mut pass = Pass {
        mir,
        scheduler,
        import_edges: BTreeMap::new(),
    };
    for (id, edge) in pass.mir.imports.iter().enumerate() {
        if let Some(syntax) = edge.syntax {
            pass.import_edges.insert(syntax, id);
        }
    }
    // All providers are indexed before any consumer is resolved.
    for index in 0..pass.mir.modules.len() {
        let module = ModuleId(index as u32);
        match pass.mir.modules[index].state {
            ModuleState::Source { body, .. } | ModuleState::Data { body } => {
                let scope = pass.scope(module, None);
                pass.mir.module_scopes[index] = Some(scope);
                pass.index_block(body, scope);
                let implicit = pass.mir.modules[index]
                    .imports
                    .iter()
                    .copied()
                    .filter(|edge| pass.mir.imports[*edge].syntax.is_none())
                    .collect::<Vec<_>>();
                pass.mir.scopes[scope.index()].open_imports.extend(implicit);
                pass.index_exports(module, body);
            }
            _ => {}
        }
    }
    pass.link_native_types();
    pass.diagnose_duplicates(false);
    pass.mir
        .resolution_facts
        .namespaces
        .resize(pass.mir.symbols.len(), None);
    pass.mir
        .resolution_facts
        .constructors
        .resize(pass.mir.symbols.len(), None);
    pass.mir
        .resolution_facts
        .constructor_namespaces
        .resize(pass.mir.hir.len(), None);
    for id in 0..pass.mir.symbols.len() {
        pass.scheduler
            .enqueue(ResolveTask::Symbol(SymbolId(id as u32)));
    }
    pass.drain();
    for id in 0..pass.mir.symbols.len() {
        let state = pass.mir.symbols[id].resolution.clone();
        let symbol = &pass.mir.symbols[id];
        if state == ResolveState::Unresolved
            && matches!(symbol.kind, SymbolKind::Import | SymbolKind::Export)
        {
            let declaration = symbol.declarations[0];
            let location = pass.mir.hir[declaration.index()].location;
            let message = if symbol.kind == SymbolKind::Import {
                let imported = match &pass.mir.hir[declaration.index()].kind {
                    HirKind::Binding {
                        imported: Some(name),
                        ..
                    } => name,
                    _ => &symbol.name,
                };
                match pass.import_edges.get(&declaration) {
                    Some(edge) => format!(
                        "unknown imported binding {imported:?} from {:?}",
                        pass.mir.imports[*edge].request
                    ),
                    None => format!("unknown imported binding {imported:?}"),
                }
            } else {
                format!("unknown exported binding {:?}", symbol.name)
            };
            pass.mir
                .diagnostics
                .push(Diagnostic::error(message, location));
        }
    }
    pass.diagnose_duplicates(true);
    for id in 0..pass.mir.hir.len() {
        if pass.mir.hir[id].resolution.is_some() {
            pass.scheduler
                .enqueue(ResolveTask::Reference(HirId(id as u32)));
        }
    }
    pass.drain();
    for node in &pass.mir.hir {
        if let Some(slot) = node.resolution {
            assert_ne!(pass.mir.resolve_slots[slot.index()], ResolveState::Pending);
            if pass.mir.resolve_slots[slot.index()] == ResolveState::Unresolved {
                let name = match &node.kind {
                    HirKind::Variable(name) | HirKind::PatternName(name) | HirKind::Name(name) => {
                        Some(name.as_str())
                    }
                    HirKind::StaticPath(path) => path.last().map(String::as_str),
                    HirKind::Field => node
                        .children
                        .iter()
                        .find(|edge| edge.role == Role::Name)
                        .and_then(|edge| match &pass.mir.hir[edge.node.index()].kind {
                            HirKind::Name(name) => Some(name.as_str()),
                            _ => None,
                        }),
                    _ => None,
                };
                if let Some(name) = name {
                    pass.mir.diagnostics.push(Diagnostic::error(
                        format!("unknown binding {name:?}"),
                        node.location,
                    ));
                }
            }
        }
    }
    assert!(
        pass.mir
            .symbols
            .iter()
            .all(|symbol| symbol.resolution != ResolveState::Pending)
    );
    pass.mir.symbols_closed = true;
    // Discovery timing is an implementation detail; presentation is ordered
    // by source evidence before the type pass attaches diagnostic indices.
    pass.mir.diagnostics.sort_by(|a, b| {
        a.labels
            .first()
            .map(|label| label.location)
            .cmp(&b.labels.first().map(|label| label.location))
            .then_with(|| a.message.cmp(&b.message))
    });
    pass.mir.diagnostics.dedup();
}

struct Pass<'a> {
    mir: &'a mut Mir,
    scheduler: Scheduler,
    import_edges: BTreeMap<HirId, usize>,
}
#[derive(Clone, Copy)]
enum IndexPass {
    Block,
    Node,
}
type IndexTask = (HirId, ScopeId, IndexPass);

impl Pass<'_> {
    fn link_native_types(&mut self) {
        let mut slots = BTreeMap::<NativeTypeId, Vec<SymbolId>>::new();
        for index in 0..self.mir.symbols.len() {
            let symbol = &self.mir.symbols[index];
            if symbol.kind != SymbolKind::Declaration(BindingKind::NativeType) {
                continue;
            }
            let module = symbol.module.expect("native declaration module");
            let declaration = symbol.declarations[0];
            let value = self.child(declaration, Role::Value).expect("native slot");
            let HirKind::NativeTypeSlot(slot) = self.mir.hir[value.index()].kind else {
                unreachable!()
            };
            let registered = u32::try_from(slot).ok().and_then(|slot| {
                let native = self.mir.modules[module.index()].native.as_ref()?;
                native
                    .types
                    .iter()
                    .any(|(s, _)| *s == slot)
                    .then_some(NativeTypeId {
                        module: native.id,
                        slot,
                    })
            });
            if let Some(id) = registered {
                self.mir.symbols[index].native_type = Some(id);
                slots.entry(id).or_default().push(SymbolId(index as u32));
            } else {
                self.mir.symbols[index].resolution = ResolveState::Unresolved;
                self.mir.diagnostics.push(Diagnostic::error(
                    "native type slot has no registered static contract",
                    self.mir.hir[value.index()].location,
                ));
            }
        }
        for (native, symbols) in slots {
            if symbols.len() < 2 {
                continue;
            }
            let conflict = ConflictId(self.mir.resolve_conflicts.len() as u32);
            self.mir
                .resolve_conflicts
                .push(ResolveConflict::DuplicateDefinition {
                    name: format!("native slot {native:?}"),
                    definitions: symbols.clone(),
                });
            for symbol in symbols {
                self.mir.symbols[symbol.index()].resolution = ResolveState::Conflicted(conflict);
                let declaration = self.mir.symbols[symbol.index()].declarations[0];
                self.mir.diagnostics.push(Diagnostic::error(
                    "duplicate native type slot",
                    self.mir.hir[declaration.index()].location,
                ));
            }
        }
    }
    fn child(&self, node: HirId, role: Role) -> Option<HirId> {
        self.mir.hir[node.index()]
            .children
            .iter()
            .find(|edge| edge.role == role)
            .map(|edge| edge.node)
    }
    fn name(&self, node: HirId) -> String {
        let node = self.child(node, Role::Name).unwrap_or(node);
        match &self.mir.hir[node.index()].kind {
            HirKind::Name(name) | HirKind::PatternName(name) | HirKind::Variable(name) => {
                name.clone()
            }
            _ => panic!("name-bearing HIR node"),
        }
    }
    fn scope(&mut self, module: ModuleId, parent: Option<ScopeId>) -> ScopeId {
        let id = ScopeId(self.mir.scopes.len().try_into().expect("scope capacity"));
        self.mir.scopes.push(Scope {
            parent,
            module,
            bindings: vec![],
            open_imports: vec![],
        });
        id
    }
    fn symbol(
        &mut self,
        module: Option<ModuleId>,
        name: String,
        kind: SymbolKind,
        node: Option<HirId>,
        scope: Option<ScopeId>,
    ) -> SymbolId {
        let id = SymbolId(self.mir.symbols.len().try_into().expect("symbol capacity"));
        let resolution = if matches!(
            kind,
            SymbolKind::Import | SymbolKind::Export | SymbolKind::Pattern
        ) {
            ResolveState::Pending
        } else {
            ResolveState::Bound(id)
        };
        self.mir.symbols.push(Symbol {
            native_type: None,
            module,
            name,
            kind,
            declarations: node.into_iter().collect(),
            scope,
            resolution,
        });
        id
    }
    fn declare(
        &mut self,
        node: HirId,
        kind: SymbolKind,
        scope: ScopeId,
        after: Option<HirId>,
    ) -> SymbolId {
        let module = self.mir.hir[node.index()].module;
        let name = self.name(node);
        if kind == SymbolKind::Declaration(BindingKind::Def) {
            let existing = self.mir.scopes[scope.index()]
                .bindings
                .iter()
                .find_map(|entry| {
                    let symbol = &self.mir.symbols[entry.symbol.index()];
                    (symbol.name == name
                        && symbol.kind == SymbolKind::Declaration(BindingKind::Decl)
                        && symbol.declarations.len() == 1)
                        .then_some(entry.symbol)
                });
            if let Some(id) = existing {
                self.mir.symbols[id.index()].declarations.push(node);
                self.mir.hir_symbols[node.index()] = Some(id);
                return id;
            }
        }
        let id = self.symbol(Some(module), name, kind, Some(node), Some(scope));
        self.mir.scopes[scope.index()]
            .bindings
            .push(ScopeBinding { symbol: id, after });
        self.mir.hir_symbols[node.index()] = Some(id);
        id
    }
    fn index_block(&mut self, node: HirId, scope: ScopeId) {
        let mut pending = vec![(node, scope, IndexPass::Block)];
        while let Some((node, scope, pass)) = pending.pop() {
            match pass {
                IndexPass::Block => self.index_block_node(node, scope, &mut pending),
                IndexPass::Node => self.index(node, scope, &mut pending),
            }
        }
    }
    fn index_block_node(&mut self, node: HirId, scope: ScopeId, pending: &mut Vec<IndexTask>) {
        self.mir.hir_scopes[node.index()] = Some(scope);
        let edges = self.mir.hir[node.index()].children.clone();
        for edge in &edges {
            if edge.role != Role::Binding {
                continue;
            }
            let HirKind::Binding { kind, .. } = self.mir.hir[edge.node.index()].kind else {
                unreachable!()
            };
            match kind {
                BindingKind::Export => {}
                kind => {
                    self.declare(
                        edge.node,
                        if kind == BindingKind::Import {
                            SymbolKind::Import
                        } else {
                            SymbolKind::Declaration(kind)
                        },
                        scope,
                        (kind == BindingKind::Let).then_some(edge.node),
                    );
                }
            }
        }
        for edge in edges.into_iter().rev() {
            pending.push((edge.node, scope, IndexPass::Node));
        }
    }
    fn index(&mut self, node: HirId, scope: ScopeId, pending: &mut Vec<IndexTask>) {
        self.mir.hir_scopes[node.index()] = Some(scope);
        let edges = self.mir.hir[node.index()].children.clone();
        let module = self.mir.hir[node.index()].module;
        match self.mir.hir[node.index()].kind {
            HirKind::Block => {
                let child = self.scope(module, Some(scope));
                pending.push((node, child, IndexPass::Block));
            }
            HirKind::Binding { .. } | HirKind::Closure => {
                let nested = if edges
                    .iter()
                    .any(|edge| matches!(edge.role, Role::TypeParameter | Role::Parameter))
                {
                    let nested = self.scope(module, Some(scope));
                    for edge in &edges {
                        let kind = match edge.role {
                            Role::TypeParameter => SymbolKind::TypeParameter,
                            Role::Parameter => SymbolKind::Parameter,
                            _ => continue,
                        };
                        self.declare(edge.node, kind, nested, None);
                    }
                    nested
                } else {
                    scope
                };
                for edge in edges.into_iter().rev() {
                    pending.push((edge.node, nested, IndexPass::Node));
                }
            }
            HirKind::IfLet | HirKind::LetElse | HirKind::MatchArm { .. } => {
                let nested = self.scope(module, Some(scope));
                for edge in edges.into_iter().rev() {
                    let target =
                        if matches!(
                            edge.role,
                            Role::Pattern | Role::Then | Role::Body | Role::Guard
                        ) || matches!(self.mir.hir[node.index()].kind, HirKind::MatchArm { .. })
                        {
                            nested
                        } else {
                            scope
                        };
                    pending.push((edge.node, target, IndexPass::Node));
                }
            }
            HirKind::PatternName(_) => {
                self.declare(node, SymbolKind::Pattern, scope, None);
            }
            _ => {
                for edge in edges.into_iter().rev() {
                    pending.push((edge.node, scope, IndexPass::Node));
                }
            }
        }
    }
    fn index_exports(&mut self, module: ModuleId, body: HirId) {
        let bindings = self.mir.hir[body.index()].children.clone();
        for binding in bindings {
            if binding.role != Role::Binding
                || !matches!(
                    self.mir.hir[binding.node.index()].kind,
                    HirKind::Binding {
                        kind: BindingKind::Export,
                        ..
                    }
                )
            {
                continue;
            }
            let Some(name) = self.child(binding.node, Role::Name) else {
                continue;
            };
            let symbol = self.symbol(
                Some(module),
                self.name(name),
                SymbolKind::Export,
                Some(binding.node),
                self.mir.module_scopes[module.index()],
            );
            self.mir.exports[module.index()].push(symbol);
        }
    }
    fn conflict(&mut self, conflict: ResolveConflict) -> ResolveState {
        let id = ConflictId(
            self.mir
                .resolve_conflicts
                .len()
                .try_into()
                .expect("resolve conflict capacity"),
        );
        self.mir.resolve_conflicts.push(conflict);
        ResolveState::Conflicted(id)
    }
    fn diagnose_duplicates(&mut self, patterns: bool) {
        for scope in 0..self.mir.scopes.len() {
            let mut groups = BTreeMap::<String, Vec<SymbolId>>::new();
            for binding in &self.mir.scopes[scope].bindings {
                let symbol = &self.mir.symbols[binding.symbol.index()];
                if (symbol.kind == SymbolKind::Pattern) != patterns {
                    continue;
                }
                if patterns && symbol.resolution != ResolveState::Bound(binding.symbol) {
                    continue;
                }
                if binding.after.is_none() {
                    groups
                        .entry(self.mir.symbols[binding.symbol.index()].name.clone())
                        .or_default()
                        .push(binding.symbol);
                }
            }
            for (name, definitions) in groups {
                if definitions.len() < 2 {
                    continue;
                }
                let state = self.conflict(ResolveConflict::DuplicateDefinition {
                    name: name.clone(),
                    definitions: definitions.clone(),
                });
                let first = self.mir.symbols[definitions[0].index()].declarations[0];
                let second = self.mir.symbols[definitions[1].index()].declarations[0];
                let description = if definitions
                    .iter()
                    .all(|id| self.mir.symbols[id.index()].kind == SymbolKind::TypeParameter)
                {
                    "type parameter"
                } else if patterns {
                    "pattern binding"
                } else {
                    "definition"
                };
                self.mir.diagnostics.push(
                    Diagnostic::error(
                        format!("duplicate {description} {name:?}"),
                        self.mir.hir[second.index()].location,
                    )
                    .with_secondary("first declared here", self.mir.hir[first.index()].location),
                );
                for id in definitions {
                    self.mir.symbols[id.index()].resolution = state.clone();
                }
            }
        }
        if !patterns {
            for module in 0..self.mir.exports.len() {
                let mut groups = BTreeMap::<String, Vec<SymbolId>>::new();
                for &id in &self.mir.exports[module] {
                    groups
                        .entry(self.mir.symbols[id.index()].name.clone())
                        .or_default()
                        .push(id);
                }
                for (name, definitions) in groups {
                    if definitions.len() < 2 {
                        continue;
                    }
                    let state = self.conflict(ResolveConflict::DuplicateDefinition {
                        name: name.clone(),
                        definitions: definitions.clone(),
                    });
                    let location = self.mir.hir
                        [self.mir.symbols[definitions[1].index()].declarations[0].index()]
                    .location;
                    self.mir.diagnostics.push(Diagnostic::error(
                        format!("duplicate export {name:?}"),
                        location,
                    ));
                    for id in definitions {
                        self.mir.symbols[id.index()].resolution = state.clone();
                    }
                }
            }
        }
    }
}
