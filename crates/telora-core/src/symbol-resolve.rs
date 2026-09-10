//! Second MIR pass: syntax-only declaration indexing and reference closure.
use crate::ast::{BindingKind, DeclaredInitializerKind};
use crate::mir::*;
use crate::source::Diagnostic;
use std::collections::BTreeMap;

#[cfg(test)]
#[path = "symbol-resolve-tests.rs"]
mod tests;

/// All names come from declarations and imports, including the prelude.
pub fn resolve(mir: &mut Mir) {
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
        active: vec![],
        reference_active: vec![],
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
    pass.active.resize(pass.mir.symbols.len(), false);
    pass.reference_active
        .resize(pass.mir.resolve_slots.len(), false);
    for id in 0..pass.mir.symbols.len() {
        let state = pass.resolve_symbol(SymbolId(id as u32));
        let symbol = &pass.mir.symbols[id];
        if state == ResolveState::Unresolved
            && matches!(symbol.kind, SymbolKind::Import | SymbolKind::Export)
        {
            let location = pass.mir.hir[symbol.declarations[0].index()].location;
            pass.mir.diagnostics.push(Diagnostic::error(
                format!("unresolved {:?} {:?}", symbol.kind, symbol.name),
                location,
            ));
        }
    }
    pass.diagnose_duplicates(true);
    for id in 0..pass.mir.hir.len() {
        if pass.mir.hir[id].resolution.is_some() {
            pass.reference(HirId(id as u32));
        }
    }
    for node in &pass.mir.hir {
        if let Some(slot) = node.resolution {
            assert_ne!(pass.mir.resolve_slots[slot.index()], ResolveState::Pending);
            if pass.mir.resolve_slots[slot.index()] == ResolveState::Unresolved {
                pass.mir.diagnostics.push(Diagnostic::error(
                    format!("unresolved symbol at {:?}", node.kind),
                    node.location,
                ));
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
}

struct Pass<'a> {
    mir: &'a mut Mir,
    active: Vec<bool>,
    reference_active: Vec<bool>,
    import_edges: BTreeMap<HirId, usize>,
}
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
                BindingKind::OpenImport => {
                    if let Some(import) = self.import_edges.get(&edge.node) {
                        self.mir.scopes[scope.index()].open_imports.push(*import);
                    }
                }
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
        for edge in edges {
            self.index(edge.node, scope);
        }
    }
    fn index(&mut self, node: HirId, scope: ScopeId) {
        self.mir.hir_scopes[node.index()] = Some(scope);
        let edges = self.mir.hir[node.index()].children.clone();
        let module = self.mir.hir[node.index()].module;
        match self.mir.hir[node.index()].kind {
            HirKind::Block => {
                let child = self.scope(module, Some(scope));
                self.index_block(node, child);
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
                for edge in edges {
                    self.index(edge.node, nested);
                }
            }
            HirKind::IfLet | HirKind::LetElse | HirKind::MatchArm { .. } => {
                let nested = self.scope(module, Some(scope));
                for edge in edges {
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
                    self.index(edge.node, target);
                }
            }
            HirKind::PatternName(_) => {
                self.declare(node, SymbolKind::Pattern, scope, None);
            }
            _ => {
                for edge in edges {
                    self.index(edge.node, scope);
                }
            }
        }
    }
    fn index_exports(&mut self, module: ModuleId, body: HirId) {
        let has_exports = self.mir.hir[body.index()].children.iter().any(|edge| {
            edge.role == Role::Binding
                && matches!(
                    self.mir.hir[edge.node.index()].kind,
                    HirKind::Binding {
                        kind: BindingKind::Export,
                        ..
                    }
                )
        });
        if !has_exports {
            return;
        }
        let Some(result) = self.child(body, Role::Result) else {
            return;
        };
        if !matches!(self.mir.hir[result.index()].kind, HirKind::Dict) {
            return;
        }
        let fields = self.mir.hir[result.index()].children.clone();
        for field in fields {
            let Some(name) = self.child(field.node, Role::Name) else {
                continue;
            };
            let symbol = self.symbol(
                Some(module),
                self.name(name),
                SymbolKind::Export,
                Some(field.node),
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
                self.mir.diagnostics.push(
                    Diagnostic::error(
                        format!("duplicate definition {name:?}"),
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
    fn exported(&mut self, module: ModuleId, name: &str) -> ResolveState {
        let candidates = self.mir.exports[module.index()]
            .iter()
            .copied()
            .filter(|id| self.mir.symbols[id.index()].name == name)
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [] => ResolveState::Unresolved,
            [id] => self.resolve_symbol(*id),
            _ => self.mir.symbols[candidates[0].index()].resolution.clone(),
        }
    }
    fn resolve_symbol(&mut self, id: SymbolId) -> ResolveState {
        let existing = self.mir.symbols[id.index()].resolution.clone();
        if existing != ResolveState::Pending {
            return existing;
        }
        if self.active[id.index()] {
            return ResolveState::Unresolved;
        }
        self.active[id.index()] = true;
        let node = self.mir.symbols[id.index()].declarations[0];
        let result = match self.mir.symbols[id.index()].kind {
            SymbolKind::Export => self
                .child(node, Role::Value)
                .and_then(|value| self.reference_value(value))
                .unwrap_or(ResolveState::Bound(id)),
            SymbolKind::Import => match self
                .import_edges
                .get(&node)
                .map(|edge| self.mir.imports[*edge].target.clone())
            {
                Some(ModuleTarget::Bound(module)) => {
                    let HirKind::Binding { imported, .. } = &self.mir.hir[node.index()].kind else {
                        unreachable!()
                    };
                    match imported.clone() {
                        Some(name) => self.exported(module, &name),
                        None => {
                            self.mir.symbols[id.index()].kind = SymbolKind::Namespace(module);
                            ResolveState::Bound(id)
                        }
                    }
                }
                Some(ModuleTarget::Conflicted(candidates)) => {
                    self.conflict(ResolveConflict::ModuleCandidates { candidates })
                }
                _ => ResolveState::Unresolved,
            },
            SymbolKind::Pattern => {
                let scope = self.mir.symbols[id.index()].scope.unwrap();
                let name = self.mir.symbols[id.index()].name.clone();
                match self.mir.scopes[scope.index()]
                    .parent
                    .map(|parent| self.lookup(parent, node, &name, true))
                {
                    None | Some(ResolveState::Unresolved) => ResolveState::Bound(id),
                    Some(state) => state,
                }
            }
            _ => unreachable!(),
        };
        self.active[id.index()] = false;
        self.mir.symbols[id.index()].resolution = result.clone();
        result
    }
    fn lookup(
        &mut self,
        mut scope: ScopeId,
        node: HirId,
        name: &str,
        pattern: bool,
    ) -> ResolveState {
        loop {
            let local = self.mir.scopes[scope.index()]
                .bindings
                .iter()
                .rev()
                .find(|binding| {
                    self.mir.symbols[binding.symbol.index()].name == name
                        && binding.after.is_none_or(|after| after < node)
                })
                .map(|binding| binding.symbol);
            if let Some(id) = local {
                let state = self.resolve_symbol(id);
                return if !pattern || self.constructor(&state, &mut vec![]) {
                    state
                } else {
                    ResolveState::Unresolved
                };
            }
            let imports = self.mir.scopes[scope.index()].open_imports.clone();
            let mut candidates = vec![];
            for import in imports {
                if let ModuleTarget::Bound(module) = self.mir.imports[import].target {
                    candidates.extend(
                        self.mir.exports[module.index()]
                            .iter()
                            .copied()
                            .filter(|id| self.mir.symbols[id.index()].name == name),
                    );
                }
            }
            if !candidates.is_empty() {
                let mut targets = BTreeMap::new();
                for candidate in candidates {
                    let state = self.resolve_symbol(candidate);
                    let key = if let ResolveState::Bound(id) = state {
                        id
                    } else {
                        candidate
                    };
                    targets.entry(key).or_insert((candidate, state));
                }
                if pattern
                    && !targets
                        .values()
                        .any(|(_, state)| self.constructor(state, &mut vec![]))
                {
                    return ResolveState::Unresolved;
                }
                if targets.len() == 1 {
                    return targets.into_values().next().unwrap().1;
                }
                let candidates = targets.into_values().map(|(id, _)| id).collect();
                let state = self.conflict(ResolveConflict::AmbiguousImport {
                    name: name.into(),
                    candidates,
                });
                self.mir.diagnostics.push(Diagnostic::error(
                    format!("ambiguous import {name:?}"),
                    self.mir.hir[node.index()].location,
                ));
                return state;
            }
            match self.mir.scopes[scope.index()].parent {
                Some(parent) => scope = parent,
                None => break,
            }
        }
        ResolveState::Unresolved
    }
    fn constructor(&mut self, state: &ResolveState, seen: &mut Vec<SymbolId>) -> bool {
        let ResolveState::Bound(id) = *state else {
            return false;
        };
        if seen.contains(&id) {
            return false;
        }
        seen.push(id);
        let Some(&node) = self.mir.symbols[id.index()].declarations.last() else {
            return false;
        };
        match self.mir.hir[node.index()].kind {
            HirKind::Binding {
                initializer: Some(DeclaredInitializerKind::Newtype),
                ..
            }
            | HirKind::Binding {
                kind: BindingKind::Def,
                imported: Some(_),
                ..
            } => true,
            HirKind::Binding {
                kind: BindingKind::Type,
                ..
            } => {
                let state = self
                    .child(node, Role::Value)
                    .and_then(|value| self.reference_value(value));
                state.is_some_and(|state| self.constructor(&state, seen))
            }
            HirKind::Binding {
                kind: BindingKind::Def,
                ..
            } => {
                let Some(value) = self.child(node, Role::Value) else {
                    return false;
                };
                if matches!(self.mir.hir[value.index()].kind, HirKind::Field) {
                    let receiver = self.child(value, Role::Receiver).unwrap();
                    self.constructor_namespace(receiver, seen)
                } else {
                    self.reference_value(value)
                        .is_some_and(|state| self.constructor(&state, seen))
                }
            }
            _ => false,
        }
    }
    fn constructor_namespace(&mut self, node: HirId, seen: &mut Vec<SymbolId>) -> bool {
        if matches!(
            self.mir.hir[node.index()].kind,
            HirKind::Call | HirKind::TypeApply
        ) {
            return self
                .child(node, Role::Callee)
                .is_some_and(|callee| self.constructor_namespace(callee, seen));
        }
        let Some(ResolveState::Bound(symbol)) = self.reference_value(node) else {
            return false;
        };
        let declaration = &self.mir.symbols[symbol.index()];
        if let Some(id) = declaration.native_type {
            let native = self.mir.modules[declaration.module.unwrap().index()]
                .native
                .as_ref()
                .unwrap();
            return native.types.iter().any(|(slot, rule)| {
                *slot == id.slot
                    && matches!(
                        rule,
                        NativeTypeRule::Primitive(
                            TypeConstructor::Bool | TypeConstructor::PropertyTarget
                        ) | NativeTypeRule::Constructor(
                            TypeFunction::Option | TypeFunction::Result | TypeFunction::FoldControl
                        )
                    )
            });
        }
        if seen.contains(&symbol) {
            return false;
        }
        seen.push(symbol);
        let Some(&node) = declaration.declarations.last() else {
            return false;
        };
        matches!(
            self.mir.hir[node.index()].kind,
            HirKind::Binding {
                initializer: Some(DeclaredInitializerKind::Enum),
                ..
            }
        )
    }
    fn namespace(&mut self, id: SymbolId, seen: &mut Vec<SymbolId>) -> Option<ModuleId> {
        if seen.contains(&id) {
            return None;
        }
        seen.push(id);
        if let SymbolKind::Namespace(module) = self.mir.symbols[id.index()].kind {
            return Some(module);
        }
        if !matches!(
            self.mir.symbols[id.index()].kind,
            SymbolKind::Declaration(BindingKind::Def)
        ) {
            return None;
        }
        let node = *self.mir.symbols[id.index()].declarations.last()?;
        let value = self.child(node, Role::Value)?;
        let ResolveState::Bound(id) = self.reference_value(value)? else {
            return None;
        };
        self.namespace(id, seen)
    }
    fn reference_value(&mut self, node: HirId) -> Option<ResolveState> {
        self.mir.hir[node.index()]
            .resolution
            .map(|_| self.reference(node))
    }
    fn reference(&mut self, node: HirId) -> ResolveState {
        let slot = self.mir.hir[node.index()]
            .resolution
            .expect("reference slot");
        let state = self.mir.resolve_slots[slot.index()].clone();
        if state != ResolveState::Pending {
            return state;
        }
        if self.reference_active[slot.index()] {
            return ResolveState::Unresolved;
        }
        self.reference_active[slot.index()] = true;
        let scope = self.mir.hir_scopes[node.index()].expect("reference scope");
        let state = match &self.mir.hir[node.index()].kind {
            HirKind::Variable(name) => {
                let name = name.clone();
                self.lookup(scope, node, &name, false)
            }
            HirKind::PatternName(_) => self
                .resolve_symbol(self.mir.hir_symbols[node.index()].expect("pattern declaration")),
            HirKind::Field => {
                let receiver = self.child(node, Role::Receiver).unwrap();
                let name = self.child(node, Role::Name).unwrap();
                match self.reference_value(receiver) {
                    Some(ResolveState::Bound(symbol)) => {
                        match self.namespace(symbol, &mut vec![]) {
                            Some(module) => self.exported(module, &self.name(name)),
                            _ => ResolveState::Member { receiver, name },
                        }
                    }
                    Some(state @ (ResolveState::Unresolved | ResolveState::Conflicted(_))) => state,
                    _ => ResolveState::Member { receiver, name },
                }
            }
            _ => unreachable!(),
        };
        self.reference_active[slot.index()] = false;
        self.mir.resolve_slots[slot.index()] = state.clone();
        state
    }
}
