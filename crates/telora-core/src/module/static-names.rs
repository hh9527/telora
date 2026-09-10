// HIR needs declaration roles, not solved types or initialized module values.
// This session-wide index follows imports and re-exports before inference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaticNameKind {
    Data,
    Type,
    Enum,
    Newtype,
    Member,
    Namespace(ModuleId),
    Unresolved,
}

// Export positions are allocated by the parsed module result, before typing.
// Aliases keep a direct reference to that row instead of rediscovering a name
// from a solved interface. Data modules have the single synthetic data row.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum StaticImportTarget {
    Namespace(ModuleId),
    Export { module: ModuleId, index: u32 },
}

impl StaticImportTarget {
    fn module(self) -> ModuleId {
        match self { Self::Namespace(module) | Self::Export { module, .. } => module }
    }

    fn exported_name(self, graph: &ModuleGraph) -> Option<&str> {
        let Self::Export { module, index } = self else { return None; };
        match graph.module(module).prepared.as_ref().and_then(|prepared| prepared.program.as_ref()) {
            Some(program) => {
                let ExprKind::Dict(fields) = &program.value.body.value.result.value else {
                    unreachable!("export ID belongs to a module result")
                };
                Some(&fields[index as usize].value.name.as_ref().expect("named export row").value)
            }
            None => { assert_eq!(index, 0); Some("data") }
        }
    }
}

struct ResolvedStaticModule {
    hir: crate::hir::HirProgram,
    imports: BTreeMap<String, StaticImportTarget>,
    diagnostics: Vec<Diagnostic>,
}

struct ResolvedStaticGraph {
    modules: Vec<Option<ResolvedStaticModule>>,
    reachable: Vec<ModuleId>,
}

impl ResolvedStaticGraph {
    fn diagnostic_inputs(&mut self, graph: &ModuleGraph)
        -> Option<Vec<SemanticModuleInput>>
    {
        let diagnostics = |id: ModuleId| graph.module(id).prepared.iter()
            .flat_map(|prepared| prepared.diagnostics.iter())
            .chain(self.modules[id.index()].iter().flat_map(|module| module.diagnostics.iter()));
        if !self.reachable.iter().any(|id|
            diagnostics(*id).any(|diagnostic| diagnostic.severity == crate::source::Severity::Error))
        { return None; }
        // Resolution errors are a session gate. Produce syntax/diagnostic inputs
        // without creating a type store or starting any module's inference.
        Some(self.reachable.iter().map(|id| {
            let module = graph.module(*id);
            let resolved = graph.resolved[id.index()].as_ref();
            let prepared = module.prepared.as_ref();
            let mut diagnostics = prepared.iter().flat_map(|prepared| prepared.diagnostics.iter()).cloned().collect::<Vec<_>>();
            let partial = self.modules[id.index()].take().map(|resolved| {
                diagnostics.extend(resolved.diagnostics);
                crate::types::PartialAnalysis {
                    hir: resolved.hir,
                    dependencies: Default::default(), definition_facts: BTreeMap::new(),
                    definition_schemes: BTreeMap::new(), diagnostics: Vec::new(), types: Default::default(),
                }
            });
            let imports = prepared.iter().flat_map(|prepared| &prepared.recovered.bindings)
                .filter(|binding| matches!(binding.value.kind, BindingKind::Import | BindingKind::OpenImport))
                .filter_map(|binding| {
                    let Ok(target) = graph.import_targets.target(binding.value.value.location)? else { return None; };
                    Some(SemanticImport {
                        name: if binding.value.kind == BindingKind::OpenImport { "*".into() }
                            else { binding.value.name.value.clone() },
                        location: binding.value.name.location,
                        target: graph.module(target).cname.clone(),
                        namespace: binding.value.kind == BindingKind::Import && binding.value.imported_name.is_none(),
                    })
                }).collect();
            SemanticModuleInput {
                key: module.cname.to_string(),
                path: resolved.and_then(|module| module.path()).map(Path::to_owned),
                kind: resolved.and_then(|module| static_data_kind(module.format)).unwrap_or_else(||
                    if matches!(module.cname, ModuleCName::Builtin(_)) { WorkspaceModuleKind::Core }
                    else { WorkspaceModuleKind::Telora }),
                source: prepared.map(|prepared| prepared.source_id),
                result_location: prepared.and_then(|prepared| prepared.program.as_ref())
                    .map(|program| program.value.body.value.result.location),
                analysis: None, partial, interface: None,
                state: WorkspaceModuleState::Unavailable,
                imports,
                diagnostics,
            }
        }).collect())
    }
}

#[derive(Default)]
struct StaticImportScope {
    direct: BTreeMap<String, StaticImportTarget>,
    authored: HashSet<String>,
    open: Vec<ModuleId>,
    prelude: Option<ModuleId>,
}

#[derive(Clone, Copy, Default)]
enum StaticExportState {
    #[default]
    Unknown,
    Resolving,
    Known(StaticNameKind),
}

struct StaticNames<'a> {
    graph: &'a ModuleGraph,
    export_names: Vec<HashMap<&'a str, u32>>,
    exports: Vec<Vec<StaticExportState>>,
    scopes: Vec<StaticImportScope>,
    diagnostics: Vec<Vec<Diagnostic>>,
}

impl<'a> StaticNames<'a> {
    fn resolve_all(self) -> ResolvedStaticGraph {
        let roots = self.graph.modules.iter().map(|module| module.id).collect();
        self.resolve_roots(roots)
    }
    fn new(graph: &'a ModuleGraph) -> Self {
        let mut names = Self { graph, export_names: Vec::new(), exports: Vec::new(),
            scopes: Vec::new(), diagnostics: Vec::new() };
        // Index every provider before resolving any consumer. Names borrow the
        // source inventory; the row identity does not depend on lookup order.
        for module in &graph.modules {
            let mut index = HashMap::new();
            let count = if let Some(program) = names.program(module.id) {
                if let ExprKind::Dict(fields) = &program.value.body.value.result.value {
                    for (row, field) in fields.iter().enumerate() {
                        if let Some(name) = &field.value.name {
                            index.entry(name.value.as_str()).or_insert_with(||
                                u32::try_from(row).expect("export count exceeds u32"));
                        }
                    }
                    fields.len()
                } else { 0 }
            } else if graph.resolved[module.id.index()].as_ref().is_some_and(|resolved|
                static_data_kind(resolved.format).is_some())
            {
                index.insert("data", 0);
                1
            } else { 0 };
            names.export_names.push(index);
            names.exports.push(vec![StaticExportState::Unknown; count]);
        }
        for module in &graph.modules {
            let (scope, diagnostics) = names.import_scope(module.id);
            names.scopes.push(scope);
            names.diagnostics.push(diagnostics);
        }
        names
    }

    fn program(&self, module: ModuleId) -> Option<&'a Program> {
        self.graph.module(module).prepared.as_ref()?.program.as_ref()
    }

    fn export_target(&self, module: ModuleId, name: &str) -> Option<StaticImportTarget> {
        self.export_names[module.index()].get(name)
            .map(|index| StaticImportTarget::Export { module, index: *index })
    }

    fn import_scope(&self, module: ModuleId) -> (StaticImportScope, Vec<Diagnostic>) {
        let Some(program) = self.program(module) else { return Default::default(); };
        let bindings = &program.value.body.value.bindings;
        let authored = bindings.iter().filter(|binding|
            !matches!(binding.value.kind, BindingKind::OpenImport | BindingKind::Export))
            .map(|binding| binding.value.name.value.clone()).collect();
        let mut scope = StaticImportScope { authored, ..Default::default() };
        let mut diagnostics = Vec::new();
        let edges = &self.graph.module(module).imports;
        for edge in edges.iter().filter(|edge| edge.local.is_some()) {
            let local = edge.local.as_ref().unwrap();
            let binding = bindings.iter().find(|binding|
                binding.value.kind == BindingKind::Import && binding.value.name.value == *local);
            let target = match binding.and_then(|binding| binding.value.imported_name.as_deref()) {
                Some(selected) => {
                    let target = self.export_target(edge.target, &selected.value);
                    let Some(target) = target else {
                        diagnostics.push(Diagnostic::error(format!("module interface has no export {:?}", selected.value),
                            selected.location));
                        continue;
                    };
                    target
                }
                None => StaticImportTarget::Namespace(edge.target),
            };
            scope.direct.insert(local.clone(), target);
        }
        for edge in edges.iter().filter(|edge| edge.local.is_none()) {
            let prelude = self.graph.module(edge.target).cname == ModuleCName::builtin(PRELUDE_MODULE);
            let explicit = bindings.iter().any(|binding| binding.value.kind == BindingKind::OpenImport
                && self.graph.import_targets.target(binding.value.value.location) == Some(Ok(edge.target)));
            if prelude { scope.prelude = Some(edge.target); }
            if !prelude || explicit {
                scope.open.push(edge.target);
            }
        }
        scope.open.sort_unstable();
        scope.open.dedup();
        (scope, diagnostics)
    }

    fn candidates(&self, module: ModuleId, name: &str) -> Vec<StaticImportTarget> {
        let scope = &self.scopes[module.index()];
        if let Some(target) = scope.direct.get(name) { return vec![*target]; }
        if scope.authored.contains(name) { return Vec::new(); }
        let mut targets = scope.open.iter().filter_map(|module| self.export_target(*module, name))
            .collect::<Vec<_>>();
        if targets.is_empty() {
            targets.extend(scope.prelude.and_then(|module| self.export_target(module, name)));
        }
        targets
    }

    fn target_kind(&mut self, target: StaticImportTarget) -> StaticNameKind {
        match target {
            StaticImportTarget::Namespace(module) => StaticNameKind::Namespace(module),
            StaticImportTarget::Export { module, index } => self.export_kind(module, index),
        }
    }

    fn export(&mut self, module: ModuleId, name: &str) -> StaticNameKind {
        self.export_target(module, name).map_or(StaticNameKind::Unresolved,
            |target| self.target_kind(target))
    }

    fn export_kind(&mut self, module: ModuleId, index: u32) -> StaticNameKind {
        match self.exports[module.index()][index as usize] {
            StaticExportState::Known(kind) => return kind,
            StaticExportState::Resolving => return StaticNameKind::Unresolved,
            StaticExportState::Unknown => {}
        }
        self.exports[module.index()][index as usize] = StaticExportState::Resolving;
        let result = self.program(module).and_then(|program| {
            let ExprKind::Dict(fields) = &program.value.body.value.result.value else { return None; };
            Some(&fields[index as usize].value.value)
        });
        let kind = result.map_or(StaticNameKind::Data, |expression|
            self.expression(module, expression, &mut Vec::new()));
        self.exports[module.index()][index as usize] = StaticExportState::Known(kind);
        kind
    }

    fn external(&mut self, module: ModuleId, name: &str) -> StaticNameKind {
        let candidates = self.candidates(module, name);
        if !candidates.is_empty() {
            return if candidates.len() == 1 { self.target_kind(candidates[0]) }
                else { StaticNameKind::Unresolved };
        }
        match name {
            "Bool" | "Option" | "Result" | "FoldControl" | "PropertyTarget" => StaticNameKind::Enum,
            _ => StaticNameKind::Data,
        }
    }

    fn expression(&mut self, module: ModuleId, expression: &Expr, path: &mut Vec<crate::Location>) -> StaticNameKind {
        use StaticNameKind as K;
        match &expression.value {
            ExprKind::Variable(name) => {
                let binding = self.program(module).and_then(|program|
                    program.value.body.value.bindings.iter().rev().find(|binding|
                        binding.value.name.value == name.value
                        && !matches!(binding.value.kind, BindingKind::Export | BindingKind::OpenImport)
                        && (!matches!(binding.value.kind, BindingKind::Let | BindingKind::Import)
                            || binding.location.end <= expression.location.start)));
                let Some(binding) = binding else { return self.external(module, &name.value); };
                if path.contains(&binding.location) { return K::Unresolved; }
                match binding.value.kind {
                    BindingKind::Import => self.external(module, &name.value),
                    BindingKind::NativeType | BindingKind::Trait => K::Type,
                    BindingKind::Type => match binding.value.declared_initializer {
                        Some(crate::ast::DeclaredInitializerKind::Enum) => K::Enum,
                        Some(crate::ast::DeclaredInitializerKind::Newtype) => K::Newtype,
                        Some(crate::ast::DeclaredInitializerKind::Struct) => K::Type,
                        None => {
                            path.push(binding.location);
                            let kind = self.expression(module, &binding.value.value, path);
                            path.pop();
                            match kind { K::Newtype | K::Enum | K::Unresolved => kind, _ => K::Type }
                        }
                    },
                    BindingKind::Def if binding.value.is_member_import() => K::Member,
                    _ => K::Data,
                }
            }
            ExprKind::TypeSyntax(inner) => self.expression(module, inner, path),
            ExprKind::Field { receiver, field } => match self.expression(module, receiver, path) {
                K::Namespace(target) => self.export(target, &field.value),
                K::Enum => K::Member,
                K::Unresolved => K::Unresolved,
                _ => K::Data,
            },
            ExprKind::TypeApply { callee, .. } => self.expression(module, callee, path),
            ExprKind::Call { callee, .. } => match self.expression(module, callee, path) {
                kind @ (K::Type | K::Enum | K::Newtype | K::Unresolved) => kind,
                _ => K::Data,
            },
            _ => K::Data,
        }
    }

    fn module_resolution(&mut self, module: ModuleId) -> Option<ResolvedStaticModule> {
        let prepared = self.graph.module(module).prepared.as_ref()?;
        let program = prepared.program.as_ref();
        let mut queried = BTreeMap::<String, Vec<StaticImportTarget>>::new();
        let mut lookup = |name: &str| {
            let targets = queried.entry(name.to_owned()).or_insert_with(|| self.candidates(module, name));
            crate::hir::HirExternalName { declared: !targets.is_empty()
                || self.graph.host_symbols.get(&module).is_some_and(|symbols| symbols.contains_key(name)),
                member: targets.iter().any(|target| matches!(self.target_kind(*target),
                    StaticNameKind::Newtype | StaticNameKind::Member)) }
        };
        let mut hir = match program {
            Some(program) => crate::types::resolve_module_hir_with_lookup(program, &mut lookup),
            None => crate::hir::HirProgram::resolve_recovered_with_lookup(&prepared.recovered, &mut |name| {
                let mut external = lookup(name);
                external.declared |= crate::types::bootstrap_symbol(name).is_some();
                external
            }),
        };
        let mut diagnostics = std::mem::take(&mut self.diagnostics[module.index()]);
        diagnostics.extend(hir.unresolved().filter(|reference| reference.resolution == crate::hir::HirResolution::Unresolved).map(|reference|
            Diagnostic::error(format!("unknown binding {:?}", reference.name), reference.location)));
        let mut imports = self.scopes[module.index()].direct.clone();
        for (name, targets) in queried {
            let Some(_) = hir.references().iter().find(|reference|
                reference.name == name && reference.resolution == crate::hir::HirResolution::External)
                else { continue; };
            if targets.len() == 1 {
                imports.insert(name, targets[0]);
            } else if targets.len() > 1 {
                hir.conflict_external(&name, targets.iter().map(|target| match *target {
                    StaticImportTarget::Namespace(module) => crate::hir::HirImportOrigin::Namespace(module),
                    StaticImportTarget::Export { module, index } => crate::hir::HirImportOrigin::Export { module, index },
                }).collect());
            }
        }
        for (index, conflict) in hir.conflicts().iter().enumerate() {
            use crate::hir::{HirImportOrigin as Origin, HirResolveConflict as Conflict};
            match conflict {
                Conflict::DuplicateDefinition { name, definitions } => {
                    let first = hir.definition(definitions[0]).unwrap();
                    let second = hir.definition(definitions[1]).unwrap();
                    let prefix = if first.top_level { "module binding" } else { "binding" };
                    let mut diagnostic = Diagnostic::error(
                        format!("{prefix} {name:?} conflicts with an earlier explicit binding"), second.location)
                        .with_secondary("first bound here", first.location);
                    for definition in &definitions[2..] {
                        diagnostic = diagnostic.with_secondary("also bound here", hir.definition(*definition).unwrap().location);
                    }
                    if !diagnostics.contains(&diagnostic) { diagnostics.push(diagnostic); }
                }
                Conflict::AmbiguousImport { name, candidates } => {
                    let providers = candidates.iter().filter_map(|candidate| match candidate {
                        Origin::Definition { module, .. } | Origin::Export { module, .. }
                        | Origin::Host { module, .. } | Origin::Namespace(module) => Some(self.graph.module(*module).cname.to_string()),
                        Origin::Bootstrap(_) => None,
                    }).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", ");
                    let reference = hir.references().iter().find(|reference|
                        matches!(reference.resolution, crate::hir::HirResolution::Conflicted(id) if id.index() == index)).unwrap();
                    diagnostics.push(Diagnostic::error(
                        format!("open import name {name:?} is ambiguous between {providers}"), reference.location));
                }
            }
        }
        Some(ResolvedStaticModule { hir, imports, diagnostics })
    }

    fn resolve(self, root: ModuleId) -> ResolvedStaticGraph {
        self.resolve_roots(vec![root])
    }

    fn resolve_roots(mut self, mut pending: Vec<ModuleId>) -> ResolvedStaticGraph {
        let mut hir = std::iter::repeat_with(|| None).take(self.graph.modules.len()).collect::<Vec<_>>();
        let mut visited = vec![false; hir.len()];
        while let Some(id) = pending.pop() {
            if std::mem::replace(&mut visited[id.index()], true) { continue; }
            let module = self.graph.module(id);
            pending.extend(module.imports.iter().map(|edge| edge.target));
            if self.graph.resolved[id.index()].as_ref().is_some_and(|resolved|
                resolved.format != ModuleFormat::Telora)
            {
                pending.push(self.graph.id(&ModuleCName::builtin(crate::core::VALUE_MODULE))
                    .expect("builtin Value inventory"));
            }
            hir[id.index()] = self.module_resolution(id);
        }
        let aliases = self.resolve_export_aliases(&mut hir);
        for index in 0..hir.len() {
            let Some(module) = &hir[index] else { continue; };
            let mut origins = module.imports.iter().map(|(name, target)|
                (name.clone(), self.import_origin(*target, &hir))).collect::<BTreeMap<_, _>>();
            let module_id = ModuleId::from_index(index);
            if let Some(symbols) = self.graph.host_symbols.get(&module_id) {
                for (name, index) in symbols {
                    origins.insert(name.clone(), crate::hir::HirImportOrigin::Host { module: module_id, index: *index });
                }
            }
            for reference in module.hir.references() {
                if reference.resolution == crate::hir::HirResolution::External
                    && !origins.contains_key(&reference.name)
                    && let Some(origin) = crate::types::bootstrap_symbol(&reference.name)
                {
                    origins.insert(reference.name.clone(), origin);
                }
            }
            hir[index].as_mut().unwrap().hir.set_import_origins(&origins);
        }
        for index in 0..hir.len() {
            let Some(module) = &hir[index] else { continue; };
            let mut origins = vec![None; module.hir.expressions().len()];
            let mut diagnostics = Vec::new();
            // HIR indexes receivers before their member accesses, including
            // nested namespaces. Local shadowing has already been resolved.
            for member in module.hir.member_accesses() {
                let receiver = module.hir.expression(member.receiver).expect("member receiver");
                let origin = origins[member.receiver.index()].or_else(|| receiver.reference
                    .and_then(|id| module.hir.reference_import_origin(id)));
                let Some(crate::hir::HirImportOrigin::Namespace(provider)) = origin else { continue; };
                match self.export_target(provider, &member.field) {
                    Some(target) => origins[member.expression.index()] =
                        Some(self.import_origin(Self::canonical_target(&aliases, target), &hir)),
                    None => diagnostics.push(Diagnostic::error(
                        format!("module {} has no export {:?}", self.graph.module(provider).cname, member.field),
                        member.location)),
                }
            }
            let module = hir[index].as_mut().unwrap();
            module.hir.set_expression_import_origins(origins);
            module.diagnostics.extend(diagnostics);
            // Success means every nonlocal reference has a concrete source
            // identity, including bootstrap symbols. No name-only externals
            // may silently cross this boundary.
            for reference in module.hir.references() {
                if reference.resolution == crate::hir::HirResolution::External
                    && module.hir.reference_import_origin(reference.id).is_none()
                {
                    module.diagnostics.push(Diagnostic::error(
                        format!("binding {:?} has no resolved symbol identity", reference.name),
                        reference.location));
                }
            }
        }
        ResolvedStaticGraph { modules: hir, reachable: self.graph.modules.iter()
            .filter(|module| visited[module.id.index()]).map(|module| module.id).collect() }
    }

    fn import_origin(&self, target: StaticImportTarget, modules: &[Option<ResolvedStaticModule>]) -> crate::hir::HirImportOrigin {
        use crate::hir::{HirImportOrigin as Origin, HirResolution};
        match target {
            StaticImportTarget::Namespace(module) => Origin::Namespace(module),
            StaticImportTarget::Export { module, index } => {
                let definition = self.program(module).and_then(|program| {
                    let ExprKind::Dict(fields) = &program.value.body.value.result.value else { return None; };
                    let ExprKind::Variable(name) = &fields[index as usize].value.value.value else { return None; };
                    let resolved = modules[module.index()].as_ref()?;
                    let reference = resolved.hir.reference_at(name.location, &name.value)?;
                    match reference.resolution {
                        HirResolution::Definition(definition) => Some(definition),
                        _ => None,
                    }
                });
                definition.map_or(Origin::Export { module, index },
                    |definition| Origin::Definition { module, definition })
            }
        }
    }

    fn canonical_target(aliases: &[Vec<StaticImportTarget>], target: StaticImportTarget) -> StaticImportTarget {
        let mut current = target;
        let mut path = Vec::new();
        while let StaticImportTarget::Export { module, index } = current {
            let next = aliases[module.index()][index as usize];
            if next == current { break; }
            // Keep a cycle available to the module-cycle diagnostic.
            if path.contains(&current) { return target; }
            path.push(current);
            current = next;
        }
        current
    }

    fn resolve_export_aliases(&mut self, modules: &mut [Option<ResolvedStaticModule>]) -> Vec<Vec<StaticImportTarget>> {
        // These edges describe source identity, not type equality. In particular,
        // a new def whose initializer refers to another def remains distinct.
        let mut aliases = self.exports.iter().enumerate().map(|(module, rows)|
            (0..rows.len()).map(|index| StaticImportTarget::Export {
                module: self.graph.modules[module].id, index: index as u32,
            }).collect::<Vec<_>>()).collect::<Vec<_>>();
        for module in &self.graph.modules {
            let Some(resolved) = &modules[module.id.index()] else { continue; };
            let Some(program) = self.program(module.id) else { continue; };
            let ExprKind::Dict(fields) = &program.value.body.value.result.value else { continue; };
            let mut declarations = HashMap::new();
            for (index, field) in fields.iter().enumerate() {
                let target = aliases[module.id.index()][index];
                let origin = match &field.value.value.value {
                    ExprKind::Variable(name) => resolved.hir.reference_at(name.location, &name.value)
                        .and_then(|reference| match reference.resolution {
                            crate::hir::HirResolution::Definition(id) => {
                                let definition = resolved.hir.definition(id).expect("resolved definition");
                                if definition.kind == crate::hir::HirDefinitionKind::Import {
                                    resolved.imports.get(&definition.name).copied()
                                } else {
                                    Some(*declarations.entry(id).or_insert(target))
                                }
                            }
                            crate::hir::HirResolution::External => resolved.imports.get(&name.value).copied(),
                            crate::hir::HirResolution::Unresolved | crate::hir::HirResolution::Conflicted(_) => None,
                        }),
                    ExprKind::Field { receiver, field } => {
                        if let StaticNameKind::Namespace(provider) =
                            self.expression(module.id, receiver, &mut Vec::new())
                        { self.export_target(provider, &field.value) } else { None }
                    }
                    _ => None,
                };
                if let Some(origin) = origin { aliases[module.id.index()][index] = origin; }
            }
        }
        for resolved in modules.iter_mut().flatten() {
            for target in resolved.imports.values_mut() {
                *target = Self::canonical_target(&aliases, *target);
            }
        }
        aliases
    }
}
