//! Experimental interprocedural provenance and scalar-borrow summaries.
//! No storage-uniqueness proof; unknown bodies/calls always block elision.
use crate::plan::{Key, Plan, Special, child, symbol};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use telora_core::mir::{HirId, HirKind, Mir, Role, SymbolId, TypeConstructor as T, TypeId};

#[path = "collection_summary_value.rs"]
mod value;
pub(crate) use value::{Closure, Path, Value};
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Update {
    pub function: usize,
    pub node: usize,
    pub depth: usize,
    pub collection: &'static str,
    pub receiver: Value,
}
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Summary {
    pub result: Value,
    pub opaque_or_effectful: bool,
    pub scalar_borrow_safe: bool,
    pub calls_followed: usize,
    pub updates: Vec<Update>,
    pub blockers: BTreeMap<String, usize>,
    pub fold_iterations: usize,
    pub fold_widenings: usize,
}
/// Each root has a bounded exploration budget. Recursive and opaque edges are
/// conservative; no optimistic recursive fixed point is used.
#[derive(Clone)]
pub(crate) struct Analysis<'a> {
    mir: &'a Mir,
    keys: Arc<BTreeSet<Key>>,
    bindings: Arc<BTreeMap<SymbolId, HirId>>,
    captures: Arc<BTreeMap<Key, Vec<SymbolId>>>,
    checked_types: Arc<BTreeSet<TypeId>>,
    remaining: usize,
    active: BTreeSet<Key>,
    opaque: bool,
    calls: usize,
    updates: Vec<Update>,
    blockers: BTreeMap<String, usize>,
    fold_iterations: usize,
    fold_widenings: usize,
}
impl<'a> Analysis<'a> {
    pub fn new(mir: &'a Mir, plan: &Plan) -> Self {
        let mut analysis = Self::from_keys(mir, plan.functions.keys().copied());
        analysis.captures = Arc::new(plan.captures.clone());
        analysis.checked_types = Arc::new(
            plan.checks
                .keys()
                .map(|i| mir.construction_checks[*i].owner)
                .collect(),
        );
        analysis
    }
    pub fn from_keys(mir: &'a Mir, keys: impl Iterator<Item = Key>) -> Self {
        let mut bindings = BTreeMap::new();
        for s in &mir.symbols {
            for &d in &s.declarations {
                if !matches!(mir.hir[d.index()].kind, HirKind::Binding { .. }) {
                    continue;
                }
                if let (Some(symbol), Ok(value)) =
                    (mir.hir_symbols[d.index()], child(mir, d, Role::Value))
                {
                    bindings.insert(symbol, value);
                }
            }
        }
        Self {
            mir,
            keys: Arc::new(keys.collect()),
            bindings: Arc::new(bindings),
            captures: Arc::new(BTreeMap::new()),
            checked_types: Arc::new(BTreeSet::new()),
            remaining: 50_000,
            active: BTreeSet::new(),
            opaque: false,
            calls: 0,
            updates: vec![],
            blockers: BTreeMap::new(),
            fold_iterations: 0,
            fold_widenings: 0,
        }
    }
    fn opaque(&mut self) -> Value {
        self.blocked("unsupported_or_unresolved")
    }
    fn blocked(&mut self, reason: &str) -> Value {
        self.opaque = true;
        *self.blockers.entry(reason.into()).or_default() += 1;
        Value::unknown()
    }
    fn direct(&self, key: Key, node: HirId, seen: &mut BTreeSet<HirId>) -> Option<Key> {
        if !seen.insert(node) {
            return None;
        }
        if matches!(self.mir.hir[node.index()].kind, HirKind::Closure) {
            let exact = Key {
                node,
                callable: true,
                special: Special::Normal,
                ..key
            };
            if self.keys.contains(&exact) {
                return Some(exact);
            }
            let mut matches = self
                .keys
                .iter()
                .filter(|k| k.node == node && k.callable && k.special == Special::Normal);
            let first = *matches.next()?;
            return matches.next().is_none().then_some(first);
        }
        if matches!(self.mir.hir[node.index()].kind, HirKind::TypeApply) {
            return self.direct(key, child(self.mir, node, Role::Callee).ok()?, seen);
        }
        if let Ok(s) = symbol(self.mir, node) {
            let mut context = key;
            context.instance = key.reference(self.mir, node).or(key.instance);
            if let Some(value) = self.bindings.get(&s) {
                return self.direct(context, *value, seen);
            }
            for &d in &self.mir.symbols[s.index()].declarations {
                if let Ok(value) = child(self.mir, d, Role::Value) {
                    if let Some(k) = self.direct(context, value, seen) {
                        return Some(k);
                    }
                }
            }
        }
        None
    }
    pub fn function(mut self, key: Key) -> Summary {
        let args = self
            .parameters(key)
            .iter()
            .enumerate()
            .map(|(i, _)| Value {
                parameters: BTreeSet::from([i]),
                paths: BTreeSet::from([Path {
                    function: key.node.index(),
                    root: format!("parameter:{i}"),
                    fields: vec![],
                }]),
                ..Value::default()
            })
            .collect();
        let captures = self
            .captures
            .get(&key)
            .into_iter()
            .flatten()
            .map(|s| {
                (
                    *s,
                    Value {
                        paths: BTreeSet::from([Path {
                            function: key.node.index(),
                            root: format!("capture:{}", s.index()),
                            fields: vec![],
                        }]),
                        ..Value::default()
                    },
                )
            })
            .collect();
        let result = self.invoke(key, args, captures);
        let scalar = key
            .ty(self.mir, key.node)
            .ok()
            .and_then(|ty| self.mir.types[ty.index()].arguments.last().copied())
            .is_some_and(|t| {
                matches!(
                    self.mir.types[t.index()].constructor,
                    T::Int | T::Float | T::Bool
                )
            });
        Summary {
            result,
            opaque_or_effectful: self.opaque,
            scalar_borrow_safe: scalar && !self.opaque,
            calls_followed: self.calls,
            updates: self.updates,
            blockers: self.blockers,
            fold_iterations: self.fold_iterations,
            fold_widenings: self.fold_widenings,
        }
    }
    fn parameters(&self, key: Key) -> Vec<SymbolId> {
        self.mir.hir[key.node.index()]
            .children
            .iter()
            .filter(|e| e.role == Role::Parameter)
            .filter_map(|e| self.mir.hir_symbols[e.node.index()])
            .collect()
    }
    fn invoke(
        &mut self,
        key: Key,
        args: Vec<Value>,
        mut captures: BTreeMap<SymbolId, Value>,
    ) -> Value {
        if self.active.len() >= 32 || !self.active.insert(key) {
            return self.blocked("recursive_or_depth_limit");
        }
        self.calls += 1;
        let params = self.parameters(key);
        let result = if params.len() != args.len() {
            self.opaque()
        } else if let Ok(body) = child(self.mir, key.node, Role::Body) {
            captures.extend(params.into_iter().zip(args));
            self.expr(key, body, &mut captures)
        } else {
            self.opaque()
        };
        self.active.remove(&key);
        result
    }
    fn closure(&self, key: Key, env: &BTreeMap<SymbolId, Value>) -> Value {
        let captures: BTreeMap<_, _> = self
            .captures
            .get(&key)
            .into_iter()
            .flatten()
            .map(|s| {
                (
                    *s,
                    env.get(s).cloned().unwrap_or_else(Value::unknown).bounded(),
                )
            })
            .collect();
        let mut value = Value::default();
        for v in captures.values() {
            value.absorb(v);
        }
        value.callable = Some(Closure {
            key,
            captures: Arc::new(captures),
        });
        value.bounded()
    }
    fn expr(&mut self, key: Key, node: HirId, env: &mut BTreeMap<SymbolId, Value>) -> Value {
        if self.remaining == 0 {
            return self.blocked("node_budget");
        }
        self.remaining -= 1;
        let h = &self.mir.hir[node.index()];
        if (self.mir.value_adjustments[node.index()].is_some()
            || matches!(
                h.kind,
                HirKind::Dict
                    | HirKind::Tuple
                    | HirKind::Call
                    | HirKind::TypeAscription
                    | HirKind::Field
            ))
            && key
                .effective_ty(self.mir, node)
                .ok()
                .is_some_and(|ty| self.checked_types.contains(&ty))
        {
            // Construction checks are implicit calls, and may publish blame or
            // subjects even when the record's selected field looks pure.
            self.blocked("construction_check");
        }
        match &h.kind {
            HirKind::Int(_) | HirKind::Float(_) | HirKind::String(_) | HirKind::Atom(_) => {
                Value::fresh()
            }
            HirKind::Variable(_) | HirKind::StaticPath(_) => {
                if let Ok(s) = symbol(self.mir, node) {
                    if let Some(v) = env.get(&s) {
                        return v.clone();
                    }
                }
                if let Some(k) = self.direct(key, node, &mut BTreeSet::new()) {
                    return self.closure(k, env);
                }
                self.blocked("unresolved_capture_or_value")
            }
            HirKind::Closure => self
                .direct(key, node, &mut BTreeSet::new())
                .map(|k| self.closure(k, env))
                .unwrap_or_else(|| self.opaque()),
            HirKind::TypeAscription => child(self.mir, node, Role::Value)
                .map(|n| self.expr(key, n, env))
                .unwrap_or_else(|_| self.opaque()),
            HirKind::TypeApply => child(self.mir, node, Role::Callee)
                .map(|n| self.expr(key, n, env))
                .unwrap_or_else(|_| self.opaque()),
            HirKind::Block => {
                for e in h.children.iter().filter(|e| e.role == Role::Binding) {
                    let b = &self.mir.hir[e.node.index()];
                    let value = child(self.mir, e.node, Role::Value)
                        .map(|n| self.expr(key, n, env))
                        .unwrap_or_else(|_| self.opaque());
                    if let Some(s) = self.mir.hir_symbols[e.node.index()] {
                        env.insert(s, value);
                    } else if !matches!(b.kind, HirKind::Binding { .. }) {
                        self.opaque = true;
                    }
                }
                child(self.mir, node, Role::Result)
                    .map(|n| self.expr(key, n, env))
                    .unwrap_or_else(|_| self.opaque())
            }
            HirKind::If => {
                if let Ok(c) = child(self.mir, node, Role::Condition) {
                    self.expr(key, c, env);
                } else {
                    self.opaque = true;
                }
                let a = child(self.mir, node, Role::Then)
                    .map(|n| self.expr(key, n, &mut env.clone()))
                    .unwrap_or_else(|_| self.opaque());
                let b = child(self.mir, node, Role::Else)
                    .map(|n| self.expr(key, n, &mut env.clone()))
                    .unwrap_or_else(|_| self.opaque());
                a.join(b)
            }
            HirKind::Binary(_) | HirKind::Unary(_) => {
                for e in &h.children {
                    self.expr(key, e.node, env);
                }
                // Scalar arithmetic creates a value; it never publishes an input slot.
                if key.ty(self.mir, node).ok().is_some_and(|t| {
                    matches!(
                        self.mir.types[t.index()].constructor,
                        T::Int | T::Float | T::Bool
                    )
                }) {
                    Value::fresh()
                } else {
                    self.opaque()
                }
            }
            HirKind::Array | HirKind::Tuple => {
                let mut value = Value::fresh();
                for (i, e) in h
                    .children
                    .iter()
                    .filter(|e| e.role == Role::Item)
                    .enumerate()
                {
                    let item = self.expr(key, e.node, env);
                    value.absorb(&item);
                    value.members.insert(i.to_string(), item);
                }
                if matches!(h.kind, HirKind::Array) {
                    let item = value
                        .members
                        .values()
                        .cloned()
                        .reduce(Value::join)
                        .unwrap_or_default();
                    value.members.insert("*".into(), item);
                }
                value.bounded()
            }
            HirKind::Dict => {
                let mut value = Value::fresh();
                for e in h.children.iter().filter(|e| e.role == Role::Field) {
                    let field = &self.mir.hir[e.node.index()];
                    let name = field
                        .children
                        .iter()
                        .find(|e| e.role == Role::Name)
                        .and_then(|e| match &self.mir.hir[e.node.index()].kind {
                            HirKind::Name(n) => Some(n.clone()),
                            _ => None,
                        });
                    let Some(name) = name else {
                        self.blocked("record_spread");
                        return Value::unknown();
                    };
                    let item = child(self.mir, e.node, Role::Value)
                        .map(|n| self.expr(key, n, env))
                        .unwrap_or_else(|_| self.opaque());
                    value.absorb(&item);
                    value.members.insert(name, item);
                }
                value.bounded()
            }
            HirKind::Field | HirKind::TupleProjection(_) => {
                let receiver = child(self.mir, node, Role::Receiver)
                    .map(|n| self.expr(key, n, env))
                    .unwrap_or_else(|_| self.opaque());
                let name = match &h.kind {
                    HirKind::TupleProjection(i) => Some(i.to_string()),
                    _ => child(self.mir, node, Role::Name).ok().and_then(|n| {
                        match &self.mir.hir[n.index()].kind {
                            HirKind::Name(n) => Some(n.clone()),
                            _ => None,
                        }
                    }),
                };
                name.map(|n| receiver.project(&n))
                    .unwrap_or_else(|| self.blocked("unresolved_field"))
            }
            HirKind::Call => self.call(key, node, env),
            HirKind::Index | HirKind::Match | HirKind::IfLet | HirKind::LetElse => {
                self.blocked("projection_or_pattern")
            }
            HirKind::Debug { .. } | HirKind::Raise(_) | HirKind::Panic => {
                self.blocked("observable_event")
            }
            _ => self.blocked("unsupported_expression"),
        }
    }
    fn fold(&mut self, callback: Closure, source: &Value, initial: &Value) -> Value {
        let item = source.element();
        if item == Value::default() {
            return initial.clone();
        } // Known empty literal.
        let mut state = initial.clone();
        for iteration in 0..8 {
            self.fold_iterations += 1;
            let event_start = self.updates.len();
            let step = self.invoke(
                callback.key,
                vec![state.clone(), item.clone()],
                (*callback.captures).clone(),
            );
            // Keep first-step update evidence, not duplicate synthetic loop
            // iterations. These records are not whole-loop write proofs.
            if iteration > 0 {
                self.updates.truncate(event_start);
            }
            let next = state.clone().join(initial.clone().join(step));
            if next == state {
                return next;
            }
            state = next;
        }
        self.fold_widenings += 1;
        self.blocked("fold_nonconvergent")
    }
    fn call(&mut self, key: Key, node: HirId, env: &mut BTreeMap<SymbolId, Value>) -> Value {
        let Ok(callee) = child(self.mir, node, Role::Callee) else {
            return self.opaque();
        };
        let args: Vec<_> = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|e| e.role == Role::Argument)
            .map(|e| self.expr(key, e.node, env))
            .collect();
        let named = symbol(self.mir, callee).ok().and_then(|s| {
            let s = &self.mir.symbols[s.index()];
            Some((
                self.mir.modules[s.module?.index()].name.clone(),
                s.name.clone(),
            ))
        });
        if let Some((module, name)) = named.as_ref() {
            match (module.as_str(), name.as_str()) {
                ("std/map", "insert" | "remove" | "merge")
                | ("std/array", "push")
                | ("std/dict", "merge") => {
                    let Some(mut v) = args.first().cloned() else {
                        return self.opaque();
                    };
                    self.updates.push(Update {
                        function: key.node.index(),
                        node: node.index(),
                        depth: self.active.len(),
                        collection: if module == "std/array" {
                            "Array"
                        } else {
                            "Map"
                        },
                        receiver: v.clone(),
                    });
                    v.updated = true;
                    if module == "std/array" {
                        let item = v
                            .element()
                            .join(args.get(1).cloned().unwrap_or_else(Value::unknown));
                        v.members.clear();
                        v.members.insert("*".into(), item.clone());
                        v.absorb(&item);
                    } else {
                        v.members.clear();
                        for arg in args.iter().skip(1) {
                            v.parameters.extend(arg.parameters.iter().copied());
                        }
                    }
                    return v.bounded();
                }
                ("std/array", "fold") if args.len() == 3 => {
                    let Some(callback) = args[2].callable.clone() else {
                        return self.blocked("dynamic_fold_callback");
                    };
                    return self.fold(callback, &args[0], &args[1]);
                }
                ("std/array", "map" | "filter" | "flat_map") if args.len() == 2 => {
                    let Some(callback) = args[1].callable.clone() else {
                        return self.blocked("dynamic_array_callback");
                    };
                    let item = args[0].element();
                    let mapped = self.invoke(
                        callback.key,
                        vec![item.clone()],
                        (*callback.captures).clone(),
                    );
                    return Value::sequence(match name.as_str() {
                        "map" => mapped,
                        "filter" => item,
                        _ => mapped.element(),
                    });
                }
                ("std/array" | "std/map" | "std/string", "length") => return Value::fresh(),
                ("std/map", "empty" | "from_pairs") => return Value::fresh(),
                ("std/map", "keys" | "values") => {
                    let mut value = args.first().cloned().unwrap_or_else(|| self.opaque());
                    value.borrowed_view = true;
                    value.members.clear();
                    value.members.insert("*".into(), Value::unknown());
                    return value;
                }
                ("std/string", "join") => {
                    if let Some(receiver) = args.first() {
                        self.updates.push(Update {
                            function: key.node.index(),
                            node: node.index(),
                            depth: self.active.len(),
                            collection: "String",
                            receiver: receiver.clone(),
                        });
                    }
                    // Join may reuse an input view (e.g. a single chunk).
                    // Keep input dependencies; construction is not uniqueness.
                    let mut value = args.first().cloned().unwrap_or_else(Value::fresh);
                    value.fresh = true;
                    value.updated = true;
                    value.callable = None;
                    value.members.clear();
                    return value;
                }
                _ => {}
            }
        }
        let function = self.expr(key, callee, env).callable;
        function
            .map(|f| self.invoke(f.key, args, (*f.captures).clone()))
            .unwrap_or_else(|| self.blocked("unresolved_or_native_call"))
    }
}

pub(crate) fn report(mir: &Mir, plan: &Plan) -> Vec<serde_json::Value> {
    let analysis = Analysis::new(mir, plan);
    plan.functions
        .keys()
        .filter(|k| k.callable && matches!(mir.hir[k.node.index()].kind, HirKind::Closure))
        .map(|&k| {
            let summary = analysis.clone().function(k);
            serde_json::json!({
                "function":k.node.index(),
                "instance":k.instance.map(|i|i.index()),
                "module":mir.modules[mir.hir[k.node.index()].module.index()].name,
                "start":mir.hir[k.node.index()].location.start,
                "summary":summary
            })
        })
        .collect()
}

/// Deliberately narrow test lowering: every syntactic call site must supply a
/// statically resolved callback. A single forwarded/unknown target blocks all
/// elision for this native instance. Bodies are never skipped optimistically.
#[cfg(test)]
pub(crate) fn scalar_callbacks_safe(mir: &Mir, plan: &Plan, name: &str) -> bool {
    let mut found = false;
    let analysis = Analysis::new(mir, plan);
    for &key in plan.functions.keys().filter(|k| k.callable) {
        let mut pending = vec![key.node];
        while let Some(node) = pending.pop() {
            let h = &mir.hir[node.index()];
            if node != key.node && matches!(h.kind, HirKind::Closure | HirKind::Interpreter) {
                continue;
            }
            if matches!(h.kind, HirKind::Call) {
                let callee = child(mir, node, Role::Callee)
                    .ok()
                    .and_then(|n| symbol(mir, n).ok());
                if callee.is_some_and(|s| {
                    mir.symbols[s.index()].name == name
                        && mir.symbols[s.index()]
                            .module
                            .is_some_and(|m| mir.modules[m.index()].name == "std/pipeline")
                }) {
                    found = true;
                    let Some(arg) = h
                        .children
                        .iter()
                        .filter(|e| e.role == Role::Argument)
                        .nth(1)
                        .map(|e| e.node)
                    else {
                        return false;
                    };
                    let Some(target) = analysis.direct(key, arg, &mut BTreeSet::new()) else {
                        return false;
                    };
                    if !analysis.clone().function(target).scalar_borrow_safe {
                        return false;
                    }
                }
            }
            pending.extend(
                h.children
                    .iter()
                    .filter(|e| {
                        !matches!(
                            e.role,
                            Role::Annotation | Role::ReturnType | Role::TypeParameter | Role::Bound
                        )
                    })
                    .map(|e| e.node),
            );
        }
    }
    found
}

#[path = "collection_callback_sites.rs"]
pub(crate) mod callback_sites;
