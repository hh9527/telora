//! First MIR pass: inventory identities, reachable source syntax and import edges.
//! The source reader supplies text only, never resolved symbols or types.
use crate::hir_lower;
use crate::mir::*;
use crate::syntax::kinds::BindingKind;
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
    inventory: Vec<ModuleSpec>,
    roots: &[String],
    read: impl FnMut(ModuleId, &str) -> Result<String, String>,
    request_name: impl FnMut(&str, &str) -> Option<String>,
) -> Mir {
    resolve_with_requests_cancellable(inventory, roots, read, request_name, &mut || false)
        .expect("uncancelled module graph")
}

/// Cancellation discards the in-progress graph. Syntax errors remain ordinary
/// diagnostics; cancellation is not an unresolved module or a syntax error.
pub fn resolve_with_requests_cancellable(
    inventory: Vec<ModuleSpec>,
    roots: &[String],
    read: impl FnMut(ModuleId, &str) -> Result<String, String>,
    request_name: impl FnMut(&str, &str) -> Option<String>,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<Mir> {
    resolve_with_discovery_cancellable(inventory, roots, read, request_name, |_, _| None, cancelled)
}

pub fn resolve_with_discovery_cancellable(
    mut inventory: Vec<ModuleSpec>,
    roots: &[String],
    mut read: impl FnMut(ModuleId, &str) -> Result<String, String>,
    mut request_name: impl FnMut(&str, &str) -> Option<String>,
    mut discover: impl FnMut(&str, &str) -> Option<ModuleSpec>,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<Mir> {
    if cancelled() {
        return None;
    }
    inventory.sort_by(|a, b| a.name.cmp(&b.name));
    let mut mir = Mir::default();
    let mut names = BTreeMap::<String, Vec<ModuleId>>::new();
    for spec in &inventory {
        if cancelled() {
            return None;
        }
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
                    ModuleTarget::Unresolved(name) if name.starts_with("std/") => {
                        format!("unknown built-in module {name:?}")
                    }
                    ModuleTarget::Unresolved(name) => format!("module {name:?} not found"),
                    ModuleTarget::Conflicted(_) => {
                        format!("module root has multiple inventory entries: {root:?}")
                    }
                    ModuleTarget::Bound(_) => unreachable!(),
                },
                labels: vec![],
                notes: vec![],
            });
        }
    }
    let mut pending = mir.roots.iter().filter_map(bound).collect::<BTreeSet<_>>();
    let mut demanded = BTreeSet::<String>::new();
    while let Some(id) = pending.pop_first() {
        if cancelled() {
            return None;
        }
        if !matches!(mir.modules[id.index()].state, ModuleState::Unloaded) {
            continue;
        }
        let spec_name = inventory[id.index()].name.clone();
        let spec_kind = inventory[id.index()].kind;
        let implicit_imports = inventory[id.index()].implicit_imports.clone();
        // Data bytes are not an input to the static phase. This source is a
        // compiler-owned interface, whose Value reference resolves normally.
        let text = match if spec_kind == ModuleKind::Data {
            Ok("use std::value::{ Value }; decl data: Value; pub use self::{ data };".into())
        } else {
            read(id, &spec_name)
        } {
            Ok(text) => text,
            Err(message) => {
                mir.diagnostics.push(crate::source::Diagnostic {
                    severity: crate::source::Severity::Error,
                    message: format!("cannot read module {spec_name}: {message}"),
                    labels: vec![],
                    notes: vec![],
                });
                mir.modules[id.index()].state = ModuleState::Unavailable(message);
                continue;
            }
        };
        let source_name = if spec_kind == ModuleKind::Data {
            format!("{spec_name} (static contract)")
        } else {
            spec_name.clone()
        };
        let source = match mir.sources.try_add(source_name, text) {
            Ok(source) => source,
            Err(error) => {
                let message = format!("cannot register module {spec_name}: {error}");
                mir.diagnostics.push(crate::source::Diagnostic {
                    severity: crate::source::Severity::Error,
                    message: message.clone(),
                    labels: vec![],
                    notes: vec![],
                });
                mir.modules[id.index()].state = ModuleState::Unavailable(message);
                continue;
            }
        };
        if cancelled() {
            return None;
        }
        let parsed = crate::syntax::telora::parse_document_cancellable(
            source,
            mir.sources
                .get(source)
                .text()
                .document()
                .expect("code source"),
            cancelled,
        )?;
        if cancelled() {
            return None;
        }
        let lowered = hir_lower::lower_module(&mut mir, id, source, &parsed.syntax);
        if cancelled() {
            return None;
        }
        let syntax_valid = parsed.diagnostics.is_empty() && lowered.diagnostics.is_empty();
        mir.diagnostics.extend(parsed.diagnostics);
        mir.diagnostics.extend(lowered.diagnostics);
        let body = lowered.body;
        mir.modules[id.index()].state = if spec_kind == ModuleKind::Data {
            ModuleState::Data { body }
        } else {
            ModuleState::Source {
                source,
                syntax_valid,
                cst: parsed.syntax,
                body,
            }
        };
        let mut requests = implicit_imports
            .iter()
            .cloned()
            .map(|name| (None, name))
            .collect::<Vec<_>>();
        let static_roots = mir
            .hir
            .iter()
            .filter(|node| node.module == id)
            .flat_map(|node| match &node.kind {
                HirKind::StaticPath(path) if path.first().is_some_and(|part| part == "std") => {
                    let mut roots = vec!["std".to_owned()];
                    if let Some(module) = path.get(1) {
                        roots.push(format!("std/{module}"));
                    }
                    roots
                }
                HirKind::StaticPath(path)
                    if !matches!(
                        path.first().map(String::as_str),
                        Some("crate" | "self" | "super")
                    ) =>
                {
                    path.first().cloned().into_iter().collect()
                }
                _ => vec![],
            })
            .collect::<BTreeSet<_>>();
        for root in static_roots {
            demanded.insert(root.clone());
            if let Some(target) = names
                .get(&root)
                .and_then(|targets| (targets.len() == 1).then_some(targets[0]))
            {
                pending.insert(target);
            }
        }
        for edge in &mir.hir[body.index()].children {
            if edge.role != Role::Binding {
                continue;
            }
            let binding = &mir.hir[edge.node.index()];
            if !matches!(
                binding.kind,
                HirKind::Binding {
                    kind: BindingKind::Import,
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
            let resolved = request_name(&spec_name, &request);
            if let Some(name) = &resolved
                && !names.contains_key(name)
                && let Some(spec) = discover(&spec_name, name)
            {
                let target = ModuleId(mir.modules.len().try_into().expect("module capacity"));
                names.entry(spec.name.clone()).or_default().push(target);
                mir.modules.push(Module {
                    native: spec.native.clone(),
                    name: spec.name.clone(),
                    kind: spec.kind,
                    state: ModuleState::Unloaded,
                    imports: vec![],
                });
                inventory.push(spec);
            }
            let demanded_target = resolved
                .as_ref()
                .is_some_and(|name| demanded.contains(name));
            let target = resolved
                .as_ref()
                .map(|name| lookup(&names, name.clone()))
                .unwrap_or_else(|| ModuleTarget::Unresolved(request.clone()));
            if !matches!(target, ModuleTarget::Bound(_)) {
                let location = mir.hir[syntax.unwrap_or(body).index()].location;
                mir.diagnostics.push(crate::source::Diagnostic::error(
                    format!("module import {request:?} is not resolved: {target:?}"),
                    location,
                ));
            }
            let lazy_std_child = spec_name == "std"
                && syntax.is_some_and(|binding| {
                    matches!(
                        mir.hir[binding.index()].kind,
                        HirKind::Binding {
                            kind: BindingKind::Import,
                            imported: None,
                            ..
                        }
                    )
                });
            if let Some(target) = bound(&target)
                && (!lazy_std_child || demanded_target)
            {
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
    if cancelled() { None } else { Some(mir) }
}

/// Apply source-module declaration rules at the workspace admission boundary.
/// Embedded compiler clients may still resolve expressions and host natives.
/// Diagnostics do not prevent subsequent symbol/type passes from filling MIR.
pub fn validate_source_modules(mir: &mut Mir, trusted: impl Fn(&str) -> bool) {
    use crate::source::Diagnostic;
    use crate::syntax::telora::ast::{AstNode, Expr, Program};

    for module in &mir.modules {
        let ModuleState::Source {
            cst,
            body,
            syntax_valid,
            ..
        } = &module.state
        else {
            continue;
        };
        let location = mir.hir[body.index()].location;
        let authored_result = Program::root(cst).body().is_some_and(|body| {
            body.syntax()
                .children()
                .any(|child| Expr::cast(cst, child.node_ref()).is_some())
        });
        let mut has_exports = false;
        for edge in &mir.hir[body.index()].children {
            if edge.role != Role::Binding {
                continue;
            }
            let node = &mir.hir[edge.node.index()];
            let HirKind::Binding { kind, .. } = &node.kind else {
                continue;
            };
            let hidden = node.children.iter().any(|edge| edge.role == Role::Name
                && matches!(&mir.hir[edge.node.index()].kind, HirKind::Name(name) if name.starts_with('\0')));
            let message = match kind {
                BindingKind::Export => {
                    has_exports = true;
                    None
                }
                BindingKind::Let if !hidden => {
                    Some("module-level let is not supported; use def instead")
                }
                BindingKind::Native | BindingKind::NativeType if !trusted(&module.name) => {
                    Some("native declarations are only allowed in built-in std modules")
                }
                _ => None,
            };
            if let Some(message) = message {
                mir.diagnostics
                    .push(Diagnostic::error(message, node.location));
            }
        }
        // Admission checks the authored module shape only after syntax is valid.
        if *syntax_valid && authored_result {
            mir.diagnostics.push(Diagnostic::error(
                "top-level expressions are not supported; bind the computation with def and export the intended result", location,
            ));
        }
        // Recovery may have dropped an export declaration. Preserve the
        // parser's outcome rather than diagnosing absence from partial HIR.
        if *syntax_valid && !has_exports {
            mir.diagnostics.push(Diagnostic::error(
                "source module requires at least one explicit export",
                location,
            ));
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
    #[test]
    fn cancellation_discards_the_module_graph() {
        let token = crate::query::CancellationToken::default();
        let mut reads = 0;
        let mir = super::resolve_with_requests_cancellable(
            vec![super::ModuleSpec {
                native: None,
                name: "app/main".into(),
                kind: crate::mir::ModuleKind::Source,
                implicit_imports: vec![],
            }],
            &["app/main".into()],
            |_, _| {
                reads += 1;
                token.cancel();
                Ok("1".into())
            },
            super::canonical_request,
            &mut || token.is_cancelled(),
        );
        assert_eq!(reads, 1);
        assert!(mir.is_none());
    }

    #[test]
    fn cname_inventory_needs_no_filesystem_and_ids_ignore_inventory_order() {
        use super::*;
        let build = |reverse: bool| {
            let mut inventory = ["app/bin/main", "dep", "dep/helper"]
                .into_iter()
                .map(|name| ModuleSpec {
                    native: None,
                    name: name.into(),
                    kind: ModuleKind::Source,
                    implicit_imports: vec![],
                })
                .collect::<Vec<_>>();
            if reverse {
                inventory.reverse();
            }
            let mut reads = Vec::new();
            let mir = resolve(inventory, &["app/bin/main".into()], |_, cname| {
                reads.push(cname.to_owned());
                Ok(match cname {
                    "app/bin/main" => "use dep::value; pub use self::{ value };",
                    "dep" => "mod helper; pub use self::helper::value;",
                    "dep/helper" => "pub def value = 42;",
                    _ => panic!("unexpected source request: {cname}"),
                }
                .into())
            });
            assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
            assert_eq!(reads, ["app/bin/main", "dep", "dep/helper"]);
            assert!(mir.modules.iter().all(|module| {
                module
                    .imports
                    .iter()
                    .all(|edge| matches!(mir.imports[*edge].target, ModuleTarget::Bound(_)))
            }));
            mir.modules
                .into_iter()
                .map(|module| module.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(build(false), build(true));
    }

    #[test]
    fn module_declarations_create_child_namespace_edges_without_path_imports() {
        let inventory = ["app", "app/query", "dep", "dep/item"]
            .into_iter()
            .map(|name| ModuleSpec {
                native: None,
                name: name.into(),
                kind: ModuleKind::Source,
                implicit_imports: vec![],
            })
            .collect();
        let mut reads = Vec::new();
        let mut mir = resolve(inventory, &["app".into()], |_, name| {
            reads.push(name.to_owned());
            Ok(match name {
                "app" => "mod query; pub def base = 42; use self::query::answer; use dep::item::{answer as dep_answer}; pub use self::{ answer, dep_answer };",
                "app/query" => "pub def answer = crate::base;",
                "dep" => "mod item; pub use self::{ item };",
                "dep/item" => "pub def answer = 7;",
                _ => unreachable!(),
            }
            .into())
        });
        assert_eq!(reads, ["app", "app/query", "dep", "dep/item"]);
        assert_eq!(mir.imports.len(), 2);
        assert_eq!(mir.imports[0].request, "app/query");
        assert!(matches!(mir.imports[0].target, ModuleTarget::Bound(_)));
        crate::symbol_resolve::resolve(&mut mir);
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let paths: Vec<_> = mir
            .hir
            .iter()
            .filter(|node| matches!(node.kind, HirKind::StaticPath(_)))
            .collect();
        assert_eq!(paths.len(), 3);
        assert!(paths.iter().all(|node| matches!(
            mir.resolve_slots[node.resolution.unwrap().index()],
            ResolveState::Bound(_)
        )));
    }

    #[test]
    fn declarations_discover_only_the_reachable_module_tree() {
        let sources = BTreeMap::from([
            (
                "app",
                "mod query; data config = import(json) \"config.json\"; pub use self::{ query, config };",
            ),
            ("app/query", "mod parser; pub use self::{ parser };"),
            ("app/query/parser", "pub def answer = 42;"),
            ("app/unmounted", "pub def hidden = 0;"),
            ("std/value", "pub type Value = enum { Missing };"),
        ]);
        let mut reads = Vec::new();
        let mut discoveries = Vec::new();
        let mir = resolve_with_discovery_cancellable(
            vec![
                ModuleSpec {
                    native: None,
                    name: "app".into(),
                    kind: ModuleKind::Source,
                    implicit_imports: vec![],
                },
                ModuleSpec {
                    native: None,
                    name: "std/value".into(),
                    kind: ModuleKind::Source,
                    implicit_imports: vec![],
                },
            ],
            &["app".into()],
            |_, name| {
                reads.push(name.to_owned());
                sources
                    .get(name)
                    .map(|source| (*source).to_owned())
                    .ok_or_else(|| format!("unexpected source request: {name}"))
            },
            canonical_request,
            |_, name| {
                discoveries.push(name.to_owned());
                Some(ModuleSpec {
                    native: None,
                    name: name.to_owned(),
                    kind: if name.ends_with(".json") {
                        ModuleKind::Data
                    } else {
                        ModuleKind::Source
                    },
                    implicit_imports: vec![],
                })
            },
            &mut || false,
        )
        .unwrap();

        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        assert_eq!(
            mir.modules
                .iter()
                .map(|module| module.name.as_str())
                .collect::<Vec<_>>(),
            [
                "app",
                "std/value",
                "app/query",
                "app/config.json",
                "app/query/parser"
            ]
        );
        assert_eq!(
            discoveries,
            ["app/query", "app/config.json", "app/query/parser"]
        );
        assert_eq!(reads, ["app", "app/query", "std/value", "app/query/parser"]);
        assert!(!reads.iter().any(|name| name == "app/unmounted"));
    }

    #[test]
    fn std_root_admits_children_but_reads_only_demanded_modules() {
        let sources = BTreeMap::from([
            ("app", "use std::used::{ value }; pub use self::{ value };"),
            ("std", "pub mod used; pub mod unused;"),
            ("std/used", "pub def value: Int = 42;"),
            ("std/unused", "pub def value: Int = 0;"),
        ]);
        let mut reads = Vec::new();
        let mir = resolve_with_discovery_cancellable(
            vec![
                ModuleSpec {
                    native: None,
                    name: "app".into(),
                    kind: ModuleKind::Source,
                    implicit_imports: vec![],
                },
                ModuleSpec {
                    native: None,
                    name: "std".into(),
                    kind: ModuleKind::Source,
                    implicit_imports: vec![],
                },
            ],
            &["app".into()],
            |_, name| {
                reads.push(name.to_owned());
                Ok(sources[name].to_owned())
            },
            canonical_request,
            |_, name| {
                sources.get(name).map(|_| ModuleSpec {
                    native: None,
                    name: name.to_owned(),
                    kind: ModuleKind::Source,
                    implicit_imports: vec![],
                })
            },
            &mut || false,
        )
        .unwrap();

        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        assert_eq!(reads, ["app", "std", "std/used"]);
        assert!(mir.modules.iter().any(|module| {
            module.name == "std/unused" && matches!(module.state, ModuleState::Unloaded)
        }));
    }

    #[test]
    fn source_line_count_is_not_limited_to_u16() {
        let mir = super::resolve(
            vec![super::ModuleSpec {
                native: None,
                name: "@src/main".into(),
                kind: super::ModuleKind::Source,
                implicit_imports: vec![],
            }],
            &["@src/main".into()],
            |_, _| Ok("\n".repeat(65536)),
        );
        assert!(!matches!(
            mir.modules[0].state,
            super::ModuleState::Unavailable(_)
        ));
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    }

    use super::*;

    #[test]
    fn workspace_admission_retains_graph_and_checks_declarations() {
        for (source, expected) in [
            (
                "def value = 42; value",
                "top-level expressions are not supported",
            ),
            (
                "let value = 42; pub use self::{ value };",
                "module-level let is not supported",
            ),
            (
                "native value: Fn() -> Int; pub use self::{ value };",
                "only allowed in built-in std modules",
            ),
            (
                "native type Value @1; pub use self::{ Value };",
                "only allowed in built-in std modules",
            ),
            ("def value = 42;", "requires at least one explicit export"),
        ] {
            let mut mir = resolve(
                vec![ModuleSpec {
                    native: None,
                    name: "@src/main".into(),
                    kind: ModuleKind::Source,
                    implicit_imports: vec![],
                }],
                &["@src/main".into()],
                |_, _| Ok(source.into()),
            );
            assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
            let nodes = mir.hir.len();
            validate_source_modules(&mut mir, |_| false);
            assert!(
                mir.diagnostics.iter().any(|d| d.message.contains(expected)),
                "{:?}",
                mir.diagnostics
            );
            crate::symbol_resolve::resolve(&mut mir);
            crate::type_resolve::resolve(&mut mir);
            assert_eq!(mir.hir.len(), nodes);
        }
        let mut mir = resolve(
            vec![ModuleSpec {
                native: None,
                name: "std/custom".into(),
                kind: ModuleKind::Source,
                implicit_imports: vec![],
            }],
            &["std/custom".into()],
            |_, _| Ok("native value: Fn() -> Int; pub use self::{ value };".into()),
        );
        validate_source_modules(&mut mir, |_| true);
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        validate_source_modules(&mut mir, |_| false);
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.contains("only allowed in built-in std modules"))
        );
    }

    #[test]
    fn attaches_shared_syntax_once_and_never_reads_data_or_unreachable_sources() {
        let sources = BTreeMap::from([
            (
                "@src/main",
                "mod left; mod right; mod shared; data data = import(json) \"./data.json\"; pub def f = fn(x) { x };",
            ),
            (
                "@src/main/left",
                "use super::shared::shared; pub def left = shared;",
            ),
            (
                "@src/main/right",
                "use super::shared::shared; pub def right = shared;",
            ),
            ("@src/main/shared", "pub def shared = 1;"),
            ("@src/unused", "invalid unused text"),
            ("std/value", "pub type Value = enum { Missing };"),
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
        assert_eq!(mir.imports.len(), 4);
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
    fn retains_missing_targets_and_duplicate_inventory_candidates() {
        let mut inventory = ["@src/a", "@src/a/b", "@src/a/duplicate", "@src/a/duplicate"]
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
                "@src/a" => "mod b; mod missing; mod duplicate; pub def a = 1;",
                "@src/a/b" => "pub def b = 2;",
                _ => panic!("ambiguous module must not be chosen"),
            }
            .into())
        });
        assert_eq!(mir.imports.len(), 3);
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
