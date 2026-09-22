use super::*;

type Ready<T> = Result<T, ResolveTask>;

impl Pass<'_> {
    pub(super) fn resolve_symbol(&self, id: SymbolId) -> Ready<ResolveState> {
        let state = self.mir.symbols[id.index()].resolution.clone();
        if state == ResolveState::Pending {
            Err(ResolveTask::Symbol(id))
        } else {
            Ok(state)
        }
    }

    fn exported(&self, module: ModuleId, name: &str) -> Ready<ResolveState> {
        let candidate = self.mir.exports[module.index()]
            .iter()
            .copied()
            .find(|id| self.mir.symbols[id.index()].name == name);
        match candidate {
            None => Ok(ResolveState::Unresolved),
            Some(id) => self.resolve_symbol(id),
        }
    }

    pub(super) fn symbol_step(&mut self, id: SymbolId) -> Ready<ResolveState> {
        let node = self.mir.symbols[id.index()].declarations[0];
        Ok(match self.mir.symbols[id.index()].kind {
            SymbolKind::Export => match self.child(node, Role::Value) {
                Some(value) => self
                    .reference_value(value)?
                    .unwrap_or(ResolveState::Bound(id)),
                None => ResolveState::Bound(id),
            },
            SymbolKind::Import => match self
                .import_edges
                .get(&node)
                .map(|edge| self.mir.imports[*edge].target.clone())
            {
                Some(ModuleTarget::Bound(module)) => {
                    let HirKind::Binding { imported, .. } = &self.mir.hir[node.index()].kind else {
                        unreachable!()
                    };
                    match imported {
                        Some(name) => self.exported(module, name)?,
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
                let state = match self.mir.scopes[scope.index()].parent {
                    Some(parent) => self.lookup(parent, node, &name, true)?,
                    None => ResolveState::Unresolved,
                };
                if state == ResolveState::Unresolved {
                    ResolveState::Bound(id)
                } else {
                    state
                }
            }
            _ => unreachable!(),
        })
    }

    fn lookup(
        &mut self,
        mut scope: ScopeId,
        node: HirId,
        name: &str,
        pattern: bool,
    ) -> Ready<ResolveState> {
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
                let state = self.resolve_symbol(id)?;
                return Ok(if !pattern || self.constructor(&state)? {
                    state
                } else {
                    ResolveState::Unresolved
                });
            }
            let mut candidates = vec![];
            let mut implicit = vec![];
            for &import in &self.mir.scopes[scope.index()].open_imports {
                if let ModuleTarget::Bound(module) = self.mir.imports[import].target {
                    let selected = if self.mir.imports[import].syntax.is_none() {
                        &mut implicit
                    } else {
                        &mut candidates
                    };
                    selected.extend(
                        self.mir.exports[module.index()]
                            .iter()
                            .copied()
                            .filter(|id| self.mir.symbols[id.index()].name == name),
                    );
                }
            }
            if candidates.is_empty() {
                candidates = implicit;
            }
            if !candidates.is_empty() {
                let mut targets = BTreeMap::new();
                for candidate in candidates {
                    let state = self.resolve_symbol(candidate)?;
                    let key = if let ResolveState::Bound(id) = state {
                        id
                    } else {
                        candidate
                    };
                    targets.entry(key).or_insert((candidate, state));
                }
                if pattern {
                    let mut found = false;
                    for (_, state) in targets.values() {
                        if self.constructor(state)? {
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return Ok(ResolveState::Unresolved);
                    }
                }
                if targets.len() == 1 {
                    return Ok(targets.into_values().next().unwrap().1);
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
                return Ok(state);
            }
            match self.mir.scopes[scope.index()].parent {
                Some(parent) => scope = parent,
                None => return Ok(ResolveState::Unresolved),
            }
        }
    }

    pub(super) fn reference_value(&self, node: HirId) -> Ready<Option<ResolveState>> {
        let Some(slot) = self.mir.hir[node.index()].resolution else {
            return Ok(None);
        };
        let state = self.mir.resolve_slots[slot.index()].clone();
        if state == ResolveState::Pending {
            Err(ResolveTask::Reference(node))
        } else {
            Ok(Some(state))
        }
    }

    pub(super) fn reference_step(&mut self, node: HirId) -> Ready<ResolveState> {
        let scope = self.mir.hir_scopes[node.index()].expect("reference scope");
        Ok(match &self.mir.hir[node.index()].kind {
            HirKind::Variable(name) => {
                let name = name.clone();
                self.lookup(scope, node, &name, false)?
            }
            HirKind::StaticPath(path) => {
                let path = path.clone();
                self.static_path(scope, node, &path)?
            }
            HirKind::PatternName(_) => self
                .resolve_symbol(self.mir.hir_symbols[node.index()].expect("pattern declaration"))?,
            HirKind::Field => {
                let receiver = self.child(node, Role::Receiver).unwrap();
                let name = self.child(node, Role::Name).unwrap();
                if matches!(self.mir.hir[name.index()].kind, HirKind::Missing) {
                    ResolveState::Unresolved
                } else {
                    match self.reference_value(receiver)? {
                        Some(ResolveState::Bound(symbol)) => match self.namespace(symbol)? {
                            Some(module) => self.exported(module, &self.name(name))?,
                            None => ResolveState::Member { receiver, name },
                        },
                        Some(state @ (ResolveState::Unresolved | ResolveState::Conflicted(_))) => {
                            state
                        }
                        _ => ResolveState::Member { receiver, name },
                    }
                }
            }
            _ => unreachable!(),
        })
    }

    fn static_path(&mut self, scope: ScopeId, node: HirId, path: &[String]) -> Ready<ResolveState> {
        let Some((first, rest)) = path.split_first() else {
            return Ok(ResolveState::Unresolved);
        };
        let current = self.mir.scopes[scope.index()].module;
        let state = match first.as_str() {
            "crate" | "self" | "super" => {
                let current_name = &self.mir.modules[current.index()].name;
                let name = match first.as_str() {
                    "crate" => current_name.split('/').next().unwrap(),
                    "self" => current_name,
                    "super" => current_name
                        .rsplit_once('/')
                        .map_or("", |(parent, _)| parent),
                    _ => unreachable!(),
                };
                let Some(module) = self
                    .mir
                    .modules
                    .iter()
                    .position(|module| module.name == name)
                else {
                    return Ok(ResolveState::Unresolved);
                };
                let Some((name, tail)) = rest.split_first() else {
                    return Ok(ResolveState::Unresolved);
                };
                let module = ModuleId(module as u32);
                let Some(module_scope) = self.mir.module_scopes[module.index()] else {
                    return Ok(ResolveState::Unresolved);
                };
                let state = self.lookup(module_scope, node, name, false)?;
                return self.static_path_tail(state, tail);
            }
            _ => {
                if let Some(module) = self
                    .mir
                    .modules
                    .iter()
                    .position(|module| module.name == *first)
                {
                    let Some((name, tail)) = rest.split_first() else {
                        return Ok(ResolveState::Unresolved);
                    };
                    let state = self.exported(ModuleId(module as u32), name)?;
                    return self.static_path_tail(state, tail);
                }
                self.lookup(scope, node, first, false)?
            }
        };
        self.static_path_tail(state, rest)
    }

    fn static_path_tail(
        &mut self,
        mut state: ResolveState,
        path: &[String],
    ) -> Ready<ResolveState> {
        for name in path {
            let ResolveState::Bound(symbol) = state else {
                return Ok(state);
            };
            let Some(module) = self.namespace(symbol)? else {
                return Ok(ResolveState::Unresolved);
            };
            state = self.exported(module, name)?;
        }
        Ok(state)
    }
}
