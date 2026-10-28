//! Static callback-site coverage, separate from function-instance counts.
use super::Analysis;
use crate::plan::{Plan, child, symbol};
use std::collections::{BTreeMap, BTreeSet};
use telora_core::mir::{HirKind, Mir, Role, TypeConstructor as T};
pub(crate) fn report(
    mir: &Mir,
    plan: &Plan,
    summaries: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let safe: BTreeMap<_, _> = summaries
        .iter()
        .filter_map(|row| {
            Some((
                (
                    row["function"].as_u64()? as usize,
                    row["instance"].as_u64().map(|i| i as usize),
                ),
                row["summary"]["scalar_borrow_safe"].as_bool()?,
            ))
        })
        .collect();
    let analysis = Analysis::new(mir, plan);
    let mut rows = vec![];
    for &key in plan
        .functions
        .keys()
        .filter(|k| k.callable && matches!(mir.hir[k.node.index()].kind, HirKind::Closure))
    {
        let mut pending = vec![key.node];
        while let Some(node) = pending.pop() {
            let h = &mir.hir[node.index()];
            if node != key.node && matches!(h.kind, HirKind::Closure | HirKind::Interpreter) {
                continue;
            }
            if matches!(h.kind, HirKind::Call) {
                let named = child(mir, node, Role::Callee)
                    .ok()
                    .and_then(|n| symbol(mir, n).ok())
                    .and_then(|s| {
                        let s = &mir.symbols[s.index()];
                        (mir.modules[s.module?.index()].name == "std/array")
                            .then_some(s.name.as_str())
                    });
                if let Some(
                    operation @ ("map" | "filter" | "flat_map" | "fold" | "fold_control" | "any"
                    | "all" | "find"),
                ) = named
                {
                    let position = if matches!(operation, "fold" | "fold_control") {
                        2
                    } else {
                        1
                    };
                    let target = h
                        .children
                        .iter()
                        .filter(|e| e.role == Role::Argument)
                        .nth(position)
                        .and_then(|e| analysis.direct(key, e.node, &mut BTreeSet::new()));
                    let scalar_safe = target.is_some_and(|t| {
                        safe.get(&(t.node.index(), t.instance.map(|i| i.index()))) == Some(&true)
                    });
                    let input = target
                        .and_then(|t| t.ty(mir, t.node).ok())
                        .and_then(|t| mir.types[t.index()].arguments.first())
                        .map(|t| match mir.types[t.index()].constructor {
                            T::Int | T::Float | T::Bool => "scalar",
                            _ => "aggregate_or_other",
                        });
                    rows.push(serde_json::json!({"function":key.node.index(),"instance":key.instance.map(|i|i.index()),"node":node.index(),"module":mir.modules[h.module.index()].name,"operation":operation,"callback_function":target.map(|t|t.node.index()),"callback_instance":target.and_then(|t|t.instance.map(|i|i.index())),"scalar_result_borrow_safe":scalar_safe,"callback_input":input}));
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
    rows
}
