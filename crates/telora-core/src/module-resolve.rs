//! First MIR pass: inventory identities, reachable source syntax and import edges.
//! The source reader supplies text only, never resolved symbols or types.
use crate::ast::BindingKind;
use crate::mir::{self, *};
use crate::parser::parse_registered;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct ModuleSpec {
    /// Canonical logical name, such as `@src/main` or `std/prelude`.
    pub name: String,
    pub kind: ModuleKind,
    pub implicit_imports: Vec<String>,
}

/// Inventory is independent of reachability. IDs are allocated before any read.
pub fn resolve(
    mut inventory: Vec<ModuleSpec>,
    roots: &[String],
    mut read: impl FnMut(ModuleId, &str) -> Result<String, String>,
) -> Mir {
    inventory.sort_by(|a, b| a.name.cmp(&b.name));
    let mut mir = Mir::default();
    let mut names = BTreeMap::<String, Vec<ModuleId>>::new();
    for spec in &inventory {
        let id = ModuleId(mir.modules.len().try_into().expect("module capacity"));
        names.entry(spec.name.clone()).or_default().push(id);
        mir.modules.push(Module {
            name: spec.name.clone(),
            kind: spec.kind,
            state: ModuleState::Unloaded,
            imports: vec![],
        });
    }
    mir.roots = roots
        .iter()
        .map(|root| lookup(&names, root.clone()))
        .collect();
    let mut pending = mir.roots.iter().filter_map(bound).collect::<BTreeSet<_>>();
    while let Some(id) = pending.pop_first() {
        if !matches!(mir.modules[id.index()].state, ModuleState::Unloaded) {
            continue;
        }
        let spec = &inventory[id.index()];
        if spec.kind == ModuleKind::Data {
            // Neither the contents nor the syntax of a data file enter this pass.
            mir.modules[id.index()].state = ModuleState::Data;
            continue;
        }
        let text = match read(id, &spec.name) {
            Ok(text) => text,
            Err(message) => {
                mir.modules[id.index()].state = ModuleState::Unavailable(message);
                continue;
            }
        };
        let source = mir.sources.add(spec.name.clone(), text);
        let parsed = parse_registered(&mir.sources, source);
        mir.diagnostics.extend(parsed.diagnostics);
        let mut lower = mir::lower::Lower {
            mir: &mut mir,
            module: id,
        };
        let body = match parsed.program {
            Some(program) => lower.body(
                program.value.body.location,
                program.value.body.value.bindings,
                Some(*program.value.body.value.result),
            ),
            None => lower.body(
                parsed.recovered.location,
                parsed.recovered.bindings,
                parsed.recovered.result,
            ),
        };
        mir.modules[id.index()].state = ModuleState::Source {
            source,
            cst: parsed.cst,
            body,
        };
        let mut requests = spec
            .implicit_imports
            .iter()
            .cloned()
            .map(|name| (None, name))
            .collect::<Vec<_>>();
        for edge in &mir.hir[body.index()].children {
            if edge.role != Role::Binding {
                continue;
            }
            let binding = &mir.hir[edge.node.index()];
            if !matches!(
                binding.kind,
                HirKind::Binding {
                    kind: BindingKind::Import | BindingKind::OpenImport,
                    ..
                }
            ) {
                continue;
            }
            let value = binding
                .children
                .iter()
                .find(|edge| edge.role == Role::Value)
                .expect("binding value");
            if let HirKind::String(request) = &mir.hir[value.node.index()].kind {
                requests.push((Some(edge.node), request.clone()));
            }
        }
        for (syntax, request) in requests {
            let target = canonical_request(&spec.name, &request)
                .map(|name| lookup(&names, name))
                .unwrap_or_else(|| ModuleTarget::Unresolved(request.clone()));
            if let Some(target) = bound(&target) {
                pending.insert(target);
            }
            let edge = mir.imports.len();
            mir.imports.push(Import {
                owner: id,
                syntax,
                request,
                target,
            });
            mir.modules[id.index()].imports.push(edge);
        }
    }
    mir
}

fn bound(target: &ModuleTarget) -> Option<ModuleId> {
    if let ModuleTarget::Bound(id) = target {
        Some(*id)
    } else {
        None
    }
}

fn lookup(names: &BTreeMap<String, Vec<ModuleId>>, name: String) -> ModuleTarget {
    match names.get(&name).map(Vec::as_slice) {
        None | Some([]) => ModuleTarget::Unresolved(name),
        Some([id]) => ModuleTarget::Bound(*id),
        Some(ids) => ModuleTarget::Conflicted(ids.to_vec()),
    }
}

fn canonical_request(owner: &str, request: &str) -> Option<String> {
    if !request.starts_with("./") && !request.starts_with("../") {
        return Some(request.to_owned());
    }
    let mut path = owner.split('/').collect::<Vec<_>>();
    path.pop();
    for part in request.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                if path.len() <= 1 {
                    return None;
                }
                path.pop();
            }
            part => path.push(part),
        }
    }
    Some(path.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attaches_shared_syntax_once_and_never_reads_data_or_unreachable_sources() {
        let sources = BTreeMap::from([
            (
                "@src/main",
                "import \"./left\" *; import \"./right\" *; import \"./data.json\" { data }; export def f = fn(x) { x };",
            ),
            (
                "@src/left",
                "import \"./shared\" *; export def left = shared;",
            ),
            (
                "@src/right",
                "import \"./shared\" *; export def right = shared;",
            ),
            ("@src/shared", "export def shared = 1;"),
            ("@src/unused", "invalid unused text"),
        ]);
        let mut inventory = sources
            .keys()
            .map(|name| ModuleSpec {
                name: (*name).into(),
                kind: ModuleKind::Source,
                implicit_imports: vec![],
            })
            .collect::<Vec<_>>();
        inventory.push(ModuleSpec {
            name: "@src/data.json".into(),
            kind: ModuleKind::Data,
            implicit_imports: vec![],
        });
        let mut reads = BTreeMap::new();
        let mir = resolve(inventory, &["@src/main".into()], |_, name| {
            *reads.entry(name.to_owned()).or_insert(0) += 1;
            Ok(sources[name].to_owned())
        });
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        assert_eq!(reads.len(), 4);
        assert!(reads.values().all(|count| *count == 1));
        assert_eq!(mir.imports.len(), 5);
        assert_eq!(mir.hir.len(), mir.ty_slots.len());
        assert!(mir.ty_slots.iter().all(|slot| *slot == TypeState::Unknown));
        assert!(!mir.resolve_slots.is_empty());
        assert!(
            mir.resolve_slots
                .iter()
                .all(|slot| *slot == ResolveState::Pending)
        );
        assert!(
            mir.hir
                .iter()
                .any(|node| matches!(node.kind, HirKind::ReturnType))
        );
        let dump = mir.dump();
        assert!(dump.contains("CST attached"));
        assert!(dump.contains("data (static export: data: Value)"));
        assert!(dump.contains("Pending"));
        assert_eq!(dump, mir.dump());
    }

    #[test]
    fn retains_cycles_missing_targets_and_duplicate_inventory_candidates() {
        let mut inventory = ["@src/a", "@src/b", "@src/duplicate", "@src/duplicate"]
            .into_iter()
            .map(|name| ModuleSpec {
                name: name.into(),
                kind: ModuleKind::Source,
                implicit_imports: vec![],
            })
            .collect::<Vec<_>>();
        inventory.reverse();
        let mir = resolve(inventory, &["@src/a".into()], |_, name| {
            Ok(match name {
            "@src/a" => "import \"./b\" *; import \"./missing\" *; import \"./duplicate\" *; export def a = 1;",
            "@src/b" => "import \"./a\" *; export def b = 2;",
            _ => panic!("ambiguous module must not be chosen"),
        }.into())
        });
        assert_eq!(mir.imports.len(), 4);
        assert!(
            mir.imports
                .iter()
                .any(|edge| matches!(edge.target, ModuleTarget::Unresolved(_)))
        );
        assert!(
            mir.imports.iter().any(
                |edge| matches!(&edge.target, ModuleTarget::Conflicted(ids) if ids.len() == 2)
            )
        );
        assert_eq!(
            mir.modules
                .iter()
                .filter(|module| matches!(module.state, ModuleState::Source { .. }))
                .count(),
            2
        );
    }
}
