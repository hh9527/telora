struct WorkspaceBuilder<'a> {
    engine: &'a Engine,
    resolver: ModuleResolver,
    overlays: &'a BTreeMap<PathBuf, crate::document::DocumentText>,
    query: Option<&'a crate::query::QueryContext>,
    sources: SourceDatabase,
    main: MainWorld,
    core_modules: HashMap<String, ModuleArtifact>,
    inputs: BTreeMap<String, SemanticModuleInput>,
    provenances: HashMap<ModuleCName, Provenance>,
    roots: HashMap<ModuleCName, PersistentValue>,
    interfaces: HashMap<ModuleCName, ModuleInterface>,
    visiting: Vec<ModuleCName>,
    cycle_members: HashSet<ModuleCName>,
    cycle_reported: bool,
}

impl WorkspaceBuilder<'_> {
    fn load_telora<'a>(
        &'a mut self,
        module: ResolvedModule,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<PersistentValue>> + 'a>> {
        Box::pin(async move {
            if let Some(context) = self.query
                && context.checkpoint().await.is_err()
            {
                return None;
            }
            let path = module.path()?.to_owned();
            let authority = module.authority;
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
            let source = match self.overlays.get(&path).cloned() {
                Some(source) => source,
                None => match fs::read_to_string(&path) {
                    Ok(source) => crate::document::DocumentText::new(source),
                    Err(error) => {
                        self.inputs.insert(
                            key.clone(),
                            unavailable_input(key, path.clone(), WorkspaceModuleKind::Telora),
                        );
                        let _ = error;
                        return None;
                    }
                },
            };
            let source_id = self
                .sources
                .add_document(path.display().to_string(), source);
            let parsed = parse_registered(&self.sources, source_id);
            let invalid_scoped_options = parsed
                .options
                .iter()
                .filter(|_| !self.resolver.is_root(&module_id))
                .cloned()
                .collect::<Vec<_>>();
            let program = parsed.program.clone();
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

            self.visiting.push(module_id.clone());
            let mut semantic_imports = Vec::new();
            let mut external_roots = HashMap::new();
            let mut external_interfaces = BTreeMap::new();
            let mut unavailable_imports = HashSet::new();
            let mut open_candidates: BTreeMap<String, Vec<WorkspaceOpenImportCandidate>> =
                BTreeMap::new();
            let mut diagnostics = Vec::new();
            for option in &invalid_scoped_options {
                diagnostics.push(Diagnostic::error(
                    format!(
                        "option {:?} is only allowed in the selected root",
                        option.key.value
                    ),
                    option.location,
                ));
            }
            if authority == ModuleAuthority::Ordinary
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
                        "native symbol {:?} is only allowed in built-in or *.native.telora modules",
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
                let target_module = match self.resolver.resolve_import(&module_id, &target) {
                    Ok(target) => target,
                    Err(error) => {
                        unavailable_imports.insert(name.clone());
                        diagnostics.push(Diagnostic::error(error.to_string(), location));
                        continue;
                    }
                };
                if target_module.authority == ModuleAuthority::RuntimeSystem {
                    semantic_imports.push(SemanticImport {
                        name: if open { "*".into() } else { name.clone() },
                        location,
                        target: target_module.id.clone(),
                        namespace: !open && imported_name.is_none(),
                    });
                    if let Some(module) = self.core_modules.get(&target) {
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
                                unavailable_imports.insert(name);
                                diagnostics.push(Diagnostic::error(error.to_string(), location));
                            }
                        }
                    } else {
                        unavailable_imports.insert(name);
                        diagnostics.push(Diagnostic::error(
                            format!("unknown built-in module {target:?}"),
                            location,
                        ));
                    }
                    continue;
                }
                let target_path = target_module
                    .path()
                    .expect("local import resolves to a local source")
                    .to_owned();
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
                            unavailable_imports.insert(name);
                            diagnostics.push(Diagnostic::error(error.to_string(), location));
                        }
                    }
                } else {
                    unavailable_imports.insert(name);
                    if self.cycle_members.contains(&target_module.id) {
                        if !self.cycle_reported {
                            diagnostics.push(Diagnostic::error(
                                format!("module cycle reaches {}", target_path.display()),
                                location,
                            ));
                            self.cycle_reported = true;
                        }
                    } else {
                        diagnostics.push(Diagnostic::error(
                            format!("module {} is unavailable", target_path.display()),
                            location,
                        ));
                    }
                }
            }
            if module_id.to_string() != PRELUDE_MODULE
                && let Some(module) = self.core_modules.get(PRELUDE_MODULE)
            {
                let provider = ModuleCName::Builtin(PRELUDE_MODULE.into());
                if let Ok(exports) = workspace_open_import_exports(
                    &provider,
                    &module.interface,
                    module.root,
                    &self.main.heap,
                ) {
                    for (name, candidate) in exports {
                        open_candidates.entry(name).or_default().push(candidate);
                    }
                }
            }
            let explicit_names = parsed
                .recovered
                .bindings
                .iter()
                .filter(|binding| {
                    !matches!(
                        binding.value.kind,
                        BindingKind::OpenImport | BindingKind::Export
                    )
                })
                .map(|binding| binding.value.name.value.as_str())
                .collect::<HashSet<_>>();
            for (name, mut candidates) in open_candidates {
                if explicit_names.contains(name.as_str()) || external_roots.contains_key(&name) {
                    continue;
                }
                candidates.sort_by(|left, right| left.provider.cmp(&right.provider));
                candidates.dedup_by(|left, right| left.provider == right.provider);
                if candidates.len() > 1 {
                    let providers = candidates
                        .iter()
                        .map(|candidate| candidate.provider.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    for location in recovered_reference_locations(&parsed.recovered, &name) {
                        diagnostics.push(Diagnostic::error(
                            format!("open import name {name:?} is ambiguous between {providers}"),
                            location,
                        ));
                    }
                    continue;
                }
                let candidate = candidates.into_iter().next().expect("one candidate");
                external_roots.insert(name.clone(), candidate.root);
                external_interfaces.insert(
                    name.clone(),
                    ModuleInterface {
                        exports: BTreeMap::from([(name.clone(), candidate.scheme)]),
                        concrete_types: candidate.concrete_types,
                        type_family_templates: candidate
                            .type_family_template
                            .map(|family| BTreeMap::from([(name.clone(), family)]))
                            .unwrap_or_default(),
                    },
                );
            }
            self.visiting.pop();

            if let Some(context) = self.query
                && context.checkpoint().await.is_err()
            {
                return None;
            }

            let partial = analyze_partial_types_recovered_with_query(
                &self.sources,
                source_id,
                &parsed.recovered,
                parsed.diagnostics,
                self.engine.config.module_quota,
                &external_roots
                    .iter()
                    .map(|(name, root)| (name.clone(), *root))
                    .collect(),
                &mut self.main.heap,
                PartialAnalysisControl {
                    unavailable_imports: &unavailable_imports,
                    query: self.query,
                },
            );
            let runtime_module_id = self
                .main
                .modules
                .id(&module_id)
                .unwrap_or(ModuleId::ANONYMOUS);
            let evaluated = if self.cycle_members.contains(&module_id)
                || !invalid_scoped_options.is_empty()
                || missing_exports
            {
                ModuleEvaluation::default()
            } else {
                program
                    .as_ref()
                    .map_or_else(ModuleEvaluation::default, |program| {
                        self.analyze_and_evaluate(
                            runtime_module_id,
                            source_id,
                            program,
                            &external_roots,
                            &external_interfaces,
                        )
                    })
            };
            diagnostics.extend(evaluated.diagnostics);
            let analysis = evaluated.analysis;
            let partial = analysis.is_none().then_some(partial);
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
                    program,
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
        if let Some(context) = self.query
            && context.checkpoint().await.is_err()
        {
            return None;
        }
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
            None => match read_data_file(&path, self.engine.config.data_limits.file_size) {
                Ok(source) => crate::document::DocumentText::new(source),
                Err(_) => {
                    let kind = static_data_kind(module.format)?;
                    self.inputs
                        .insert(key.clone(), unavailable_input(key, path.clone(), kind));
                    return None;
                }
            },
        };
        let source_id = self
            .sources
            .add_document(path.display().to_string(), source);
        let (_, descriptor) = semantic_value_contract(&self.core_modules, &self.main.heap)
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
                program: None,
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
                &self.core_modules,
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
        program: &Program,
        external_roots: &HashMap<String, PersistentValue>,
        external_interfaces: &BTreeMap<String, ModuleInterface>,
    ) -> ModuleEvaluation {
        let mut account = QuotaAccount::new(self.engine.config.module_quota);
        if let Some(query) = self.query {
            account = account.with_query(query.clone());
        }
        let source = self.sources.get(source_id);
        let analysis = match analyze_program_with_bindings_observed(
            &source.name,
            module_id,
            program,
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
        ) {
            Ok(analysis) => analysis,
            Err(error) => {
                return ModuleEvaluation::failed(frontend_diagnostic(error, source_id, program));
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
            program,
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
                }
            }
        }
    }
}

#[derive(Default)]
struct ModuleEvaluation {
    analysis: Option<crate::Analysis>,
    root: Option<PersistentValue>,
    diagnostics: Vec<Diagnostic>,
}

impl ModuleEvaluation {
    fn failed(diagnostic: Diagnostic) -> Self {
        Self {
            diagnostics: vec![diagnostic],
            ..Self::default()
        }
    }

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
        program: None,
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
    load_module_with_native_modules(
        path,
        external_bindings,
        module_quota,
        DataLimits::default(),
        debug_sink,
        &[],
        ModuleSourcePolicy::ExpressionHarness,
    )
}
