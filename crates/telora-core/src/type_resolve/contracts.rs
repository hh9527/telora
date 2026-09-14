//! Establish type contracts on the same graph before adding value evidence.
use super::*;

impl Solver<'_> {
    pub(super) fn solve_contracts(&mut self) -> Vec<bool> {
        let generated = self.mir.hir.iter().enumerate().map(|(index, node)|
            self.type_uses[index] || matches!(node.kind,
                HirKind::TypeParameter | HirKind::Binding {
                    kind: BindingKind::Type | BindingKind::NativeType | BindingKind::Trait, .. }))
            .collect::<Vec<_>>();
        for (index, &generate) in generated.iter().enumerate() {
            if generate { self.generate(HirId(index as u32)); }
        }
        // An annotation supplies the binding's type; its initializer is not
        // generated here. Paired decl/def nodes already share a symbol slot.
        for index in 0..self.mir.symbols.len() {
            let declarations = self.mir.symbols[index].declarations.clone();
            for node in declarations {
                if matches!(self.mir.hir[node.index()].kind, HirKind::Binding { .. }) {
                    self.annotation(node);
                }
            }
        }
        loop {
            let revision = self.revision;
            let pending = std::mem::take(&mut self.tasks);
            for task in pending {
                if let Some(task) = self.solve_task(task) { self.tasks.push(task); }
            }
            if self.revision == revision { break; }
        }
        self.mir.declaration_contract_ready = self.mir.symbol_types.iter().enumerate()
            .map(|(index, slot)| self.closed_contract(*slot)
                && self.mir.symbol_generics[index].iter().all(|parameter|
                    self.mir.symbols[parameter.index()].declarations.iter().all(|node|
                        self.children(*node, Role::Bound).iter().all(|bound| self.closed_contract(bound.ty())))))
            .collect();
        self.protect_contracts();
        generated
    }

    fn protect_contracts(&mut self) {
        let mut pending = self.mir.symbol_types.iter().enumerate().filter_map(|(index, slot)|
            self.mir.declaration_contract_ready[index].then_some(*slot)).collect::<Vec<_>>();
        while let Some(slot) = pending.pop() {
            let root = self.root(slot);
            if self.contract_slots[root.index()] { continue; }
            let Some(term) = self.term(root).cloned() else { continue; };
            self.contract_slots[root.index()] = true;
            pending.extend(term.arguments);
            if let TypeConstructor::Nominal(symbol) = term.constructor
                && let Some(index) = self.nominal_index[symbol.index()] {
                pending.extend(self.mir.type_definitions[index].members.iter().filter_map(|member| member.payload));
            }
        }
    }

    fn closed_contract(&self, slot: TypeSlotId) -> bool {
        let mut pending = vec![slot];
        let mut visited = BTreeSet::new();
        while let Some(slot) = pending.pop() {
            let root = self.root(slot);
            if !visited.insert(root) { continue; }
            let Some(term) = self.term(root) else { return false; };
            pending.extend(term.arguments.iter().copied());
            if let TypeConstructor::Nominal(symbol) = term.constructor
                && let Some(index) = self.nominal_index[symbol.index()] {
                // A nominal ID alone does not prove its field/payload skeleton
                // complete. Recursive definitions terminate through visited slots.
                pending.extend(self.mir.type_definitions[index].members.iter().filter_map(|member| member.payload));
            }
        }
        true
    }
}
