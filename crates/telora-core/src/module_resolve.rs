//! First MIR pass: inventory identities, reachable source syntax and import edges.
//! The source reader supplies text only, never resolved symbols or types.
use crate::ast::BindingKind;
use crate::mir::{self, *};
use crate::parser::parse_registered;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct ModuleSpec {
    pub native: Option<NativeModule>,
    /// Canonical logical name, such as `@src/main` or `std/prelude`.
    pub name: String,
    pub kind: ModuleKind,
    pub implicit_imports: Vec<String>,
}

/// Inventory is independent of reachability. IDs are allocated before any read.
pub fn resolve(
    inventory: Vec<ModuleSpec>,
    roots: &[String],
    read: impl FnMut(ModuleId, &str) -> Result<String, String>,
) -> Mir {
    resolve_with_requests(inventory, roots, read, canonical_request)
}

/// The host supplies logical naming/access policy, never another module graph.
pub fn resolve_with_requests(
    mut inventory: Vec<ModuleSpec>,
    roots: &[String],
    mut read: impl FnMut(ModuleId, &str) -> Result<String, String>,
    mut request_name: impl FnMut(&str, &str) -> Option<String>,
) -> Mir {
    inventory.sort_by(|a, b| a.name.cmp(&b.name));
    let mut mir = Mir::default();
    let mut names = BTreeMap::<String, Vec<ModuleId>>::new();
    for spec in &inventory {
        let id = ModuleId(mir.modules.len().try_into().expect("module capacity"));
        names.entry(spec.name.clone()).or_default().push(id);
        mir.modules.push(Module {
            native: spec.native.clone(),
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
    for root in &mir.roots {
        if !matches!(root, ModuleTarget::Bound(_)) {
            mir.diagnostics.push(crate::source::Diagnostic {
                severity: crate::source::Severity::Error,
                message: match root {
                    ModuleTarget::Unresolved(name) if name.starts_with("std/") => format!("unknown built-in module {name:?}"),
                    ModuleTarget::Unresolved(name) => format!("module {name:?} not found"),
                    ModuleTarget::Conflicted(_) => format!("module root has multiple inventory entries: {root:?}"),
                    ModuleTarget::Bound(_) => unreachable!(),
                },
                labels: vec![],
                notes: vec![],
            });
        }
    }
    let mut pending = mir.roots.iter().filter_map(bound).collect::<BTreeSet<_>>();
    while let Some(id) = pending.pop_first() {
        if !matches!(mir.modules[id.index()].state, ModuleState::Unloaded) {
            continue;
        }
        let spec = &inventory[id.index()];
        // Data bytes are not an input to the static phase. This source is a
        // compiler-owned interface, whose Value reference resolves normally.
        let text = match if spec.kind == ModuleKind::Data {
            Ok("import \"std/value\" { Value }; decl data: Value; export { data };".into())
        } else {
            read(id, &spec.name)
        } {
            Ok(text) => text,
            Err(message) => {
                mir.diagnostics.push(crate::source::Diagnostic {
                    severity: crate::source::Severity::Error,
                    message: format!("cannot read module {}: {message}", spec.name),
                    labels: vec![],
                    notes: vec![],
                });
                mir.modules[id.index()].state = ModuleState::Unavailable(message);
                continue;
            }
        };
        let source_name = if spec.kind == ModuleKind::Data {
            format!("{} (static contract)", spec.name)
        } else {
            spec.name.clone()
        };
        let source = mir.sources.add(source_name, text);
        let parsed = parse_registered(&mir.sources, source);
        let syntax_valid = parsed.program.is_some();
        mir.diagnostics.extend(parsed.diagnostics);
        let mut lower = mir::lower::Lower {
            mir: &mut mir,
            module: id,
            needs_display: false,
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
        lower.finish_module(body);
        mir.modules[id.index()].state = if spec.kind == ModuleKind::Data {
            ModuleState::Data { body }
        } else {
            ModuleState::Source {
                source,
                syntax_valid,
                cst: parsed.cst,
                body,
            }
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
            let target = request_name(&spec.name, &request)
                .map(|name| lookup(&names, name))
                .unwrap_or_else(|| ModuleTarget::Unresolved(request.clone()));
            if !matches!(target, ModuleTarget::Bound(_)) {
                let location = mir.hir[syntax.unwrap_or(body).index()].location;
                mir.diagnostics.push(crate::source::Diagnostic::error(
                    format!("module import {request:?} is not resolved: {target:?}"),
                    location,
                ));
            }
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

/// Apply source-module declaration rules at the workspace admission boundary.
/// Embedded compiler clients may still resolve expressions and host natives.
/// Diagnostics do not prevent subsequent symbol/type passes from filling MIR.
pub fn validate_source_modules(mir: &mut Mir, trusted: impl Fn(&str) -> bool) {
    use crate::source::Diagnostic;
    use crate::syntax::telora::ast::Program;

    for module in &mir.modules {
        let ModuleState::Source { cst, body, syntax_valid, .. } = &module.state else { continue };
        let location = mir.hir[body.index()].location;
        let authored_result = Program::root(cst).body().is_some_and(|body| body.result().is_some());
        let mut has_exports = false;
        for edge in &mir.hir[body.index()].children {
            if edge.role != Role::Binding { continue; }
            let node = &mir.hir[edge.node.index()];
            let HirKind::Binding { kind, .. } = &node.kind else { continue };
            let hidden = node.children.iter().any(|edge| edge.role == Role::Name
                && matches!(&mir.hir[edge.node.index()].kind, HirKind::Name(name) if name.starts_with('\0')));
            let message = match kind {
                BindingKind::Export => { has_exports = true; None }
                BindingKind::Let if !hidden => Some("module-level let is not supported; use def instead"),
                BindingKind::Native | BindingKind::NativeType if !trusted(&module.name) => Some("native declarations are only allowed in built-in std modules"),
                _ => None,
            };
            if let Some(message) = message {
                mir.diagnostics.push(Diagnostic::error(message, node.location));
            }
        }
        // The parser already diagnoses the explicit-export + expression case.
        if *syntax_valid && authored_result && !has_exports {
            mir.diagnostics.push(Diagnostic::error(
                "top-level expressions are not supported; bind the computation with def and export the intended result", location,
            ));
        }
        // Recovery may have dropped an export declaration. Preserve the
        // parser's outcome rather than diagnosing absence from partial HIR.
        if *syntax_valid && !has_exports {
            mir.diagnostics.push(Diagnostic::error("source module requires at least one explicit export", location));
        }
    }
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
    fn workspace_admission_retains_graph_and_checks_declarations() {
        for (source, expected) in [
            ("def value = 42; value", "top-level expressions are not supported"),
            ("let value = 42; export { value };", "module-level let is not supported"),
            ("native value: Fn() -> Int; export { value };", "only allowed in built-in std modules"),
            ("native type Value @1; export { Value };", "only allowed in built-in std modules"),
            ("def value = 42;", "requires at least one explicit export"),
        ] {
            let mut mir = resolve(vec![ModuleSpec { native: None, name: "@src/main".into(), kind: ModuleKind::Source, implicit_imports: vec![] }], &["@src/main".into()], |_, _| Ok(source.into()));
            assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
            let nodes = mir.hir.len();
            validate_source_modules(&mut mir, |_| false);
            assert!(mir.diagnostics.iter().any(|d| d.message.contains(expected)), "{:?}", mir.diagnostics);
            crate::symbol_resolve::resolve(&mut mir);
            crate::type_resolve::resolve(&mut mir);
            assert_eq!(mir.hir.len(), nodes);
        }
        let mut mir = resolve(vec![ModuleSpec { native: None, name: "std/custom".into(), kind: ModuleKind::Source, implicit_imports: vec![] }], &["std/custom".into()], |_, _| Ok("native value: Fn() -> Int; export { value };".into()));
        validate_source_modules(&mut mir, |_| true);
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        validate_source_modules(&mut mir, |_| false);
        assert!(mir.diagnostics.iter().any(|d| d.message.contains("only allowed in built-in std modules")));
    }

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
            ("std/value", "export type Value = enum { Missing };"),
        ]);
        let mut inventory = sources
            .keys()
            .map(|name| ModuleSpec {
                native: None,
                name: (*name).into(),
                kind: ModuleKind::Source,
                implicit_imports: vec![],
            })
            .collect::<Vec<_>>();
        inventory.push(ModuleSpec {
            native: None,
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
        assert_eq!(reads.len(), 5);
        assert!(!reads.contains_key("@src/data.json"));
        assert!(reads.values().all(|count| *count == 1));
        assert_eq!(mir.imports.len(), 6);
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
                native: None,
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
