//! Closed-world linear accumulator proof for internal formatter specializations.
//! Function signatures select possible targets, never prove exclusive ownership.
use crate::plan::{Key, Plan, Special, child, symbol};
use std::collections::{BTreeMap, BTreeSet};
use telora_core::mir::{HirId, HirKind, Mir, Role, SymbolId, TypeConstructor as T, TypeId};

pub(crate) fn is_fmt(mir: &Mir, ty: TypeId) -> bool {
    matches!(mir.types[ty.index()].constructor,T::Native(id) if (id.module,id.slot)==(20,1))
}
fn contains_fmt(mir: &Mir, ty: TypeId, seen: &mut BTreeSet<TypeId>) -> bool {
    if is_fmt(mir, ty) {
        return true;
    }
    if !seen.insert(ty)
        || matches!(
            mir.types[ty.index()].constructor,
            T::Function | T::Type | T::TypeOf
        )
    {
        return false;
    }
    mir.types[ty.index()]
        .arguments
        .iter()
        .any(|&a| contains_fmt(mir, a, seen))
}
fn name<'a>(mir: &'a Mir, node: HirId) -> Option<(&'a str, &'a str)> {
    let s = &mir.symbols[symbol(mir, node).ok()?.index()];
    Some((&mir.modules[s.module?.index()].name, &s.name))
}
fn seed(mir: &Mir, node: HirId) -> bool {
    child(mir, node, Role::Callee)
        .ok()
        .is_some_and(|c| name(mir, c) == Some(("std/fmt", "empty")))
}
#[derive(Default)]
pub(crate) struct Regions {
    pub functions: BTreeMap<Key, u32>,
    pub calls: BTreeSet<(Key, HirId)>,
    pub signatures: BTreeSet<TypeId>,
}
impl Regions {
    pub fn build(mir: &Mir, plan: &Plan) -> Self {
        if std::env::var_os("TELORA_DISABLE_FORMAT_REGIONS").is_some() {
            return Self::default();
        }
        let mut by_signature: BTreeMap<TypeId, Vec<Key>> = BTreeMap::new();
        let mut safe = BTreeSet::new();
        for &k in plan
            .functions
            .keys()
            .filter(|k| k.callable && k.special == Special::Normal)
        {
            let Ok(ty) = k.ty(mir, k.node) else { continue };
            let shape = &mir.types[ty.index()];
            if shape.constructor != T::Function
                || !shape.arguments.last().is_some_and(|&t| is_fmt(mir, t))
            {
                continue;
            }
            by_signature.entry(ty).or_default().push(k);
            let args = &shape.arguments[..shape.arguments.len() - 1];
            if args.iter().filter(|&&t| is_fmt(mir, t)).count() > 1 {
                continue;
            }
            if args
                .iter()
                .filter(|&&t| !is_fmt(mir, t))
                .any(|&t| contains_fmt(mir, t, &mut BTreeSet::new()))
            {
                continue;
            }
            if plan.capture_types.get(&k).is_some_and(|ts| {
                ts.iter()
                    .any(|&t| contains_fmt(mir, t, &mut BTreeSet::new()))
            }) {
                continue;
            }
            if matches!(mir.hir[k.node.index()].kind, HirKind::Closure)
                || matches!(
                    crate::natives::identity(mir, k.node),
                    Some((
                        20,
                        "write_str"
                            | "write_int"
                            | "write_float"
                            | "encode_json_string"
                            | "encode_json_float"
                    )) | Some((5, "fold"))
                )
            {
                safe.insert(k);
            }
        }
        // Greatest fixed point: recursive forwarding is safe if every body is
        // linear and every possible indirect target satisfies the same proof.
        loop {
            let signatures: BTreeSet<_> = by_signature
                .iter()
                .filter(|(_, ks)| ks.iter().all(|k| safe.contains(k)))
                .map(|(&t, _)| t)
                .collect();
            let remove: Vec<_> = safe
                .iter()
                .filter(|&&k| analyze(mir, plan, k, &signatures).is_none())
                .copied()
                .collect();
            if remove.is_empty() {
                let mut reachable = BTreeSet::new();
                for &key in plan.functions.keys() {
                    let mut pending = vec![key.node];
                    while let Some(n) = pending.pop() {
                        if matches!(mir.hir[n.index()].kind, HirKind::Call) {
                            if let Ok(c) = child(mir, n, Role::Callee) {
                                if name(mir, c) == Some(("std/fmt", "render")) {
                                    if let Ok(input) = child(mir, n, Role::Argument) {
                                        if matches!(mir.hir[input.index()].kind, HirKind::Call) {
                                            if let Ok(f) = child(mir, input, Role::Callee) {
                                                if let Ok(t) = key.ty(mir, f) {
                                                    reachable.insert(t);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if matches!(mir.hir[n.index()].kind, HirKind::InterpolatedString) {
                            for edge in &mir.hir[n.index()].children {
                                if matches!(mir.hir[edge.node.index()].kind, HirKind::Call) {
                                    if let Ok(f) = child(mir, edge.node, Role::Callee) {
                                        if let Ok(t) = key.ty(mir, f) {
                                            reachable.insert(t);
                                        }
                                    }
                                }
                            }
                        }
                        pending.extend(mir.hir[n.index()].children.iter().map(|e| e.node));
                    }
                }
                for &source in &plan.display_codecs {
                    for &ty in &signatures {
                        let args = &mir.types[ty.index()].arguments;
                        if args.len() == 3 && args[0] == source && is_fmt(mir, args[1]) {
                            reachable.insert(ty);
                        }
                    }
                }
                let mut selected = BTreeSet::new();
                loop {
                    let mut changed = false;
                    for &k in &safe {
                        let ty = k.ty(mir, k.node).unwrap();
                        if !reachable.contains(&ty) || !selected.insert(k) {
                            continue;
                        }
                        changed = true;
                        if crate::natives::identity(mir, k.node) == Some((5, "fold")) {
                            reachable.insert(mir.types[ty.index()].arguments[2]);
                        }
                        for n in analyze(mir, plan, k, &signatures).unwrap() {
                            reachable
                                .insert(k.ty(mir, child(mir, n, Role::Callee).unwrap()).unwrap());
                        }
                    }
                    if !changed {
                        break;
                    }
                }
                let mut r = Self {
                    signatures: signatures.intersection(&reachable).copied().collect(),
                    ..Self::default()
                };
                for (i, k) in selected.into_iter().enumerate() {
                    r.calls.extend(
                        analyze(mir, plan, k, &signatures)
                            .unwrap()
                            .into_iter()
                            .map(|n| (k, n)),
                    );
                    r.functions.insert(
                        k,
                        crate::abi::FIRST_FUNCTION + plan.functions.len() as u32 + 4 + i as u32,
                    );
                }
                return r;
            }
            for k in remove {
                safe.remove(&k);
            }
        }
    }
}
fn analyze(
    mir: &Mir,
    plan: &Plan,
    key: Key,
    signatures: &BTreeSet<TypeId>,
) -> Option<BTreeSet<HirId>> {
    if let Some((module, name)) = crate::natives::identity(mir, key.node) {
        if module == 20
            && matches!(
                name,
                "write_str"
                    | "write_int"
                    | "write_float"
                    | "encode_json_string"
                    | "encode_json_float"
            )
        {
            return Some(BTreeSet::new());
        }
        if (module, name) == (5, "fold") {
            let ty = key.ty(mir, key.node).ok()?;
            return signatures
                .contains(&mir.types[ty.index()].arguments[2])
                .then(BTreeSet::new);
        }
        return None;
    }
    let params: Vec<_> = mir.hir[key.node.index()]
        .children
        .iter()
        .filter(|e| e.role == Role::Parameter)
        .map(|e| e.node)
        .collect();
    let mut a = Analysis {
        mir,
        key,
        signatures,
        env: BTreeMap::new(),
        uses: BTreeMap::new(),
        calls: BTreeSet::new(),
        seeds: 0,
        remaining: 10000,
    };
    for n in params {
        if is_fmt(mir, key.ty(mir, n).ok()?) {
            a.env.insert(mir.hir_symbols[n.index()]?, true);
            a.seeds += 1;
        }
    }
    let result = a.expr(child(mir, key.node, Role::Body).ok()?)?;
    let _ = plan;
    (result && a.seeds == 1 && a.uses.values().all(|&n| n <= 1)).then_some(a.calls)
}
struct Analysis<'a> {
    mir: &'a Mir,
    key: Key,
    signatures: &'a BTreeSet<TypeId>,
    env: BTreeMap<SymbolId, bool>,
    uses: BTreeMap<SymbolId, u32>,
    calls: BTreeSet<HirId>,
    seeds: u32,
    remaining: u32,
}
impl Analysis<'_> {
    fn expr(&mut self, n: HirId) -> Option<bool> {
        self.remaining = self.remaining.checked_sub(1)?;
        let h = &self.mir.hir[n.index()];
        match &h.kind {
            HirKind::Variable(_) | HirKind::StaticPath(_) => {
                if let Ok(s) = symbol(self.mir, n) {
                    if self.env.get(&s) == Some(&true) {
                        *self.uses.entry(s).or_default() += 1;
                        return Some(true);
                    }
                }
                Some(false)
            }
            HirKind::Block => {
                let saved = self.env.clone();
                for e in &h.children {
                    if !matches!(e.role, Role::Binding | Role::Result | Role::Annotation) {
                        return None;
                    }
                    if e.role == Role::Binding {
                        if let Ok(v) = child(self.mir, e.node, Role::Value) {
                            let dep = self.expr(v)?;
                            if let Some(s) = self.mir.hir_symbols[e.node.index()] {
                                self.env.insert(s, dep);
                            }
                        }
                    }
                }
                let result = child(self.mir, n, Role::Result)
                    .ok()
                    .map(|v| self.expr(v))
                    .unwrap_or(Some(false));
                self.env = saved;
                result
            }
            HirKind::TypeAscription => self.expr(child(self.mir, n, Role::Value).ok()?),
            HirKind::Call => {
                let callee = child(self.mir, n, Role::Callee).ok()?;
                if self.expr(callee)? {
                    return None;
                }
                let args: Vec<_> = h
                    .children
                    .iter()
                    .filter(|e| e.role == Role::Argument)
                    .map(|e| e.node)
                    .collect();
                let mut deps = 0;
                for v in args {
                    deps += u32::from(self.expr(v)?);
                }
                if deps == 0 {
                    if seed(self.mir, n) {
                        self.seeds += 1;
                        return Some(true);
                    }
                    return Some(false);
                }
                if deps != 1
                    || !self
                        .signatures
                        .contains(&self.key.ty(self.mir, callee).ok()?)
                {
                    return None;
                }
                self.calls.insert(n);
                Some(true)
            }
            HirKind::Closure | HirKind::Interpreter => {
                // Capturing the accumulator would let a use escape its region.
                let mut pending = vec![n];
                while let Some(v) = pending.pop() {
                    if matches!(
                        self.mir.hir[v.index()].kind,
                        HirKind::Variable(_) | HirKind::StaticPath(_)
                    ) && symbol(self.mir, v)
                        .ok()
                        .is_some_and(|s| self.env.get(&s) == Some(&true))
                    {
                        return None;
                    }
                    pending.extend(self.mir.hir[v.index()].children.iter().map(|e| e.node));
                }
                Some(false)
            }
            HirKind::If | HirKind::Match | HirKind::MatchArm { .. } => {
                let mut any = false;
                for e in &h.children {
                    if matches!(
                        e.role,
                        Role::Annotation | Role::Pattern | Role::Name | Role::ReturnType
                    ) {
                        continue;
                    }
                    let d = self.expr(e.node)?;
                    if d && matches!(e.role, Role::Condition | Role::Subject | Role::Guard) {
                        return None;
                    }
                    any |= d;
                    // Every normally returning formatter arm must forward f.
                    if !d && matches!(e.role, Role::Then | Role::Else | Role::Body | Role::Arm) {
                        if self
                            .key
                            .ty(self.mir, e.node)
                            .ok()
                            .is_some_and(|t| is_fmt(self.mir, t))
                        {
                            return None;
                        }
                    }
                }
                Some(any)
            }
            _ => {
                for e in &h.children {
                    if matches!(
                        e.role,
                        Role::Annotation | Role::ReturnType | Role::TypeParameter | Role::Name
                    ) {
                        continue;
                    }
                    if self.expr(e.node)? {
                        return None;
                    }
                }
                Some(false)
            }
        }
    }
}
