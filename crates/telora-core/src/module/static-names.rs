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
        let program = self.program(module)?;
        let mut queried = BTreeMap::<String, Vec<StaticImportTarget>>::new();
        let hir = crate::types::resolve_module_hir_with_lookup(program, |name| {
            let targets = queried.entry(name.to_owned()).or_insert_with(|| self.candidates(module, name));
            crate::hir::HirExternalName { declared: !targets.is_empty(),
                member: targets.iter().any(|target| matches!(self.target_kind(*target),
                    StaticNameKind::Newtype | StaticNameKind::Member)) }
        });
        let mut diagnostics = std::mem::take(&mut self.diagnostics[module.index()]);
        let mut imports = self.scopes[module.index()].direct.clone();
        for (name, targets) in queried {
            let Some(reference) = hir.references().iter().find(|reference|
                reference.name == name && reference.resolution == crate::hir::HirResolution::External)
                else { continue; };
            if targets.len() == 1 {
                imports.insert(name, targets[0]);
            } else if targets.len() > 1 {
                let providers = targets.iter().map(|target| self.graph.module(target.module()).cname.to_string())
                    .collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(", ");
                diagnostics.push(Diagnostic::error(
                    format!("open import name {name:?} is ambiguous between {providers}"), reference.location));
            }
        }
        Some(ResolvedStaticModule { hir, imports, diagnostics })
    }

    fn resolve(mut self, root: ModuleId) -> Vec<Option<ResolvedStaticModule>> {
        let mut hir = std::iter::repeat_with(|| None).take(self.graph.modules.len()).collect::<Vec<_>>();
        let mut visited = vec![false; hir.len()];
        let mut pending = vec![root];
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
        self.resolve_export_aliases(&mut hir);
        hir
    }

    fn resolve_export_aliases(&mut self, modules: &mut [Option<ResolvedStaticModule>]) {
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
                            crate::hir::HirResolution::Unresolved => None,
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
                let mut current = *target;
                let mut path = Vec::new();
                while let StaticImportTarget::Export { module, index } = current {
                    let next = aliases[module.index()][index as usize];
                    if next == current { break; }
                    // A source cycle is not a successful identity solution.
                    // Preserve its target for the module-cycle diagnostic.
                    if path.contains(&current) { current = *target; break; }
                    path.push(current);
                    current = next;
                }
                *target = current;
            }
        }
    }
}
