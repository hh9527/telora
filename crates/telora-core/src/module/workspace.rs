fn runtime_program_with_evidence(program: &Program, analysis: &crate::Analysis) -> Program {
    let mut runtime_program = program.clone();
    if let ExprKind::Dict(fields) = &mut runtime_program.value.body.value.result.value {
        for published in &analysis.module_interface.trait_implementations {
            let source = analysis
                .trait_implementations
                .iter()
                .find(|implementation| implementation.id == published.id)
                .expect("published trait implementation has an analysis source");
            let location = runtime_program.value.body.value.result.location;
            fields.push(located(
                DictFieldKind {
                    decorators: Vec::new(),
                    name: Some(located(published.dictionary.clone(), location)),
                    value: located(
                        ExprKind::Variable(located(source.dictionary.clone(), location)),
                        location,
                    ),
                },
                location,
            ));
        }
        for evidence in &analysis.module_interface.type_properties {
            let location = runtime_program.value.body.value.result.location;
            fields.push(located(
                DictFieldKind {
                    decorators: Vec::new(),
                    name: Some(located(evidence.root.clone(), location)),
                    value: located(
                        ExprKind::Variable(located(evidence.root.clone(), location)),
                        location,
                    ),
                },
                location,
            ));
        }
    }
    runtime_program
}

struct WorkspaceBuilder<'a> {
    engine: &'a Engine,
    overlays: &'a BTreeMap<PathBuf, crate::document::DocumentText>,
    sources: SourceDatabase,
    main: MainWorld,
    builtin_modules: HashMap<String, ModuleArtifact>,
    inputs: BTreeMap<String, SemanticModuleInput>,
    provenances: HashMap<ModuleCName, Provenance>,
    roots: HashMap<ModuleCName, PersistentValue>,
    interfaces: HashMap<ModuleCName, ModuleInterface>,
    visiting: Vec<ModuleCName>,
    cycle_members: HashSet<ModuleCName>,
    cycle_reported: bool,
}

impl WorkspaceBuilder<'_> {
    fn install_evidence_roots(
        &self,
        root: PersistentValue,
        interface: &ModuleInterface,
        external_roots: &mut HashMap<String, PersistentValue>,
        diagnostics: &mut Vec<Diagnostic>,
        location: crate::Loc,
    ) {
        for name in interface
            .trait_implementations
            .iter()
            .map(|implementation| &implementation.dictionary)
            .chain(
                interface
                    .type_properties
                    .iter()
                    .map(|evidence| &evidence.root),
            )
        {
            match root.export_get(&self.main.heap, name) {
                Ok(Some(value)) => {
                    external_roots.entry(name.clone()).or_insert(value);
                }
                Ok(None) => diagnostics.push(Diagnostic::error(
                    format!("module is missing evidence root {name:?}"),
                    location,
                )),
                Err(error) => diagnostics.push(Diagnostic::error(error.to_string(), location)),
            }
        }
    }

    fn load_telora<'a>(
        &'a mut self,
        module: ResolvedModule,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<PersistentValue>> + 'a>> {
        Box::pin(async move {
            let path = module.path()?.to_owned();
            let vendor = module.vendor;
            let module_id = module.id;
            if let Some(root) = self.roots.get(&module_id) {
                return Some(*root);
            }
            let key = module_id.to_string();
            if self.inputs.contains_key(&key) {
                return None;
            }
            if let Some(index) = self
                .visiting
                .iter()
                .position(|candidate| candidate == &module_id)
            {
                self.cycle_members
                    .extend(self.visiting[index..].iter().cloned());
                self.cycle_members.insert(module_id.clone());
                return None;
            }
            let parsed = self.main.modules.prepared(&module_id).expect("prepared syntax");
            let source_id = parsed.source_id;
            let recovered_location = parsed.recovered.location;
            let program = parsed.program.as_ref();
            let imports = parsed
                .recovered
                .bindings
                .iter()
                .filter(|binding| {
                    matches!(
                        binding.value.kind,
                        BindingKind::Import | BindingKind::OpenImport
                    )
                })
                .filter_map(|binding| match &binding.value.value.value {
                    ExprKind::String(target) => Some((
                        binding.value.name.value.clone(),
                        binding.value.imported_name.clone(),
                        binding.value.kind == BindingKind::OpenImport,
                        binding.value.value.location,
                        target.clone(),
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let imports_fmt = imports.iter().any(|(_, _, _, _, target)| target == FMT_MODULE);

            self.visiting.push(module_id.clone());
            let mut semantic_imports = Vec::new();
            let mut external_roots = HashMap::new();
            let mut external_interfaces = BTreeMap::new();
            let mut open_candidates: BTreeMap<String, Vec<WorkspaceOpenImportCandidate>> =
                BTreeMap::new();
            let mut diagnostics = parsed.diagnostics.clone();
            if vendor == ModuleVendor::Configured
                && let Some(binding) = program.as_ref().and_then(|program| {
                    program.value.body.value.bindings.iter().find(|binding| {
                        matches!(
                            binding.value.kind,
                            BindingKind::Native | BindingKind::NativeType
                        )
                    })
                })
            {
                diagnostics.push(Diagnostic::error(
                    format!(
                        "native symbol {:?} is only allowed in built-in std modules",
                        binding.value.name.value
                    ),
                    binding.location,
                ));
            }
            if let Some(program) = &program {
                diagnostics.extend(module_binding_diagnostics(program));
                for binding in &program.value.body.value.bindings {
                    if binding.value.kind == BindingKind::Let {
                        diagnostics.push(Diagnostic::error(
                            "module-level let is not supported; use def, or use def name = do { ... } for local computation",
                            binding.location,
                        ));
                    }
                }
                if program.value.authored_result {
                    diagnostics.push(Diagnostic::error(
                        "top-level expressions are not supported; bind the computation with def and export the intended result",
                        program.value.body.value.result.location,
                    ));
                }
            }
            let missing_exports = program.as_ref().is_some_and(|program| {
                !program
                    .value
                    .body
                    .value
                    .bindings
                    .iter()
                    .any(|binding| binding.value.kind == BindingKind::Export)
            });
            if missing_exports {
                let program = program.as_ref().expect("module was parsed");
                diagnostics.push(Diagnostic::error(
                    "module requires at least one explicit export",
                    program.value.body.location,
                ));
            }
            for (name, imported_name, open, location, target) in imports {
                let target_module = match self.main.modules.resolve_import(
                    location,
                ) {
                    Ok(target) => target,
                    Err(error) => {
                        diagnostics.push(Diagnostic::error(error.to_string(), location));
                        continue;
                    }
                };
                if target_module.vendor == ModuleVendor::Builtin {
                    semantic_imports.push(SemanticImport {
                        name: if open { "*".into() } else { name.clone() },
                        location,
                        target: target_module.id.clone(),
                        namespace: !open && imported_name.is_none(),
                    });
                    if let Some(module) = self.builtin_modules.get(&target) {
                        self.install_evidence_roots(
                            module.root,
                            &module.interface,
                            &mut external_roots,
                            &mut diagnostics,
                            location,
                        );
                        if open {
                            match workspace_open_import_exports(
                                &target_module.id,
                                &module.interface,
                                module.root,
                                &self.main.heap,
                            ) {
                                Ok(exports) => {
                                    for (name, candidate) in exports {
                                        open_candidates.entry(name).or_default().push(candidate);
                                    }
                                }
                                Err(error) => {
                                    diagnostics.push(Diagnostic::error(error.to_string(), location))
                                }
                            }
                            continue;
                        }
                        match select_import_root(
                            module.root,
                            module.interface.clone(),
                            imported_name.as_deref(),
                            &name,
                            &self.main.heap,
                        ) {
                            Ok((root, interface)) => {
                                external_roots.insert(name.clone(), root);
                                external_interfaces.insert(name, interface);
                            }
                            Err(error) => {
                                diagnostics.push(Diagnostic::error(error.to_string(), location));
                            }
                        }
                    } else {
                        diagnostics.push(Diagnostic::error(
                            format!("unknown built-in module {target:?}"),
                            location,
                        ));
                    }
                    continue;
                }
                semantic_imports.push(SemanticImport {
                    name: if open { "*".into() } else { name.clone() },
                    location,
                    target: target_module.id.clone(),
                    namespace: !open && imported_name.is_none(),
                });
                let root = match target_module.format {
                    ModuleFormat::Telora => self.load_telora(target_module.clone()).await,
                    ModuleFormat::Json | ModuleFormat::Toml | ModuleFormat::Yaml => {
                        self.load_static_data(target_module.clone()).await
                    }
                };
                if let Some(root) = root {
                    let interface = self
                        .interfaces
                        .get(&target_module.id)
                        .cloned()
                        .unwrap_or_default();
                    self.install_evidence_roots(
                        root,
                        &interface,
                        &mut external_roots,
                        &mut diagnostics,
                        location,
                    );
                    if open {
                        let interface = self
                            .interfaces
                            .get(&target_module.id)
                            .cloned()
                            .unwrap_or_default();
                        match workspace_open_import_exports(
                            &target_module.id,
                            &interface,
                            root,
                            &self.main.heap,
                        ) {
                            Ok(exports) => {
                                for (name, candidate) in exports {
                                    open_candidates.entry(name).or_default().push(candidate);
                                }
                            }
                            Err(error) => {
                                diagnostics.push(Diagnostic::error(error.to_string(), location));
                            }
                        }
                        continue;
                    }
                    let interface = self
                        .interfaces
                        .get(&target_module.id)
                        .cloned()
                        .unwrap_or_default();
                    match select_import_root(
                        root,
                        interface,
                        imported_name.as_deref(),
                        &name,
                        &self.main.heap,
                    ) {
                        Ok((root, interface)) => {
                            external_roots.insert(name.clone(), root);
                            external_interfaces.insert(name.clone(), interface);
                        }
                        Err(error) => {
                            diagnostics.push(Diagnostic::error(error.to_string(), location));
                        }
                    }
                } else {
                    if self.cycle_members.contains(&target_module.id) {
                        if !self.cycle_reported {
                            diagnostics.push(Diagnostic::error(
                                format!("module cycle reaches {}", target_module.id),
                                location,
                            ));
                            self.cycle_reported = true;
                        }
                    } else {
                        diagnostics.push(Diagnostic::error(
                            format!("module {} is unavailable", target_module.id),
                            location,
                        ));
                    }
                }
            }
            if module_id.to_string() != PRELUDE_MODULE
                && let Some(module) = self.builtin_modules.get(PRELUDE_MODULE)
            {
                let provider = ModuleCName::Builtin(PRELUDE_MODULE.into());
                self.install_evidence_roots(
                    module.root,
                    &module.interface,
                    &mut external_roots,
                    &mut diagnostics,
                    recovered_location,
                );
                if let Ok(exports) = workspace_open_import_exports(
                    &provider,
                    &module.interface,
                    module.root,
                    &self.main.heap,
                ) {
                    for (name, candidate) in exports {
                        open_candidates.entry(name).or_insert_with(|| vec![candidate]);
                    }
                }
            }
            if !matches!(module_id, ModuleCName::Builtin(_))
                && !imports_fmt
                && let Some(module) = self.builtin_modules.get(FMT_MODULE)
            {
                for implementation in &module.interface.trait_implementations {
                    match module
                        .root
                        .export_get(&self.main.heap, &implementation.dictionary)
                    {
                        Ok(Some(root)) => {
                            external_roots
                                .entry(implementation.dictionary.clone())
                                .or_insert(root);
                        }
                        Ok(None) => diagnostics.push(Diagnostic::error(
                            "std/fmt is missing a trait implementation root",
                            recovered_location,
                        )),
                        Err(error) => diagnostics.push(Diagnostic::error(
                            error.to_string(),
                            recovered_location,
                        )),
                    }
                }
                external_interfaces
                    .insert(FMT_CAPABILITY_BINDING.into(), module.interface.clone());
            }
            let resolved_id = self.main.modules.id(&module_id).expect("session module");
            let resolution = self.main.resolved.modules[resolved_id.index()].as_ref().expect("session resolve result");
            for (name, mut candidates) in open_candidates {
                if !resolution.imports.contains_key(&name) || external_roots.contains_key(&name) {
                    continue;
                }
                candidates.sort_by(|left, right| left.provider.cmp(&right.provider));
                candidates.dedup_by(|left, right| left.provider == right.provider);
                assert_eq!(candidates.len(), 1, "resolved import must have one runtime provider: {name}");
                let candidate = candidates.into_iter().next().expect("one candidate");
                external_roots.insert(name.clone(), candidate.root);
                external_interfaces.insert(
                    name.clone(),
                    candidate.namespace.unwrap_or_else(|| ModuleInterface {
                        value_binding: Some(name.clone()),
                        type_declarations: if candidate.type_declaration { BTreeSet::from([name.clone()]) } else { BTreeSet::new() },
                        member_constructors: candidate.member_constructor.clone()
                            .map(|constructor| BTreeMap::from([(name.clone(), constructor)])).unwrap_or_default(),
                        namespaces: BTreeMap::new(),
                        exports: candidate.scheme.map(|scheme| BTreeMap::from([(name.clone(), scheme)])).unwrap_or_default(),
                        concrete_types: candidate.concrete_types,
                        traits: candidate
                            .trait_id
                            .map(|id| BTreeMap::from([(name.clone(), id)]))
                            .unwrap_or_default(),
                        trait_implementations: candidate.trait_implementations,
                        type_properties: candidate.type_properties,
                        display_trait: candidate.display_trait,
                        type_family_constructors: candidate
                            .type_family_constructor
                            .map(|family| BTreeMap::from([(name.clone(), family)]))
                            .unwrap_or_default(),
                    }),
                );
            }
            self.visiting.pop();

            let runtime_module_id = self
                .main
                .modules
                .id(&module_id)
                .expect("prepared source has a session ModuleId");
            diagnostics.append(&mut self.main.resolved.modules[runtime_module_id.index()].as_mut()
                .expect("module retains its resolve result before analysis").diagnostics);
            let evaluated = if self.cycle_members.contains(&module_id) || missing_exports {
                ModuleEvaluation::default()
            } else {
                self.analyze_and_evaluate(
                    runtime_module_id,
                    source_id,
                    &module_id,
                    &external_roots,
                    &external_interfaces,
                )
            };
            let parsed = self.main.modules.prepared(&module_id).expect("session syntax");
            let program = parsed.program.as_ref();
            diagnostics.extend(evaluated.diagnostics);
            let analysis = evaluated.analysis;
            // A failed analysis retains the exact session HIR. Do not run a
            // second resolver/type solver against runtime-derived interfaces.
            let partial = analysis.is_none().then(|| {
                let hir = evaluated.untyped_hir.or_else(||
                    self.main.resolved.modules[runtime_module_id.index()].take().map(|resolved| resolved.hir))
                    .expect("unsolved module retains its session HIR");
                crate::types::PartialAnalysis::from_resolved(hir)
            });
            // Availability describes whether the source Module exists. Failed,
            // unknown and incomputable facts remain properties of its graph.
            let state = WorkspaceModuleState::Available;
            self.inputs.insert(
                key.clone(),
                SemanticModuleInput {
                    key: key.clone(),
                    path: Some(path.clone()),
                    kind: WorkspaceModuleKind::Telora,
                    source: Some(source_id),
                    result_location: program.map(|program| program.value.body.value.result.location),
                    analysis,
                    partial,
                    interface: None,
                    state,
                    imports: semantic_imports,
                    diagnostics,
                },
            );
            if let Some(root) = evaluated.root {
                let interface = self.inputs[&key]
                    .analysis
                    .as_ref()
                    .expect("strict module has analysis")
                    .module_interface
                    .clone();
                self.interfaces.insert(module_id.clone(), interface);
                self.roots.insert(module_id.clone(), root);
                Some(root)
            } else {
                None
            }
        })
    }

    async fn load_static_data(&mut self, module: ResolvedModule) -> Option<PersistentValue> {
        let path = module.path()?.to_owned();
        let module_id = module.id;
        if let Some(root) = self.roots.get(&module_id) {
            return Some(*root);
        }
        let key = module_id.to_string();
        if self.inputs.contains_key(&key) {
            return None;
        }
        let source = match self.overlays.get(&path).cloned() {
            Some(source) if source.byte_len() <= self.engine.config.data_limits.file_size => source,
            Some(_) => {
                let kind = static_data_kind(module.format)?;
                self.inputs
                    .insert(key.clone(), unavailable_input(key, path.clone(), kind));
                return None;
            }
            None => match read_data_file(
                &path,
                self.engine.config.data_limits.file_size,
                &key,
            ) {
                Ok(source) => crate::document::DocumentText::new(source),
                Err(_) => {
                    let kind = static_data_kind(module.format)?;
                    self.inputs
                        .insert(key.clone(), unavailable_input(key, path.clone(), kind));
                    return None;
                }
            },
        };
        let source_id = self.sources.add_document(key.clone(), source);
        let (_, descriptor) = semantic_value_contract(&self.builtin_modules, &self.main.heap)
            .expect("std/value provides the static data interface");
        let parsed = parse_static_data_registered(module.format, &self.sources, source_id)?;
        let plan = parsed.plan;
        let interface = static_data_interface(descriptor);
        self.inputs.insert(
            key.clone(),
            SemanticModuleInput {
                key: key.clone(),
                path: Some(path),
                kind: parsed.kind,
                source: Some(source_id),
                result_location: None,
                analysis: None,
                partial: None,
                interface: Some(SemanticModuleInterface::new(&interface)),
                state: WorkspaceModuleState::Available,
                imports: Vec::new(),
                diagnostics: parsed.diagnostics,
            },
        );
        if let Some(plan) = plan {
            let source_len = self.sources.get(source_id).text().byte_len();
            let (root, interface, provenance) = match publish_static_data_module(
                &plan,
                &self.builtin_modules,
                &mut self.main.heap,
                source_len,
                self.engine.config.data_limits,
            ) {
                Ok(published) => published,
                Err(error) => {
                    let location = crate::Location::from_usize(source_id, 0..source_len)
                        .expect("registered source range fits Location");
                    self.inputs
                        .get_mut(&key)
                        .expect("static data input was inserted")
                        .diagnostics
                        .push(Diagnostic::error(error.to_string(), location));
                    return None;
                }
            };
            self.interfaces.insert(module_id.clone(), interface);
            self.roots.insert(module_id.clone(), root);
            self.provenances.insert(module_id, provenance);
            return Some(root);
        }
        None
    }

    fn analyze_and_evaluate(
        &mut self,
        module_id: ModuleId,
        source_id: crate::SourceId,
        cname: &ModuleCName,
        external_roots: &HashMap<String, PersistentValue>,
        external_interfaces: &BTreeMap<String, ModuleInterface>,
    ) -> ModuleEvaluation {
        let Some(program) = self.main.modules.prepared(cname)
            .expect("session syntax").program.as_ref() else {
                return ModuleEvaluation::default();
            };
        let mut account = QuotaAccount::new(self.engine.config.module_quota).with_sources(&self.sources);
        let source = self.sources.get(source_id);
        let mut hir = Some(self.main.resolved.modules[module_id.index()].take()
            .expect("source module has session-resolved HIR").hir);
        let dependency_facts = self.main.modules.module(module_id).imports.iter().filter_map(|edge| {
            let provider = &self.main.modules.module(edge.target).cname;
            self.builtin_modules.get(&provider.to_string()).map(|module| &module.interface)
                .or_else(|| self.interfaces.get(provider)).map(ModuleInterface::type_facts)
        }).collect::<Vec<_>>();
        let analysis = match analyze_program_with_bindings_observed(
            &source.name,
            module_id,
            ModuleAnalysisContext::Ordinary,
            program,
            &mut hir,
            &mut account,
            &external_roots
                .iter()
                .map(|(name, root)| (name.clone(), *root))
                .collect(),
            &HashSet::new(),
            &self.sources,
            &BTreeMap::new(),
            external_interfaces,
            &self.engine.debug_sink,
            &mut self.main.heap,
            &mut self.main.types,
            &dependency_facts,
        ) {
            Ok(analysis) => analysis,
            Err(error) => {
                return ModuleEvaluation {
                    untyped_hir: hir,
                    diagnostics: vec![frontend_diagnostic(error, source_id, program)],
                    ..Default::default()
                };
            }
        };
        let mut execution_roots = external_roots.clone();
        install_type_family_roots(&mut execution_roots, &analysis);
        let static_funcs = self.main.modules.static_funcs(module_id);
        let metadata = metadata_compilation_plan(program);
        let promoted_types = metadata
            .as_ref()
            .map(|metadata| metadata.type_names.iter().cloned().collect())
            .unwrap_or_default();
        let erased_bindings = metadata
            .map(|metadata| metadata.erased_bindings)
            .unwrap_or_default();
        let function = match compile_program_with_promoted_types_and_static_funcs(
            source,
            &runtime_program_with_evidence(program, &analysis),
            &analysis,
            &promoted_types,
            &erased_bindings,
            &static_funcs,
        ) {
            Ok(function) => function,
            Err(error) => {
                return ModuleEvaluation::analyzed(
                    analysis,
                    frontend_diagnostic(error, source_id, program),
                );
            }
        };
        let inherited_failure_count = self.main.failures.len();
        let execution = match Vm::new()
            .with_debug_sink(Arc::clone(&self.engine.debug_sink))
            .execute_in_work_best_effort_with_failures(
                &self.main.heap,
                &runtime_roots(&execution_roots),
                &function,
                &[],
                &mut account,
                inherited_failure_count,
            ) {
            Ok(execution) => execution,
            Err(failure) => {
                let mut diagnostics = account.take_diagnostics();
                merge_runtime_errors(&mut diagnostics, failure.failures);
                if let Some(diagnostic) = failure.error.diagnostic() {
                    merge_runtime_diagnostics(&mut diagnostics, [diagnostic]);
                } else if failure.error.propagated_failure().is_none() {
                    merge_runtime_diagnostics(
                        &mut diagnostics,
                        [Diagnostic::error(
                            failure.error.to_string(),
                            program.location,
                        )],
                    );
                }
                return ModuleEvaluation {
                    analysis: Some(analysis),
                    root: None,
                    diagnostics,
                    untyped_hir: None,
                };
            }
        };
        let mut diagnostics = Vec::new();
        merge_runtime_diagnostics(&mut diagnostics, account.take_diagnostics());
        merge_runtime_errors(&mut diagnostics, execution.failures.clone());
        let failures = execution.failures;
        let root = if analysis.explicit_exports {
            execution.world.publish_module(&mut self.main.heap)
        } else {
            execution.world.publish(&mut self.main.heap)
        };
        match root {
            Ok(root) => {
                self.main.failures.extend(failures);
                ModuleEvaluation {
                    analysis: Some(analysis),
                    root: Some(root),
                    diagnostics,
                    untyped_hir: None,
                }
            }
            Err(error) => {
                merge_runtime_diagnostics(
                    &mut diagnostics,
                    [Diagnostic::error(error.to_string(), program.location)],
                );
                ModuleEvaluation {
                    analysis: Some(analysis),
                    root: None,
                    diagnostics,
                    untyped_hir: None,
                }
            }
        }
    }
}

#[derive(Default)]
struct ModuleEvaluation {
    analysis: Option<crate::Analysis>,
    untyped_hir: Option<crate::hir::HirProgram>,
    root: Option<PersistentValue>,
    diagnostics: Vec<Diagnostic>,
}

impl ModuleEvaluation {
    fn analyzed(analysis: crate::Analysis, diagnostic: Diagnostic) -> Self {
        Self {
            analysis: Some(analysis),
            diagnostics: vec![diagnostic],
            ..Self::default()
        }
    }
}

fn frontend_diagnostic(
    error: crate::FrontendError,
    source: crate::SourceId,
    program: &Program,
) -> Diagnostic {
    error
        .diagnostic
        .map(|diagnostic| *diagnostic)
        .unwrap_or_else(|| {
            let offset = u32::try_from(error.location.offset).unwrap_or(program.location.start);
            Diagnostic::error(
                error.message,
                crate::Location::new(source, crate::TextRange::at(offset)),
            )
        })
}

fn merge_runtime_errors(diagnostics: &mut Vec<Diagnostic>, errors: Vec<crate::RuntimeError>) {
    merge_runtime_diagnostics(
        diagnostics,
        errors.into_iter().filter_map(|error| error.diagnostic()),
    );
}

fn merge_runtime_diagnostics(
    diagnostics: &mut Vec<Diagnostic>,
    emitted: impl IntoIterator<Item = Diagnostic>,
) {
    for diagnostic in emitted {
        if let Some(existing) = diagnostics
            .iter_mut()
            .find(|existing| same_runtime_diagnostic(existing, &diagnostic))
        {
            if existing.labels.len() < diagnostic.labels.len() {
                *existing = diagnostic;
            }
        } else {
            diagnostics.push(diagnostic);
        }
    }
}

fn same_runtime_diagnostic(left: &Diagnostic, right: &Diagnostic) -> bool {
    if left.severity != right.severity || left.message != right.message {
        return false;
    }
    let primary = |diagnostic: &Diagnostic| {
        diagnostic
            .labels
            .iter()
            .find(|label| label.primary)
            .map(|label| label.location)
    };
    match (primary(left), primary(right)) {
        (Some(left), Some(right)) if left.source == right.source && left.start == right.start => {
            return true;
        }
        (None, _) | (_, None) => return left.labels == right.labels,
        _ => {}
    }
    let compact_matches = |compact: &Diagnostic, rich: &Diagnostic| {
        compact.labels.len() == 1
            && rich.labels.iter().any(|label| {
                label.location.source == compact.labels[0].location.source
                    && label.location.start == compact.labels[0].location.start
            })
    };
    compact_matches(left, right) || compact_matches(right, left)
}

fn block_on_recovery<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};

    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}

fn unavailable_input(key: String, path: PathBuf, kind: WorkspaceModuleKind) -> SemanticModuleInput {
    SemanticModuleInput {
        key,
        path: Some(path),
        kind,
        source: None,
        result_location: None,
        analysis: None,
        partial: None,
        interface: None,
        state: WorkspaceModuleState::Unavailable,
        imports: Vec::new(),
        diagnostics: Vec::new(),
    }
}

/// Evaluates a source file as an isolated expression harness.
///
/// This testing API accepts the historical final-expression form. Production
/// modules must be loaded through [`Engine::load_module`].
pub fn evaluate_expression_module(
    path: impl AsRef<Path>,
    external_bindings: BTreeMap<String, crate::DataWorld>,
    evaluation_fuel: usize,
) -> Result<LoadedModule, ModuleError> {
    evaluate_expression_module_with_quota(
        path,
        external_bindings,
        Quota::with_fuel(evaluation_fuel),
    )
}

pub fn evaluate_expression_module_with_quota(
    path: impl AsRef<Path>,
    external_bindings: BTreeMap<String, crate::DataWorld>,
    module_quota: Quota,
) -> Result<LoadedModule, ModuleError> {
    evaluate_expression_module_with_quota_and_debug_sink(
        path,
        external_bindings,
        module_quota,
        Arc::new(DiscardDebugSink),
    )
}

pub fn evaluate_expression_module_with_quota_and_debug_sink(
    path: impl AsRef<Path>,
    external_bindings: BTreeMap<String, crate::DataWorld>,
    module_quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
) -> Result<LoadedModule, ModuleError> {
    load_module_with_policy(
        path,
        external_bindings,
        module_quota,
        DataLimits::default(),
        debug_sink,
        ModuleSourcePolicy::ExpressionHarness,
    )
}
