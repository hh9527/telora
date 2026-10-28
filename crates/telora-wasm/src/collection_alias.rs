//! Exploratory local transfer analysis. This reports candidates only: a single
//! use of a value head is NOT a license to mutate shared backing storage.
use crate::plan::{Key, Plan, child, symbol};
use std::collections::{BTreeMap, BTreeSet};
use telora_core::{
    mir::{HirId, HirKind, Mir, Role, SymbolId},
    syntax::kinds::BindingKind,
};

#[derive(Debug, serde::Serialize)]
pub(crate) struct Candidate {
    function: usize,
    node: usize,
    collection: &'static str,
    target_backend: &'static str,
    module: String,
    start: u32,
    end: u32,
    pub(crate) decision: &'static str,
}
struct Local<'a> {
    mir: &'a Mir,
    bindings: BTreeMap<SymbolId, HirId>,
    uses: BTreeMap<SymbolId, usize>,
}
impl Local<'_> {
    fn named(&self, node: HirId) -> Option<(&str, &str)> {
        let node = child(self.mir, node, Role::Callee).ok()?;
        let symbol = &self.mir.symbols[symbol(self.mir, node).ok()?.index()];
        Some((&self.mir.modules[symbol.module?.index()].name, &symbol.name))
    }
    fn receiver(&self, node: HirId) -> Option<(&'static str, HirId)> {
        match self.mir.hir[node.index()].kind {
            HirKind::Call => {
                let kind = match self.named(node)? {
                    ("std/map", "insert" | "remove" | "merge") => "Map",
                    ("std/array", "push") => "Array",
                    ("std/dict", "merge") => "Map",
                    ("std/string", "replace" | "ensure_trailing_newline" | "trim_margin") => {
                        "String"
                    }
                    ("std/string", "join") => {
                        let items = child(self.mir, node, Role::Argument).ok()?;
                        if !matches!(self.mir.hir[items.index()].kind, HirKind::Array) {
                            return None;
                        }
                        return Some(("String", child(self.mir, items, Role::Item).ok()?));
                    }
                    _ => return None,
                };
                Some((kind, child(self.mir, node, Role::Argument).ok()?))
            }
            _ => None,
        }
    }
    fn transfer(&self, mut node: HirId) -> &'static str {
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(node) {
                return "recursive_or_unknown";
            }
            match self.mir.hir[node.index()].kind {
                HirKind::Array | HirKind::Dict => return "local_transfer_candidate",
                HirKind::TypeAscription => {
                    let Ok(value) = child(self.mir, node, Role::Value) else {
                        return "unknown";
                    };
                    node = value;
                }
                HirKind::Variable(_) | HirKind::StaticPath(_) => {
                    let Ok(symbol) = symbol(self.mir, node) else {
                        return "unresolved";
                    };
                    let Some(&value) = self.bindings.get(&symbol) else {
                        return "parameter_capture_or_global";
                    };
                    if self.uses.get(&symbol) != Some(&1) {
                        return "multiple_uses_or_escape";
                    }
                    node = value;
                }
                HirKind::Call
                    if matches!(
                        self.named(node),
                        Some(("std/map", "empty" | "from_pairs") | ("std/string", "join"))
                    ) =>
                {
                    return "local_transfer_candidate";
                }
                _ => {
                    if let Some((_, receiver)) = self.receiver(node) {
                        node = receiver;
                    } else {
                        return "unknown_or_shared_storage";
                    }
                }
            }
        }
    }
}

pub(crate) fn analyze(mir: &Mir, key: Key) -> Vec<Candidate> {
    if !key.callable || !matches!(mir.hir[key.node.index()].kind, HirKind::Closure) {
        return vec![];
    }
    let Ok(body) = child(mir, key.node, Role::Body) else {
        return vec![];
    };
    let mut local = Local {
        mir,
        bindings: BTreeMap::new(),
        uses: BTreeMap::new(),
    };
    let mut pending = vec![(body, false)];
    let mut nodes = vec![];
    while let Some((node, nested)) = pending.pop() {
        let hir = &mir.hir[node.index()];
        // Include nested references in use counts so closure capture blocks a
        // transfer, but analyze each nested function in its own lexical scope.
        let nested = nested || matches!(hir.kind, HirKind::Closure | HirKind::Interpreter);
        if matches!(hir.kind, HirKind::Variable(_) | HirKind::StaticPath(_)) {
            if let Ok(symbol) = symbol(mir, node) {
                *local.uses.entry(symbol).or_default() += 1;
            }
        }
        if !nested {
            nodes.push(node);
            if matches!(
                hir.kind,
                HirKind::Binding {
                    kind: BindingKind::Let,
                    ..
                }
            ) {
                if let (Some(symbol), Ok(value)) =
                    (mir.hir_symbols[node.index()], child(mir, node, Role::Value))
                {
                    local.bindings.insert(symbol, value);
                }
            }
        }
        pending.extend(
            hir.children
                .iter()
                .filter(|e| !matches!(e.role, Role::Annotation | Role::ReturnType))
                .map(|e| (e.node, nested)),
        );
    }
    nodes
        .into_iter()
        .filter_map(|node| {
            let (collection, receiver) = local.receiver(node)?;
            Some(Candidate {
                function: key.node.index(),
                node: node.index(),
                collection,
                target_backend: match collection {
                    "Map" => "MapMut",
                    "Array" => "ArrayMut",
                    "String" => "StringMut",
                    _ => unreachable!(),
                },
                module: mir.modules[mir.hir[node.index()].module.index()]
                    .name
                    .clone(),
                start: mir.hir[node.index()].location.start,
                end: mir.hir[node.index()].location.end,
                decision: local.transfer(receiver),
            })
        })
        .collect()
}

pub(crate) fn write_report(mir: &Mir, plan: &Plan) -> Result<(), String> {
    let Some(path) = std::env::var_os("TELORA_EXPERIMENTAL_ALIAS_REPORT") else {
        return Ok(());
    };
    let candidates: Vec<_> = plan
        .functions
        .keys()
        .flat_map(|&key| analyze(mir, key))
        .collect();
    let summaries = crate::collection_summaries::report(mir, plan);
    let callback_sites = crate::collection_summaries::callback_sites::report(mir, plan, &summaries);
    let report = serde_json::json!({"version":3,"function_summaries":summaries,"callback_sites":callback_sites,"mode":"analysis_only","region_lowering_implemented":false,"candidates":candidates});
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write alias report: {e}"))
}
