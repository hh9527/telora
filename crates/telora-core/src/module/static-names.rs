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

type StaticImportCandidates = BTreeMap<String, Vec<StaticImportTarget>>;

struct StaticNames<'a> {
    graph: &'a ModuleGraph,
    exports: HashMap<(ModuleId, String), StaticNameKind>,
    visiting: HashSet<(ModuleId, String)>,
    imports: Vec<StaticImportCandidates>,
    diagnostics: Vec<Vec<Diagnostic>>,
}

impl<'a> StaticNames<'a> {
    fn new(graph: &'a ModuleGraph) -> Self {
        let mut names = Self { graph, exports: HashMap::new(), visiting: HashSet::new(),
            imports: Vec::new(), diagnostics: Vec::new() };
        for module in &graph.modules {
            let (imports, diagnostics) = names.import_candidates(module.id);
            names.imports.push(imports);
            names.diagnostics.push(diagnostics);
        }
        names
    }

    fn program(&self, module: ModuleId) -> Option<&'a Program> {
        self.graph.module(module).prepared.as_ref()?.program.as_ref()
    }

    fn export_targets(&self, module: ModuleId) -> Vec<(String, StaticImportTarget)> {
        let Some(program) = self.program(module) else {
            return if self.graph.resolved[module.index()].as_ref().is_some_and(|resolved|
                static_data_kind(resolved.format).is_some())
            { vec![("data".into(), StaticImportTarget::Export { module, index: 0 })] }
            else { Vec::new() };
        };
        match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields.iter().enumerate()
                .filter_map(|(index, field)| field.value.name.as_ref().map(|name|
                    (name.value.clone(), StaticImportTarget::Export { module,
                        index: u32::try_from(index).expect("export count exceeds u32") })))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn import_candidates(&self, module: ModuleId) -> (StaticImportCandidates, Vec<Diagnostic>) {
        let Some(program) = self.program(module) else { return Default::default(); };
        let bindings = &program.value.body.value.bindings;
        let explicit = bindings.iter().filter(|binding|
            !matches!(binding.value.kind, BindingKind::OpenImport | BindingKind::Export))
            .map(|binding| binding.value.name.value.as_str()).collect::<HashSet<_>>();
        let mut imports = StaticImportCandidates::new();
        let mut diagnostics = Vec::new();
        let edges = &self.graph.module(module).imports;
        for edge in edges.iter().filter(|edge| edge.local.is_some()) {
            let local = edge.local.as_ref().unwrap();
            let binding = bindings.iter().find(|binding|
                binding.value.kind == BindingKind::Import && binding.value.name.value == *local);
            let target = match binding.and_then(|binding| binding.value.imported_name.as_deref()) {
                Some(selected) => {
                    let target = self.export_targets(edge.target).into_iter()
                        .find(|(name, _)| *name == selected.value).map(|(_, target)| target);
                    let Some(target) = target else {
                        diagnostics.push(Diagnostic::error(format!("module interface has no export {:?}", selected.value),
                            selected.location));
                        continue;
                    };
                    target
                }
                None => StaticImportTarget::Namespace(edge.target),
            };
            imports.insert(local.clone(), vec![target]);
        }
        for edge in edges.iter().filter(|edge| edge.local.is_none()
            && self.graph.module(edge.target).cname != ModuleCName::builtin(PRELUDE_MODULE))
        {
            for (name, target) in self.export_targets(edge.target) {
                if !explicit.contains(name.as_str()) {
                    imports.entry(name).or_default().push(target);
                }
            }
        }
        for edge in edges.iter().filter(|edge| edge.local.is_none()
            && self.graph.module(edge.target).cname == ModuleCName::builtin(PRELUDE_MODULE))
        {
            for (name, target) in self.export_targets(edge.target) {
                if !explicit.contains(name.as_str()) { imports.entry(name).or_insert_with(|| vec![target]); }
            }
        }
        for candidates in imports.values_mut() { candidates.sort_unstable(); candidates.dedup(); }
        (imports, diagnostics)
    }

    fn target_kind(&mut self, target: StaticImportTarget) -> StaticNameKind {
        match target {
            StaticImportTarget::Namespace(module) => StaticNameKind::Namespace(module),
            StaticImportTarget::Export { module, .. } => self.export(module,
                target.exported_name(self.graph).expect("selected export")),
        }
    }

    fn export(&mut self, module: ModuleId, name: &str) -> StaticNameKind {
        let key = (module, name.to_owned());
        if let Some(kind) = self.exports.get(&key) { return *kind; }
        if !self.visiting.insert(key.clone()) { return StaticNameKind::Unresolved; }
        let result = self.program(module).and_then(|program| {
            let ExprKind::Dict(fields) = &program.value.body.value.result.value else { return None; };
            fields.iter().find(|field| field.value.name.as_ref().is_some_and(|field| field.value == name))
                .map(|field| &field.value.value)
        });
        let kind = result.map_or(StaticNameKind::Data, |expression|
            self.expression(module, expression, &mut Vec::new()));
        self.visiting.remove(&key);
        self.exports.insert(key, kind);
        kind
    }

    fn external(&mut self, module: ModuleId, name: &str) -> StaticNameKind {
        if let Some(candidates) = self.imports[module.index()].get(name) {
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
        let candidates = self.imports[module.index()].clone();
        let names = candidates.keys().cloned().collect();
        let members = candidates.iter().filter(|(_, targets)| targets.iter().any(|target|
            matches!(self.target_kind(*target), StaticNameKind::Newtype | StaticNameKind::Member)))
            .map(|(name, _)| name.clone()).collect();
        let hir = crate::types::resolve_module_hir(program, &names, members);
        let mut diagnostics = std::mem::take(&mut self.diagnostics[module.index()]);
        let mut imports = BTreeMap::new();
        for (name, targets) in candidates {
            if targets.len() == 1 {
                imports.insert(name, targets[0]);
            } else if let Some(reference) = hir.references().iter().find(|reference|
                reference.name == name && reference.resolution == crate::hir::HirResolution::External)
            {
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
        hir
    }
}
