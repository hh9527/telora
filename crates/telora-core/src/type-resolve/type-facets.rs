use super::*;

impl Solver<'_> {
    pub(super) fn resolve_constructor_patterns(&mut self) {
        for index in 0..self.mir.hir.len() {
            let node = HirId(index as u32);
            if !matches!(self.mir.hir[index].kind, HirKind::ConstructorPattern | HirKind::PatternName(_)) || self.term(node.ty()).is_none() { continue; }
            let mut callee = if matches!(self.mir.hir[index].kind, HirKind::PatternName(_)) {
                if self.mir.hir_symbols[index].is_some_and(|symbol| self.mir.symbols[symbol.index()].resolution == ResolveState::Bound(symbol)) { continue; }
                node
            } else { self.child(node, Role::Callee).unwrap() };
            let mut seen = std::collections::BTreeSet::new();
            let mut selected = false;
            let mut value_alias = false;
            while seen.insert(callee) {
                match self.mir.member_selections[callee.index()] {
                    Some(MemberSelection::NewtypeConstructor) => {
                        if value_alias { break; }
                        self.mir.member_selections[index] = Some(MemberSelection::NewtypePattern);
                        selected = true;
                        break;
                    }
                    Some(selection @ (MemberSelection::EnumVariant { .. } | MemberSelection::Boolean(_))) => {
                        if value_alias && matches!(selection, MemberSelection::EnumVariant { .. }) { break; }
                        self.mir.member_selections[index] = Some(selection);
                        selected = true;
                        break;
                    }
                    _ => {}
                }
                if let Some(slot) = self.mir.hir[callee.index()].resolution
                    && let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()]
                    && let Some(&declaration) = self.mir.symbols[symbol.index()].declarations.last() {
                    callee = declaration;
                    continue;
                }
                let role = match self.mir.hir[callee.index()].kind {
                    HirKind::TypeApply => Role::Callee,
                    HirKind::Binding { kind: BindingKind::Let | BindingKind::Def, imported: None, .. } => {
                        value_alias = true;
                        Role::Value
                    }
                    HirKind::Binding { .. } | HirKind::TypeAscription => Role::Value,
                    _ => break,
                };
                let Some(next) = self.child(callee, role) else { break; };
                callee = next;
            }
            if !selected {
                self.conflict(node.ty(), node.ty(), Some(self.mir.hir[index].location),
                    "constructor pattern requires a type declaration".into());
            }
        }
    }

    pub(super) fn type_result(&mut self, node: HirId, source: TypeSlotId) {
        if self.type_uses[node.index()] { self.same(node, source); }
        else { self.tasks.push(Task::TypeFacet { node, source }); }
    }

    /// Surface type positions select the declaration's type facet. Other
    /// positions can still receive type evidence from a type application.
    pub(super) fn prepare_type_uses(&mut self) {
        let mut pending = vec![];
        for hir in &self.mir.hir {
            for edge in &hir.children {
                if matches!(edge.role, Role::Annotation | Role::Bound)
                    || matches!((&hir.kind, edge.role),
                        (HirKind::TypeMetadata | HirKind::TypeSyntax, Role::Operand)
                        | (HirKind::TypeApply | HirKind::TypeOperation(_), Role::Argument)
                        | (HirKind::TypeAscription | HirKind::CheckedCast, Role::Target)
                        | (HirKind::Binding { kind: BindingKind::Type, .. }, Role::Value)) {
                    pending.push(edge.node);
                }
            }
        }
        while let Some(node) = pending.pop() {
            if std::mem::replace(&mut self.type_uses[node.index()], true) { continue; }
            pending.extend(self.mir.hir[node.index()].children.iter()
                .filter(|edge| edge.role != Role::Decorator).map(|edge| edge.node));
        }
    }

    pub(super) fn type_facet(&mut self, node: HirId, source: TypeSlotId, finish: bool) -> Option<Task> {
        if matches!(self.mir.ty_slots[self.root(source).index()], TypeState::Conflicted(_)) {
            self.same(node, source);
            return None;
        }
        let Some(term) = self.term(source).cloned() else { return Some(Task::TypeFacet { node, source }); };
        if term.constructor != TypeConstructor::Meta {
            self.same(node, source);
            return None;
        }
        let owner = term.arguments[0];
        let Some(raw) = self.term(owner).cloned() else { return Some(Task::TypeFacet { node, source }); };
        let newtype = matches!(raw.constructor, TypeConstructor::Nominal(symbol)
            if self.nominal_index[symbol.index()].is_some_and(|index| self.mir.type_definitions[index].operation == TypeOperation::Newtype));
        if !newtype || self.term(node.ty()).is_some_and(|term| term.constructor == TypeConstructor::Meta) {
            self.same(node, source);
            return None;
        }
        if !finish && self.term(node.ty()).is_none() { return Some(Task::TypeFacet { node, source }); }
        let TypeConstructor::Nominal(symbol) = raw.constructor else { unreachable!() };
        let (_, members) = self.nominal_members(symbol, &raw.arguments).unwrap();
        self.assign(node, TypeConstructor::Function, vec![members[0].1.unwrap(), owner]);
        self.mir.member_selections[node.index()] = Some(MemberSelection::NewtypeConstructor);
        None
    }

    /// Only a reference with no remaining type-position evidence defaults to
    /// its constructor value. This never changes the declaration's type slot.
    pub(super) fn finish_type_facets(&mut self) -> bool {
        let pending = std::mem::take(&mut self.tasks);
        let mut changed = false;
        for task in pending {
            if let Task::TypeFacet { node, source } = task {
                if let Some(task) = self.type_facet(node, source, true) { self.tasks.push(task); }
                else { changed = true; }
            } else { self.tasks.push(task); }
        }
        changed
    }
}
