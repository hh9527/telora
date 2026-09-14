//! Explicit source interfaces. Invalid interfaces remain inspectable but cannot seal.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeclarationContractState {
    Missing,
    Incomplete(HirId),
    Unresolved,
    Complete(TypeId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationContract {
    pub symbol: SymbolId,
    pub declaration: HirId,
    pub annotation: Option<HirId>,
    pub exports: Vec<SymbolId>,
    pub state: DeclarationContractState,
}

impl Mir {
    fn contract_hole(&self, annotation: HirId) -> Option<HirId> {
        let mut pending = vec![annotation];
        let mut seen = BTreeSet::new();
        while let Some(node) = pending.pop() {
            if !seen.insert(node) { continue; }
            let syntax = &self.hir[node.index()];
            if matches!(syntax.kind, HirKind::InferredTypeArgument) { return Some(node); }
            pending.extend(syntax.children.iter().map(|edge| edge.node));
            // An alias must not hide an interface hole in its definition.
            if let Some(slot) = syntax.resolution
                && let ResolveState::Bound(symbol) = self.resolve_slots[slot.index()]
                && self.symbols[symbol.index()].kind == SymbolKind::Declaration(BindingKind::Type) {
                pending.extend(self.symbols[symbol.index()].declarations.iter().flat_map(|node|
                    self.hir[node.index()].children.iter().filter(|edge| edge.role == Role::Value).map(|edge| edge.node)));
            }
        }
        None
    }

    fn declaration_contract_image(&self) -> Vec<DeclarationContract> {
        let mut declarations = BTreeMap::<SymbolId, Vec<SymbolId>>::new();
        for (index, definition) in self.symbols.iter().enumerate() {
            let Some(module) = definition.module else { continue; };
            if definition.scope != self.module_scopes[module.index()]
                || !matches!(definition.kind, SymbolKind::Declaration(BindingKind::Def | BindingKind::Let
                    | BindingKind::Native | BindingKind::Decl)) { continue; }
            // Member imports reuse the selected type-domain identity. They are
            // lowered as Def, but are not ordinary source value declarations.
            if definition.declarations.iter().all(|node| matches!(self.hir[node.index()].kind,
                HirKind::Binding { kind: BindingKind::Def, imported: Some(_), .. })) { continue; }
            declarations.insert(SymbolId(index as u32), Vec::new());
        }
        for &export in self.exports.iter().flatten() {
            let ResolveState::Bound(symbol) = self.symbols[export.index()].resolution else { continue; };
            if let Some(exports) = declarations.get_mut(&symbol) { exports.push(export); }
        }
        declarations.into_iter().filter_map(|(symbol, exports)| {
            let declaration = *self.symbols[symbol.index()].declarations.last()?;
            let annotation = self.symbols[symbol.index()].declarations.iter().find_map(|declaration|
                self.hir[declaration.index()].children.iter()
                    .find(|edge| edge.role == Role::Annotation).map(|edge| edge.node));
            let state = match annotation {
                None => DeclarationContractState::Missing,
                Some(annotation) => match self.contract_hole(annotation) {
                    Some(hole) => DeclarationContractState::Incomplete(hole),
                    None => match self.symbol_types.get(symbol.index()).and_then(|slot| self.ty_slots.get(slot.index())) {
                        Some(TypeState::Known(ty)) => DeclarationContractState::Complete(*ty),
                        _ => DeclarationContractState::Unresolved,
                    },
                },
            };
            Some(DeclarationContract { symbol, declaration, annotation, exports, state })
        }).collect()
    }

    pub(crate) fn build_declaration_contracts(&mut self) {
        self.declaration_contracts = self.declaration_contract_image();
        for contract in &self.declaration_contracts {
            let diagnostic = match contract.state {
                DeclarationContractState::Missing => Diagnostic::error(
                    "top-level value requires an explicit complete type annotation",
                    self.hir[contract.declaration.index()].location),
                DeclarationContractState::Incomplete(hole) => Diagnostic::error(
                    "top-level type contract cannot contain an inference hole", self.hir[hole.index()].location),
                // Name/type errors already retain their original diagnostic.
                _ => continue,
            };
            self.diagnostics.push(diagnostic);
        }
    }

    pub(crate) fn validate_declaration_contracts(&self) -> bool {
        self.declaration_contracts == self.declaration_contract_image()
            && self.declaration_contracts.iter().all(|contract| matches!(contract.state, DeclarationContractState::Complete(_)))
    }
}
