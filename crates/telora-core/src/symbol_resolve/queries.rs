use super::*;

type Ready<T> = Result<T, ResolveTask>;

impl Pass<'_> {
    pub(super) fn constructor(&self, state: &ResolveState) -> Ready<bool> {
        let ResolveState::Bound(id) = *state else {
            return Ok(false);
        };
        self.mir.resolution_facts.constructors[id.index()].ok_or(ResolveTask::Constructor(id))
    }

    pub(super) fn constructor_step(&self, id: SymbolId) -> Ready<bool> {
        let Some(&node) = self.mir.symbols[id.index()].declarations.last() else {
            return Ok(false);
        };
        Ok(match self.mir.hir[node.index()].kind {
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
                kind: BindingKind::Type | BindingKind::Def,
                ..
            } => {
                let Some(value) = self.child(node, Role::Value) else {
                    return Ok(false);
                };
                if matches!(
                    self.mir.hir[node.index()].kind,
                    HirKind::Binding {
                        kind: BindingKind::Def,
                        ..
                    }
                ) && matches!(self.mir.hir[value.index()].kind, HirKind::Field)
                {
                    let receiver = self.child(value, Role::Receiver).unwrap();
                    self.constructor_namespace(receiver)?
                } else {
                    match self.reference_value(value)? {
                        Some(state) => self.constructor(&state)?,
                        None => false,
                    }
                }
            }
            _ => false,
        })
    }

    fn constructor_namespace(&self, node: HirId) -> Ready<bool> {
        self.mir.resolution_facts.constructor_namespaces[node.index()]
            .ok_or(ResolveTask::ConstructorNamespace(node))
    }

    pub(super) fn constructor_namespace_step(&self, node: HirId) -> Ready<bool> {
        if matches!(
            self.mir.hir[node.index()].kind,
            HirKind::Call | HirKind::TypeApply
        ) {
            return match self.child(node, Role::Callee) {
                Some(callee) => self.constructor_namespace(callee),
                None => Ok(false),
            };
        }
        let Some(ResolveState::Bound(symbol)) = self.reference_value(node)? else {
            return Ok(false);
        };
        let declaration = &self.mir.symbols[symbol.index()];
        if let Some(id) = declaration.native_type {
            let native = self.mir.modules[declaration.module.unwrap().index()]
                .native
                .as_ref()
                .unwrap();
            return Ok(native.types.iter().any(|(slot, rule)| {
                *slot == id.slot
                    && matches!(
                        rule,
                        NativeTypeRule::Primitive(
                            TypeConstructor::Bool | TypeConstructor::PropertyTarget
                        ) | NativeTypeRule::Constructor(
                            TypeFunction::Option | TypeFunction::Result | TypeFunction::FoldControl
                        )
                    )
            }));
        }
        let Some(&node) = declaration.declarations.last() else {
            return Ok(false);
        };
        Ok(matches!(
            self.mir.hir[node.index()].kind,
            HirKind::Binding {
                initializer: Some(DeclaredInitializerKind::Enum),
                ..
            }
        ))
    }

    pub(super) fn namespace(&self, id: SymbolId) -> Ready<Option<ModuleId>> {
        self.mir.resolution_facts.namespaces[id.index()].ok_or(ResolveTask::Namespace(id))
    }

    pub(super) fn namespace_step(&self, id: SymbolId) -> Ready<Option<ModuleId>> {
        // Imports may become Namespace only once their symbol task completes.
        self.resolve_symbol(id)?;
        if let SymbolKind::Namespace(module) = self.mir.symbols[id.index()].kind {
            return Ok(Some(module));
        }
        if self.mir.symbols[id.index()].kind != SymbolKind::Declaration(BindingKind::Def) {
            return Ok(None);
        }
        let Some(&node) = self.mir.symbols[id.index()].declarations.last() else {
            return Ok(None);
        };
        let Some(value) = self.child(node, Role::Value) else {
            return Ok(None);
        };
        match self.reference_value(value)? {
            Some(ResolveState::Bound(id)) => self.namespace(id),
            _ => Ok(None),
        }
    }
}
