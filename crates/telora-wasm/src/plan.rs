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
    pub special: Special,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Special {
    Normal,
    Configured,
    Property(usize),
    Equal(TypeId),
    Parse(TypeId),
    Json(TypeId),
    Encode(TypeId, TypeId),
    Decode(TypeId, TypeId),
    DecodeVariant(TypeId, TypeId, u32),
}

impl Key {
    pub fn effective_ty(self, mir: &Mir, node: HirId) -> Result<TypeId, String> {
        if let Some(slot) = mir.value_adjustments[node.index()] {
            return match self.instance {
                Some(id) => mir.generic_instances[id.index()]
                    .adjustment(node)
                    .ok_or_else(|| "Wasm: missing sealed adjustment".into()),
                None => match mir.ty_slots[slot.index()] {
                    TypeState::Known(ty) => Ok(ty),
                    _ => Err("Wasm: unsealed adjustment".into()),
                },
            };
        }
        self.ty(mir, node)
    }
    pub fn ty(self, mir: &Mir, node: HirId) -> Result<TypeId, String> {
        if self.special == Special::Configured && node == self.node {
            let original = Self {
                special: Special::Normal,
                ..self
            }
            .ty(mir, node)?;
            return mir.types[original.index()]
                .arguments
                .last()
                .copied()
                .ok_or_else(|| "Wasm: missing configured signature".into());
        }
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
    pub locations: crate::locations::StaticLocations,
    pub constructor_aliases: BTreeMap<Key, Key>,
    pub functions: BTreeMap<Key, u32>,
    pub globals: BTreeMap<SymbolId, Key>,
    pub instances: BTreeMap<GenericInstanceId, Key>,
    pub demands: BTreeMap<Key, u32>,
    pub captures: BTreeMap<Key, Vec<SymbolId>>,
    pub instance_captures: BTreeMap<Key, Vec<GenericInstanceId>>,
    pub local_instances: BTreeSet<GenericInstanceId>,
    pub layouts: Vec<telora_core::candidate_layout::Entry>,
    pub root: Key,
    pub properties: BTreeMap<usize, Key>,
    pub checks: BTreeMap<usize, Key>,
    pub comparisons: BTreeMap<TypeId, Key>,
    pub parsers: BTreeMap<TypeId, Key>,
    pub reflection: Vec<u8>,
}

impl Plan {
    pub fn new(executable: &SealedExecutable<'_>) -> Result<Self, String> {
        let mir = executable.sealed_mir().mir();
        let root = Key {
            node: executable.root(),
            instance: None,
            callable: false,
            special: Special::Normal,
        };
        let mut plan = Self {
            locations: crate::locations::StaticLocations::new(mir)?,
            constructor_aliases: BTreeMap::new(),
            functions: BTreeMap::new(),
            globals: BTreeMap::new(),
            instances: BTreeMap::new(),
            demands: BTreeMap::new(),
            captures: BTreeMap::new(),
            instance_captures: BTreeMap::new(),
            local_instances: BTreeSet::new(),
            layouts: telora_core::candidate_layout::calculate(executable.sealed_mir())?,
            root,
            properties: BTreeMap::new(),
            checks: BTreeMap::new(),
            comparisons: BTreeMap::new(),
            parsers: BTreeMap::new(),
            reflection: vec![],
        };
        for &symbol in executable.globals() {
            if !mir.symbol_generics[symbol.index()].is_empty() {
                continue;
            }
            let node = *mir.symbols[symbol.index()]
                .declarations
                .last()
                .ok_or("Wasm: missing global declaration")?;
            // A monomorphic trait implementation can be reached only through
            // a selected instance. Emit exactly the contexts sealed by MIR;
            // global scope alone does not admit another initializer.
            if executable.closure().nodes().binary_search(&telora_core::mir::ExecutionRoot {
                node,
                instance: None,
            }).is_err() {
                continue;
            }
            let key = Key {
                node,
                instance: None,
                callable: false,
                special: Special::Normal,
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
                special: Special::Normal,
            };
            plan.instances.insert(instance, key);
            if !executable.globals().contains(&symbol) {
                plan.local_instances.insert(instance);
                continue;
            }
            plan.functions.insert(key, 0);
            plan.demands.insert(key, 0);
        }
        for root in executable.closure().nodes() {
            let key = Key {
                node: root.node,
                instance: root.instance,
                callable: true,
                special: Special::Normal,
            };
            let constructor = crate::enums::selection(mir, root.node).is_some_and(|s| {
                matches!(
                    s,
                    telora_core::mir::MemberSelection::EnumVariant { .. }
                        | telora_core::mir::MemberSelection::NewtypeConstructor
                )
            }) && mir.required_types[root.node.index()]
                && key.ty(mir, root.node).is_ok_and(|ty| {
                    mir.types[ty.index()].constructor == telora_core::mir::TypeConstructor::Function
                });
            if matches!(
                mir.hir[root.node.index()].kind,
                HirKind::Closure
                    | HirKind::Interpreter
                    | HirKind::Binding {
                        kind: telora_core::syntax::kinds::BindingKind::Native,
                        ..
                    }
            ) || constructor
            {
                plan.functions.insert(
                    Key {
                        node: root.node,
                        instance: root.instance,
                        callable: true,
                        special: Special::Normal,
                    },
                    0,
                );
                if matches!(mir.hir[root.node.index()].kind, HirKind::Interpreter)
                    || matches!(
                        crate::natives::identity(mir, root.node),
                        Some((18, "property") | (17, "stringify_pretty"))
                    )
                {
                    plan.functions.insert(
                        Key {
                            special: Special::Configured,
                            ..key
                        },
                        0,
                    );
                }
            }
        }
        for &index in executable.properties() {
            let property = &mir.properties[index];
            let node = *property
                .providers
                .first()
                .ok_or("Wasm: property has no provider")?;
            let key = Key {
                node,
                instance: property.instance,
                callable: false,
                special: Special::Property(index),
            };
            plan.properties.insert(index, key);
            plan.functions.insert(key, 0);
            plan.demands.insert(key, 0);
        }
        for &index in executable.checks() {
            let check = &mir.construction_checks[index];
            let key = Key {
                node: check.checker,
                instance: check.instance,
                callable: false,
                special: Special::Normal,
            };
            if !check.concrete || key.ty(mir, key.node)? != check.signature {
                return Err("Wasm: checker signature is not sealed".into());
            }
            plan.checks.insert(index, key);
            plan.functions.insert(key, 0);
            plan.demands.insert(key, 0);
        }
        plan.functions.insert(plan.root, 0);
        plan.demands.insert(plan.root, 0);
        plan.plan_comparisons(executable)?;
        plan.plan_encoders(executable)?;
        plan.plan_decoders(executable)?;
        plan.plan_parsers(executable)?;
        for root in executable.closure().nodes() {
            let Some((17, name @ ("stringify" | "stringify_pretty"))) =
                crate::natives::identity(mir, root.node)
            else {
                continue;
            };
            let key = Key {
                node: root.node,
                instance: root.instance,
                callable: true,
                special: if name == "stringify_pretty" {
                    Special::Configured
                } else {
                    Special::Normal
                },
            };
            let ty = mir.types[key.ty(mir, root.node)?.index()].arguments[0];
            plan.functions.insert(
                Key {
                    special: Special::Json(ty),
                    callable: true,
                    ..plan.root
                },
                0,
            );
        }
        if executable.closure().nodes().iter().any(|root| {
            crate::natives::identity(mir, root.node).is_some_and(|(module, name)| {
                module == 3
                    || (module == 2
                        && matches!(
                            name,
                            "kind"
                                | "get_field_value"
                                | "get_variant_index"
                                | "get_variant_payload"
                                | "tag_raw"
                                | "payload_raw"
                                | "array_items_raw"
                                | "tuple_items_raw"
                                | "fields_raw"
                                | "field_raw"
                        ))
            })
        }) {
            plan.reflection =
                crate::reflection_data::build(executable.sealed_mir().types(), &plan.layouts)?;
        }
        // Constructor identity is (closed signature, variant), independent of
        // the expression's source location. Values carry their own source head.
        let mut constructors = BTreeMap::new();
        for key in plan.functions.keys().copied().collect::<Vec<_>>() {
            if !key.callable || key.special != Special::Normal { continue; }
            let Some(fact @ telora_core::mir::ValueMaterialization::EnumVariant { .. }) = mir.value_materializations[key.node.index()] else { continue; };
            let identity = (key.ty(mir, key.node)?, fact);
            let canonical = *constructors.entry(identity).or_insert(key);
            plan.constructor_aliases.insert(key, canonical);
            if canonical != key { plan.functions.remove(&key); }
        }
        for (index, function) in plan.functions.values_mut().enumerate() {
            *function = u32::try_from(index)
                .map_err(|_| "Wasm: function index overflow")?
                .checked_add(crate::abi::FIRST_FUNCTION)
                .ok_or("Wasm: function index overflow")?;
        }
        let static_base = crate::compose::static_base()?;
        for (index, offset) in plan.demands.values_mut().enumerate() {
            *offset = u32::try_from(index)
                .ok()
                .and_then(|i| i.checked_mul(crate::abi::DEMAND_BYTES))
                .and_then(|n| n.checked_add(static_base))
                .ok_or("Wasm: demand offset overflow")?;
        }
        for &key in plan.functions.keys().filter(|key| key.callable) {
            if matches!(
                key.special,
                Special::Equal(_)
                    | Special::Parse(_)
                    | Special::Json(_)
                    | Special::Encode(_, _)
                    | Special::Decode(_, _)
                    | Special::DecodeVariant(_, _, _)
            ) {
                continue;
            }
            let mut pending = vec![key.node];
            let mut declared = BTreeSet::new();
            let mut referenced = BTreeSet::new();
            let mut instances = BTreeSet::new();
            while let Some(node) = pending.pop() {
                let syntax = &mir.hir[node.index()];
                if matches!(
                    syntax.kind,
                    HirKind::Binding { .. } | HirKind::Parameter | HirKind::PatternName(_)
                ) && let Some(symbol) = mir.hir_symbols[node.index()]
                {
                    declared.insert(symbol);
                }
                if let Some(slot) = syntax.resolution
                    && let ResolveState::Bound(symbol) = mir.resolve_slots[slot.index()]
                    && !executable.globals().contains(&symbol)
                    // Patterns consume sealed tags/layouts, not constructor values.
                    // They intentionally have no value-materialization record.
                    && !(matches!(syntax.kind, HirKind::ConstructorPattern | HirKind::PatternName(_))
                        && matches!(mir.member_selections[node.index()],
                            Some(telora_core::mir::MemberSelection::Boolean(_)
                                | telora_core::mir::MemberSelection::EnumVariant { .. }
                                | telora_core::mir::MemberSelection::NewtypePattern)))
                    && !matches!(crate::enums::selection(mir, node),
                        Some(telora_core::mir::MemberSelection::Boolean(_)))
                    && !matches!(
                        crate::enums::selection(mir, node),
                        Some(
                            telora_core::mir::MemberSelection::EnumVariant { .. }
                                | telora_core::mir::MemberSelection::NewtypeConstructor
                                | telora_core::mir::MemberSelection::NewtypePattern
                        )
                    )
                    && matches!(
                        mir.symbols[symbol.index()].kind,
                        telora_core::mir::SymbolKind::Parameter
                            | telora_core::mir::SymbolKind::Pattern
                            | telora_core::mir::SymbolKind::Declaration(
                                telora_core::syntax::kinds::BindingKind::Let
                                    | telora_core::syntax::kinds::BindingKind::Def
                                    | telora_core::syntax::kinds::BindingKind::Decl
                            )
                    )
                {
                    if let Some(instance) = key.reference(mir, node)
                        && plan.local_instances.contains(&instance)
                    {
                        instances.insert(instance);
                    } else if mir.symbol_generics[symbol.index()].is_empty() {
                        referenced.insert(symbol);
                    }
                }
                pending.extend(syntax.children.iter().map(|edge| edge.node));
            }
            plan.captures
                .insert(key, referenced.difference(&declared).copied().collect());
            plan.instance_captures.insert(
                key,
                instances
                    .into_iter()
                    .filter(|id| !declared.contains(&mir.generic_instances[id.index()].symbol))
                    .collect(),
            );
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
