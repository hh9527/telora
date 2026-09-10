//! Read-only CLI consumers of the new MIR, including unsuccessful pass outcomes.
use crate::{
    ModuleSelector, QUERY_SCHEMA, QueryArgs, QueryCommand, QueryPosition, ShowKind, emit,
    kind_name, static_input::Inventory,
};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Instant};
use telora_core::{
    Location, PositionEncoding, TextPosition,
    ast::BindingKind,
    mir::{Mir, ModuleState, ResolveState, Symbol, SymbolId, SymbolKind, TypeState},
    mir_query::MirQuery,
    source::{Diagnostic, Severity},
};

fn location(mir: &Mir, loc: Location) -> Value {
    let source = mir.sources.get(loc.source);
    let start = source
        .text()
        .position(loc.start, PositionEncoding::Utf8)
        .expect("HIR position");
    let end = source
        .text()
        .position(loc.end, PositionEncoding::Utf8)
        .expect("HIR position");
    json!({"line": start.line+1, "column": start.character, "end_line": end.line+1, "end_column": end.character})
}

pub(crate) fn diagnostic(mir: &Mir, schema: &str, root: &str, d: &Diagnostic) -> Value {
    json!({"schema": schema, "module": root, "record": "diagnostic",
        "severity": match d.severity { Severity::Error => "error", Severity::Warning => "warning", Severity::Info => "info" },
        "message": d.message, "notes": d.notes,
        "labels": d.labels.iter().map(|l| json!({"source": mir.sources.get(l.location.source).name.as_ref(),
            "location": location(mir, l.location), "message": l.message, "primary": l.primary})).collect::<Vec<_>>()})
}

pub fn check(
    context: PathBuf,
    selector: &str,
    schema: &str,
    types_only: bool,
) -> Result<i32, String> {
    let started = Instant::now();
    let mut inventory = Inventory::new(&context, selector.starts_with("std/"))?;
    let root = inventory.select(selector)?;
    for message in inventory.undeclared_warnings()? {
        emit(
            json!({"schema": schema, "module": root, "record": "diagnostic",
            "severity": "warning", "message": message, "labels": [], "notes": []}),
        )?;
    }
    let catalog_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let mut mir = inventory.solve(&root);
    let static_seconds = started.elapsed().as_secs_f64();
    let unproven_bounds = mir
        .bound_requirements
        .iter()
        .filter(|r| !r.state.is_proven())
        .count();
    let static_failed = mir
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
        || !mir.type_unknowns.is_empty()
        || !mir.type_conflicts.is_empty()
        || unproven_bounds != 0;
    let execution_started = Instant::now();
    let mut execution_diagnostics = vec![];
    if !types_only && !static_failed {
        let artifact = mir.seal().and_then(telora_core::codegen::compile_check);
        let linked = artifact.and_then(|artifact| {
            telora_core::execution_link::link_entry_with_data(artifact, |link| {
                inventory.read_data(link, crate::engine_config().data_limits.file_size)
            })
        });
        match linked {
            Ok(linked) => {
                let config = crate::engine_config();
                execution_diagnostics = telora_core::Vm::new()
                    .with_debug_sink(std::sync::Arc::new(crate::StderrDebugSink))
                    .check_linked(
                        linked,
                        config.session_quota,
                        config.data_limits,
                        &mut mir.sources,
                    );
            }
            Err(diagnostics) => execution_diagnostics = diagnostics,
        }
    }
    let execution_seconds = if types_only || static_failed {
        0.0
    } else {
        execution_started.elapsed().as_secs_f64()
    };
    let failed = static_failed
        || execution_diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error);
    for d in mir.diagnostics.iter().chain(&execution_diagnostics) {
        emit(diagnostic(&mir, schema, &root, d))?;
    }
    let check_seconds = static_seconds + execution_seconds;
    emit(
        json!({"schema": schema, "module": root, "record": "summary",
        "status": if failed { "error" } else { "ok" }, "types_only": types_only,
        "dependencies": mir.modules.iter().filter(|m| !matches!(m.state, ModuleState::Unloaded)
            && inventory.entries.get(&m.name).is_some_and(|entry| entry.origin != "builtin")).count().saturating_sub(1),
        "unknown_types": mir.type_unknowns.len(), "type_conflicts": mir.type_conflicts.len(),
        "property_records": mir.properties.len(), "bound_requirements": mir.bound_requirements.len(), "unproven_bounds": unproven_bounds,
        "check_seconds": check_seconds, "static_seconds": static_seconds, "execution_seconds": execution_seconds,
        "catalog_seconds": catalog_seconds}),
    )?;
    Ok(i32::from(failed))
}

fn type_fields(mir: &Mir, state: TypeState) -> (Option<usize>, Option<String>, &'static str) {
    match state {
        TypeState::Known(id) => (Some(id.index()), Some(MirQuery::new(mir).type_name(id)), "Known"),
        TypeState::Unknown => (None, None, "Unknown"),
        TypeState::Conflicted(_) => (None, None, "Conflicted"),
        TypeState::ProxyTo(_) | TypeState::Structure(_) => {
            unreachable!("type pass normalizes all slots")
        }
    }
}

fn kind(symbol: &Symbol) -> Option<ShowKind> {
    match symbol.kind {
        SymbolKind::Declaration(BindingKind::Type | BindingKind::NativeType) => {
            Some(ShowKind::Type)
        }
        SymbolKind::Declaration(BindingKind::Let) => Some(ShowKind::Let),
        SymbolKind::Declaration(BindingKind::Def | BindingKind::Decl | BindingKind::Native) => {
            Some(ShowKind::Def)
        }
        SymbolKind::Import | SymbolKind::Namespace(_) => Some(ShowKind::Import),
        _ => None,
    }
}

fn resolve_fields(state: &ResolveState) -> (&'static str, Option<usize>) {
    match state {
        ResolveState::Bound(id) => ("Bound", Some(id.index())),
        ResolveState::Unresolved => ("Unresolved", None),
        ResolveState::Conflicted(_) => ("Conflicted", None),
        ResolveState::Member { .. } => ("Member", None),
        ResolveState::Pending => unreachable!("symbol pass closes all references"),
    }
}

fn definition(mir: &Mir, root: &str, id: SymbolId, kind: ShowKind) -> Value {
    let index = id.index();
    let symbol = &mir.symbols[index];
    let (type_id, ty, state) = type_fields(mir, MirQuery::new(mir).symbol_type(id));
    let (resolution, target_id) = resolve_fields(&symbol.resolution);
    let loc = symbol
        .declarations
        .first()
        .map(|id| location(mir, mir.hir[id.index()].location));
    let target = match symbol.kind {
        SymbolKind::Namespace(id) => Some(mir.modules[id.index()].name.as_str()),
        _ => None,
    };
    json!({"schema": QUERY_SCHEMA, "module": root, "record": "definition", "authority": "authoritative",
        "symbol_id": index, "name": symbol.name, "kind": kind_name(kind), "type_id": type_id,
        "type": ty, "state": state, "resolution": resolution, "target_id": target_id, "target": target, "location": loc})
}

fn position_range(
    mir: &Mir,
    module: usize,
    at: &QueryPosition,
) -> Result<(telora_core::source::SourceId, u32, u32), String> {
    let ModuleState::Source { source, .. } = mir.modules[module].state else {
        return Err("selected module has no source".into());
    };
    let text = mir.sources.get(source).text();
    let line = u32::try_from(at.line - 1).map_err(|_| "line is outside module")?;
    let (start, end) = if let Some(column) = at.column {
        let column = u32::try_from(column).map_err(|_| "column is outside module")?;
        let start = text
            .offset(TextPosition::new(line, column), PositionEncoding::Utf8)
            .map_err(|e| e.to_string())?;
        (start, start)
    } else {
        text.line_content_offsets(line).map_err(|e| e.to_string())?
    };
    Ok((source, start, end))
}

pub fn query(context: PathBuf, args: QueryArgs) -> Result<i32, String> {
    if let QueryCommand::Modules(args) = &args.command {
        let inventory = Inventory::new(&context, false)?;
        for e in inventory
            .catalog()
            .filter(|e| args.pattern.as_deref().is_none_or(|p| e.name.contains(p)))
        {
            emit(
                json!({"schema": QUERY_SCHEMA, "record": "module", "module": e.name,
                "origin": e.origin, "visibility": e.visibility, "format": e.format.name()}),
            )?;
        }
        return Ok(0);
    }
    let selector = match &args.command {
        QueryCommand::Exports(a) => &a.module_id,
        QueryCommand::At(a) => &a.selector.module_id,
        QueryCommand::Modules(_) => unreachable!(),
    };
    let mut inventory = Inventory::new(&context, selector.starts_with("std/"))?;
    let root = inventory.select(selector)?;
    let mir = inventory.solve(&root);
    for d in &mir.diagnostics {
        emit(diagnostic(&mir, QUERY_SCHEMA, &root, d))?;
    }
    let Some(module) = mir.modules.iter().position(|m| m.name == root) else {
        return Ok(1);
    };
    if !matches!(
        mir.modules[module].state,
        ModuleState::Source { .. } | ModuleState::Data { .. }
    ) {
        return Ok(1);
    }
    match args.command {
        QueryCommand::Exports(args) => {
            let mut exports = mir.exports[module].clone();
            exports.sort_by_key(|id| &mir.symbols[id.index()].name);
            for id in exports {
                let symbol = &mir.symbols[id.index()];
                if args
                    .pattern
                    .as_deref()
                    .is_some_and(|p| !symbol.name.contains(p))
                {
                    continue;
                }
                let (type_id, ty, state) =
                    type_fields(&mir, MirQuery::new(&mir).symbol_type(id));
                let (resolution, target_id) = resolve_fields(&symbol.resolution);
                emit(
                    json!({"schema": QUERY_SCHEMA, "module": root, "record": "export", "authority": "authoritative",
                    "name": symbol.name, "symbol_id": id.index(), "target_id": target_id, "resolution": resolution,
                    "type_id": type_id, "type": ty, "state": state}),
                )?;
            }
        }
        QueryCommand::At(args) => {
            let ModuleSelector { position, .. } = args.selector;
            if position.is_some() && (args.pattern.is_some() || args.kinds.is_some()) {
                return Err("-p/--pattern and -k/--kind require a module-only query target".into());
            }
            let range = position
                .as_ref()
                .map(|at| position_range(&mir, module, at))
                .transpose()?;
            let intersects = |loc: Location| {
                range.is_some_and(|(source, start, end)| {
                    loc.source == source
                        && if start == end {
                            loc.start <= start && start < loc.end
                        } else {
                            loc.start < end && start < loc.end
                        }
                })
            };
            let mut definitions = MirQuery::new(&mir).symbols()
                .filter(|(_, s)| s.module.is_some_and(|id| id.index() == module))
                .filter_map(|(i, s)| kind(s).map(|k| (i, s, k)))
                .filter(|(_, s, k)| {
                    if range.is_some() {
                        s.declarations
                            .iter()
                            .any(|id| intersects(mir.hir[id.index()].location))
                    } else {
                        s.scope == mir.module_scopes[module]
                            && args.pattern.as_deref().is_none_or(|p| s.name.contains(p))
                            && args.kinds.as_ref().is_none_or(|ks| ks.0.contains(k))
                    }
                })
                .collect::<Vec<_>>();
            definitions.sort_by_key(|(i, s, k)| (&s.name, *k, *i));
            for (i, _, k) in definitions {
                emit(definition(&mir, &root, i, k))?;
            }
            if range.is_some() {
                for reference in MirQuery::new(&mir).references().filter(|reference| intersects(reference.location)) {
                    let node = &mir.hir[reference.node.index()];
                    let slot = node.resolution.expect("reference has a resolve slot");
                    let (resolution, target_id) = resolve_fields(reference.resolution);
                    let name = match &node.kind {
                        telora_core::mir::HirKind::Variable(n) | telora_core::mir::HirKind::PatternName(n) => Some(n.as_str()),
                        _ => None,
                    };
                    emit(json!({"schema": QUERY_SCHEMA, "module": root, "record": "reference", "authority": "authoritative",
                        "hir_id": reference.node.index(), "resolve_slot": slot.index(), "name": name, "resolved": target_id.is_some(),
                        "resolution": resolution, "target_id": target_id, "location": location(&mir, reference.location)}))?;
                }
                for (i, node) in mir
                    .hir
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.module.index() == module && intersects(n.location))
                {
                    if mir.required_types[i] {
                        let (type_id, ty, state) = type_fields(&mir, mir.ty_slots[i]);
                        emit(
                            json!({"schema": QUERY_SCHEMA, "module": root, "record": "expression", "authority": "authoritative",
                            "hir_id": i, "type_slot": i, "type_id": type_id, "type": ty, "state": state, "location": location(&mir,node.location)}),
                        )?;
                    }
                }
            }
        }
        QueryCommand::Modules(_) => unreachable!(),
    }
    // Query success means facts were returned, even if the program has diagnostics.
    Ok(0)
}
