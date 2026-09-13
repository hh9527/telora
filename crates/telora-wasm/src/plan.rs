use std::collections::{BTreeMap, BTreeSet};
use telora_core::mir::{
    GenericInstanceId, HirId, HirKind, Mir, ResolveState, Role, SealedExecutable, SymbolId, TypeId,
    TypeState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key {
    pub node: HirId,
    pub instance: Option<GenericInstanceId>,
    pub callable: bool,
}

impl Key {
    pub fn ty(self, mir: &Mir, node: HirId) -> Result<TypeId, String> {
        if let Some(instance) = self.instance {
            return mir.generic_instances[instance.index()]
                .ty(node)
                .ok_or_else(|| format!("Wasm: instance has no sealed type for {node:?}"));
        }
        match mir.ty_slots[node.index()] {
            TypeState::Known(ty) => Ok(ty),
            _ => Err(format!("Wasm: node {node:?} has no sealed type")),
        }
    }
    pub fn reference(self, mir: &Mir, node: HirId) -> Option<GenericInstanceId> {
        match self.instance {
            Some(instance) => mir.generic_instances[instance.index()].reference(node),
            None => mir.generic_references[node.index()].and_then(|r| r.instance()),
        }
    }
}

pub(crate) struct Plan {
    pub functions: BTreeMap<Key, u32>,
    pub globals: BTreeMap<SymbolId, Key>,
    pub instances: BTreeMap<GenericInstanceId, Key>,
    pub demands: BTreeMap<Key, u32>,
    pub captures: BTreeMap<Key, Vec<SymbolId>>,
    pub root: Key,
}

impl Plan {
    pub fn new(executable: &SealedExecutable<'_>) -> Result<Self, String> {
        if !executable.properties().is_empty() || !executable.checks().is_empty() {
            return Err("Wasm: property/check initialization is not implemented yet".into());
        }
        let mir = executable.sealed_mir().mir();
        let root = Key {
            node: executable.root(),
            instance: None,
            callable: false,
        };
        let mut plan = Self {
            functions: BTreeMap::new(),
            globals: BTreeMap::new(),
            instances: BTreeMap::new(),
            demands: BTreeMap::new(),
            captures: BTreeMap::new(),
            root,
        };
        for &symbol in executable.globals() {
            if !mir.symbol_generics[symbol.index()].is_empty() {
                continue;
            }
            let node = *mir.symbols[symbol.index()]
                .declarations
                .last()
                .ok_or("Wasm: missing global declaration")?;
            let key = Key {
                node,
                instance: None,
                callable: false,
            };
            plan.globals.insert(symbol, key);
            plan.functions.insert(key, 0);
            plan.demands.insert(key, 0);
        }
        for &instance in executable.instances() {
            let symbol = mir.generic_instances[instance.index()].symbol;
            let node = *mir.symbols[symbol.index()]
                .declarations
                .last()
                .ok_or("Wasm: missing instance declaration")?;
            let key = Key {
                node,
                instance: Some(instance),
                callable: false,
            };
            plan.instances.insert(instance, key);
            plan.functions.insert(key, 0);
            plan.demands.insert(key, 0);
        }
        for root in executable.closure().nodes() {
            if matches!(mir.hir[root.node.index()].kind, HirKind::Closure) {
                plan.functions.insert(
                    Key {
                        node: root.node,
                        instance: root.instance,
                        callable: true,
                    },
                    0,
                );
            }
        }
        plan.functions.insert(plan.root, 0);
        plan.demands.insert(plan.root, 0);
        for (index, function) in plan.functions.values_mut().enumerate() {
            *function = u32::try_from(index)
                .map_err(|_| "Wasm: function index overflow")?
                .checked_add(crate::abi::FIRST_FUNCTION)
                .ok_or("Wasm: function index overflow")?;
        }
        for (index, offset) in plan.demands.values_mut().enumerate() {
            *offset = u32::try_from(index)
                .ok()
                .and_then(|i| i.checked_mul(crate::abi::DEMAND_BYTES))
                .and_then(|n| n.checked_add(crate::abi::STATIC_BASE))
                .ok_or("Wasm: demand offset overflow")?;
        }
        for &key in plan.functions.keys().filter(|key| key.callable) {
            let mut pending = vec![key.node];
            let mut declared = BTreeSet::new();
            let mut referenced = BTreeSet::new();
            while let Some(node) = pending.pop() {
                let syntax = &mir.hir[node.index()];
                if matches!(syntax.kind, HirKind::Binding { .. } | HirKind::Parameter)
                    && let Some(symbol) = mir.hir_symbols[node.index()]
                {
                    declared.insert(symbol);
                }
                if let Some(slot) = syntax.resolution
                    && let ResolveState::Bound(symbol) = mir.resolve_slots[slot.index()]
                    && !executable.globals().contains(&symbol)
                    && matches!(
                        mir.symbols[symbol.index()].kind,
                        telora_core::mir::SymbolKind::Parameter
                            | telora_core::mir::SymbolKind::Pattern
                            | telora_core::mir::SymbolKind::Declaration(
                                telora_core::ast::BindingKind::Let
                                    | telora_core::ast::BindingKind::Def
                            )
                    )
                {
                    referenced.insert(symbol);
                }
                pending.extend(syntax.children.iter().map(|edge| edge.node));
            }
            plan.captures
                .insert(key, referenced.difference(&declared).copied().collect());
        }
        Ok(plan)
    }
}

pub(crate) fn child(mir: &Mir, node: HirId, role: Role) -> Result<HirId, String> {
    mir.hir[node.index()]
        .children
        .iter()
        .find(|e| e.role == role)
        .map(|e| e.node)
        .ok_or_else(|| format!("Wasm: missing {role:?} child at {node:?}"))
}

pub(crate) fn symbol(mir: &Mir, node: HirId) -> Result<SymbolId, String> {
    let slot = mir.hir[node.index()]
        .resolution
        .ok_or("Wasm: missing reference slot")?;
    match mir.resolve_slots[slot.index()] {
        ResolveState::Bound(symbol) => Ok(symbol),
        _ => Err("Wasm: executable reference is not resolved".into()),
    }
}
