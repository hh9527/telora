struct SelectedEntryLoader {
    loader: ModuleLoader,
    main_module: ResolvedModule,
    main_path: PathBuf,
    entry: CompiledTeloraModule,
}

fn prepare_selected_entry(
    resolver: ModuleResolver,
    entry_id: ModuleCName,
    source: &str,
    module_quota: Quota,
    data_limits: DataLimits,
    debug_sink: Arc<dyn DebugSink>,
    native_modules: &[RegisteredNativeModule],
) -> Result<SelectedEntryLoader, ModuleError> {
    let injected_modules = BTreeMap::from([(
        EDGE_RUNTIME_MODULE.to_owned(),
        edge_runtime_source().to_owned(),
    )]);
    let resolver = resolver
        .with_builtins(builtin_list(native_modules))
        .with_entry_context(entry_id.clone(), injected_modules.keys().cloned());
    let main_module = resolver
        .selected_root()
        .map_err(|error| ModuleError::new(error.to_string()))?;
    if main_module.format != ModuleFormat::Telora {
        return Err(ModuleError::new(
            "main module must have a .telora extension",
        ));
    }
    let main_path = main_module
        .path()
        .ok_or_else(|| ModuleError::new("main module has no physical path"))?
        .to_owned();
    let mut synthetic = injected_modules
        .iter()
        .map(|(name, source)| {
            (
                ModuleCName::builtin(name),
                (main_path.clone(), source.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    synthetic.insert(entry_id.clone(), (main_path.clone(), source.to_owned()));
    let opaque_modules = builtin_list(native_modules)
        .into_iter()
        .map(|(name, _)| ModuleCName::builtin(name));
    let graph = ModuleGraph::discover(
        &resolver,
        vec![main_module.clone()],
        &synthetic,
        opaque_modules,
        None,
        false,
    )?;
    let mut main = MainWorld::with_modules(graph);
    let mut sources = SourceDatabase::default();
    let core_modules =
        install_native_modules(&mut main, &mut sources, &debug_sink, native_modules)?;
    let mut loader = ModuleLoader {
        resolver,
        cache: HashMap::new(),
        core_modules,
        main,
        visiting: Vec::new(),
        dependencies: BTreeSet::new(),
        module_quota,
        data_limits,
        debug_sink,
        sources,
        semantic_inputs: BTreeMap::new(),
        source_policy: ModuleSourcePolicy::ExplicitExports,
    };
    loader.install_injected_modules(&main_path, injected_modules)?;
    let entry = loader.compile_entry(&main_path, entry_id, source, BTreeMap::new())?;
    Ok(SelectedEntryLoader {
        loader,
        main_module,
        main_path,
        entry,
    })
}

struct ModuleLoader {
    resolver: ModuleResolver,
    cache: HashMap<ModuleCName, ModuleState>,
    core_modules: HashMap<String, ModuleArtifact>,
    main: MainWorld,
    visiting: Vec<ModuleCName>,
    dependencies: BTreeSet<PathBuf>,
    module_quota: Quota,
    data_limits: DataLimits,
    debug_sink: Arc<dyn DebugSink>,
    sources: SourceDatabase,
    semantic_inputs: BTreeMap<String, SemanticModuleInput>,
    source_policy: ModuleSourcePolicy,
}

#[derive(Clone)]
enum ModuleState {
    Ready(ModuleArtifact),
}

impl ModuleLoader {
    fn install_injected_modules(
        &mut self,
        context_path: &Path,
        modules: BTreeMap<String, String>,
    ) -> Result<(), ModuleError> {
        for (name, source) in modules {
            let module_id = ModuleCName::builtin(&name);
            let mut account = QuotaAccount::new(self.module_quota);
            let compiled = self.compile_telora(
                &module_id,
                ModuleAuthority::RuntimeSystem,
                TeloraModuleSource::Synthetic {
                    name: &name,
                    context_path,
                    source: &source,
                },
                BTreeMap::new(),
                false,
                &mut account,
            )?;
            let arena = Vm::new()
                .with_debug_sink(Arc::clone(&self.debug_sink))
                .execute_in_work(
                    &self.main.heap,
                    &compiled.externals,
                    &compiled.function,
                    &[],
                    &mut account,
                )
                .map_err(|error| ModuleError::new(error.with_sources(&self.sources).to_string()))?;
            let root = arena
                .publish_module(&mut self.main.heap)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            self.core_modules.insert(
                name,
                ModuleArtifact {
                    root,
                    interface: compiled.analysis.module_interface,
                    provenance: None,
                },
            );
        }
        Ok(())
    }

    fn compile_entry(
        &mut self,
        main_path: &Path,
        module_id: ModuleCName,
        entry_source: &str,
        external_bindings: BTreeMap<String, crate::DataWorld>,
    ) -> Result<CompiledTeloraModule, ModuleError> {
        self.enter(&module_id)?;
        let mut account = QuotaAccount::new(self.module_quota);
        let source_name = module_id.to_string();
        let result = self.compile_telora(
            &module_id,
            ModuleAuthority::RuntimeSystem,
            TeloraModuleSource::Synthetic {
                name: &source_name,
                context_path: main_path,
                source: entry_source,
            },
            external_bindings,
            true,
            &mut account,
        );
        self.leave(&module_id);
        result
    }

    fn load_root(
        &mut self,
        module: ResolvedModule,
        external_bindings: BTreeMap<String, crate::DataWorld>,
    ) -> Result<LoadedModule, ModuleError> {
        let (path, compiled) = self.compile_root(module, external_bindings)?;
        let CompiledTeloraModule {
            analysis,
            function,
            externals,
            options,
        } = compiled;
        let workspace = WorkspaceSnapshot::build(
            self.sources.clone(),
            self.semantic_inputs.values().cloned().collect(),
        );
        let main = std::mem::replace(&mut self.main, MainWorld::building()).seal();
        Ok(LoadedModule {
            path,
            dependencies: self.dependencies.iter().cloned().collect(),
            analysis,
            function,
            sources: self.sources.clone(),
            workspace,
            options,
            runtime: Arc::new(ModuleRuntime {
                main: Arc::new(main),
                externals,
            }),
        })
    }

    fn compile_root(
        &mut self,
        module: ResolvedModule,
        external_bindings: BTreeMap<String, crate::DataWorld>,
    ) -> Result<(PathBuf, CompiledTeloraModule), ModuleError> {
        let path = module
            .path()
            .expect("root module has a physical path")
            .to_owned();
        let authority = module.authority;
        let module_id = module.id;
        self.dependencies.insert(path.clone());
        self.enter(&module_id)?;
        let mut account = QuotaAccount::new(self.module_quota);
        let result = self.compile_telora(
            &module_id,
            authority,
            TeloraModuleSource::File(&path),
            external_bindings,
            true,
            &mut account,
        );
        self.leave(&module_id);
        result.map(|compiled| (path, compiled))
    }

    #[cfg(test)]
    fn load_value(&mut self, path: &Path) -> Result<ModuleArtifact, ModuleError> {
        let module = self
            .resolver
            .resolve_root(path)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        self.load_resolved_value(module)
    }

    fn load_resolved_value(
        &mut self,
        module: ResolvedModule,
    ) -> Result<ModuleArtifact, ModuleError> {
        let format = module.format;
        let authority = module.authority;
        let path = module
            .path()
            .expect("source module has a physical path")
            .to_owned();
        let module_id = module.id;
        if let Some(ModuleState::Ready(artifact)) = self.cache.get(&module_id) {
            return Ok(artifact.clone());
        }
        self.enter(&module_id)?;
        self.dependencies.insert(path.clone());
        let result: Result<ModuleArtifact, ModuleError> = match format {
            ModuleFormat::Json | ModuleFormat::Toml | ModuleFormat::Yaml => (|| {
                let source = read_data_file(&path, self.data_limits.file_size)?;
                let source_id = self.sources.add(path.display().to_string(), source);
                let StaticDataParse {
                    plan,
                    diagnostics,
                    kind,
                } = parse_static_data_registered(format, &self.sources, source_id)
                    .expect("static data format has a frontend");
                plan.ok_or_else(|| {
                    ModuleError::new(
                        diagnostics
                            .iter()
                            .map(|diagnostic| self.sources.render(diagnostic))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                })
                .and_then(|plan| {
                    let (root, interface, provenance) = publish_static_data_module(
                        &plan,
                        &self.core_modules,
                        &mut self.main.heap,
                        self.sources.get(source_id).text().byte_len(),
                        self.data_limits,
                    )?;
                    let key = module_id.to_string();
                    self.semantic_inputs.insert(
                        key.clone(),
                        SemanticModuleInput {
                            key,
                            path: Some(path.clone()),
                            kind,
                            source: Some(source_id),
                            program: None,
                            analysis: None,
                            partial: None,
                            interface: Some(SemanticModuleInterface::new(&interface)),
                            state: crate::semantic::WorkspaceModuleState::Available,
                            imports: Vec::new(),
                            diagnostics: Vec::new(),
                        },
                    );
                    Ok(ModuleArtifact {
                        root,
                        interface,
                        provenance: Some(provenance),
                    })
                })
            })(),
            ModuleFormat::Telora => {
                let mut account = QuotaAccount::new(self.module_quota);
                self.compile_telora(
                    &module_id,
                    authority,
                    TeloraModuleSource::File(&path),
                    BTreeMap::new(),
                    false,
                    &mut account,
                )
                .and_then(|compiled| {
                    let CompiledTeloraModule {
                        analysis,
                        function,
                        externals,
                        ..
                    } = compiled;
                    let arena = Vm::new()
                        .with_debug_sink(Arc::clone(&self.debug_sink))
                        .execute_in_work(&self.main.heap, &externals, &function, &[], &mut account)
                        .map_err(|error| {
                            ModuleError::new(error.with_sources(&self.sources).to_string())
                        })?;
                    let root = if analysis.explicit_exports {
                        arena.publish_module(&mut self.main.heap)
                    } else {
                        arena.publish(&mut self.main.heap)
                    }
                    .map_err(|error| ModuleError::new(error.to_string()))?;
                    Ok(ModuleArtifact {
                        root,
                        interface: analysis.module_interface,
                        provenance: None,
                    })
                })
            }
        };
        self.leave(&module_id);
        let artifact = result?;
        self.cache
            .insert(module_id, ModuleState::Ready(artifact.clone()));
        Ok(artifact)
    }

    fn compile_telora(
        &mut self,
        module_id: &ModuleCName,
        authority: ModuleAuthority,
        module_source: TeloraModuleSource<'_>,
        external_bindings: BTreeMap<String, crate::DataWorld>,
        is_root: bool,
        account: &mut QuotaAccount,
    ) -> Result<CompiledTeloraModule, ModuleError> {
        let path = module_source.context_path();
        let synthetic = matches!(module_source, TeloraModuleSource::Synthetic { .. });
        let (source_name, source) = match module_source {
            TeloraModuleSource::File(path) => (module_id.to_string(), read(path)?),
            TeloraModuleSource::Synthetic { name, source, .. } => {
                (name.to_owned(), source.to_owned())
            }
        };
        let source_id = self.sources.add(source_name.clone(), source);
        let parsed = parse_registered(&self.sources, source_id);
        if let Some(option) = parsed
            .options
            .iter()
            .find(|option| option.key.value.starts_with("crate.") && !self.resolver.is_standalone())
        {
            return Err(ModuleError::new(self.sources.render(&Diagnostic::error(
                format!(
                    "resolver option {:?} is only allowed in standalone mode",
                    option.key.value
                ),
                option.location,
            ))));
        }
        if let Some(option) = parsed
            .options
            .iter()
            .find(|_| !self.resolver.is_root(module_id))
        {
            return Err(ModuleError::new(self.sources.render(&Diagnostic::error(
                format!(
                    "option {:?} is only allowed in the selected root",
                    option.key.value
                ),
                option.location,
            ))));
        }
        let program = parsed.program.ok_or_else(|| {
            ModuleError::new(
                parsed
                    .diagnostics
                    .iter()
                    .map(|diagnostic| self.sources.render(diagnostic))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })?;
        if let Some(diagnostic) = module_binding_diagnostics(&program).into_iter().next() {
            return Err(ModuleError::new(self.sources.render(&diagnostic)));
        }
        let skeleton = if self.main.modules.modules.is_empty() {
            None
        } else {
            let id = self.main.modules.id(module_id).ok_or_else(|| {
                ModuleError::new(format!(
                    "module {module_id} was not present during module graph discovery"
                ))
            })?;
            let skeleton = self.main.modules.module(id).clone();
            let parsed_blueprint = ModuleBlueprint::from_program(&program).map_err(|message| {
                ModuleError::new(format!(
                    "module {module_id} has an invalid skeleton: {message}"
                ))
            })?;
            if skeleton.id != id
                || skeleton.cname != *module_id
                || skeleton.exports != parsed_blueprint.exports
                || skeleton.slots != parsed_blueprint.slots
            {
                return Err(ModuleError::new(format!(
                    "module {module_id} changed after its static skeleton was assigned"
                )));
            }
            Some(skeleton)
        };
        let options = parsed
            .options
            .iter()
            .map(|option| {
                immediate_value(&option.value)
                    .map(|value| LoadedOptionAction {
                        key: option.key.value.clone(),
                        value,
                    })
                    .map_err(|error| {
                        ModuleError::new(
                            self.sources
                                .render(&Diagnostic::error(error.to_string(), option.location)),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let has_explicit_exports = program
            .value
            .body
            .value
            .bindings
            .iter()
            .any(|binding| binding.value.kind == BindingKind::Export);
        if self.source_policy == ModuleSourcePolicy::ExplicitExports
            && let Some(binding) = program
                .value
                .body
                .value
                .bindings
                .iter()
                .find(|binding| binding.value.kind == BindingKind::Let)
        {
            let message = "module-level let is not supported; use def, or use def name = do { ... } for local computation";
            return Err(ModuleError::new(
                self.sources
                    .render(&Diagnostic::error(message, binding.location)),
            ));
        }
        if self.source_policy == ModuleSourcePolicy::ExplicitExports
            && program.value.authored_result
        {
            let message = "top-level expressions are not supported; bind the computation with def and export the intended result";
            return Err(ModuleError::new(self.sources.render(&Diagnostic::error(
                message,
                program.value.body.value.result.location,
            ))));
        }
        if self.source_policy == ModuleSourcePolicy::ExplicitExports && !has_explicit_exports {
            let message = "module requires at least one explicit export";
            return Err(ModuleError::new(
                self.sources
                    .render(&Diagnostic::error(message, program.value.body.location)),
            ));
        }
        reject_nested_imports(&program, &source_name)?;
        let mut external_provenance = BTreeMap::new();
        let mut external_roots = HashMap::new();
        for (name, value) in &external_bindings {
            let root = value
                .publish(&mut self.main.heap)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            external_roots.insert(name.clone(), root);
        }
        let mut semantic_imports = Vec::new();
        let mut graph_imports = Vec::new();
        let mut external_interfaces = BTreeMap::new();
        let mut open_candidates: BTreeMap<String, Vec<OpenImportCandidate>> = BTreeMap::new();
        let mut direct_import_names = external_bindings.keys().cloned().collect::<HashSet<_>>();

        for binding in &program.value.body.value.bindings {
            if !matches!(
                binding.value.kind,
                BindingKind::Import | BindingKind::OpenImport
            ) {
                continue;
            }
            if binding.value.kind == BindingKind::Import
                && !direct_import_names.insert(binding.value.name.value.clone())
            {
                return Err(ModuleError::new(format!(
                    "duplicate module binding {:?} in {source_name}",
                    binding.value.name.value
                )));
            }
            let ExprKind::String(relative) = &binding.value.value.value else {
                return Err(ModuleError::new("import path must be a string"));
            };
            let imported = self
                .resolver
                .resolve_import(module_id, relative)
                .map_err(|error| {
                    ModuleError::new(self.sources.render(&Diagnostic::error(
                        error.to_string(),
                        binding.value.value.location,
                    )))
                })?;
            if skeleton.is_some() {
                let imported_module_id = self.main.modules.id(&imported.id).ok_or_else(|| {
                    ModuleError::new(format!(
                        "imported module {} was not present during module graph discovery",
                        imported.id
                    ))
                })?;
                graph_imports.push(ImportEdge {
                    local: (binding.value.kind == BindingKind::Import)
                        .then(|| binding.value.name.value.clone()),
                    target: imported_module_id,
                });
            }
            if imported.authority == ModuleAuthority::RuntimeSystem {
                let module = self.load_native_module(relative).map_err(|error| {
                    ModuleError::new(self.sources.render(&Diagnostic::error(
                        error.to_string(),
                        binding.value.value.location,
                    )))
                })?;
                semantic_imports.push(SemanticImport {
                    name: if binding.value.kind == BindingKind::OpenImport {
                        "*".into()
                    } else {
                        binding.value.name.value.clone()
                    },
                    location: binding.value.name.location,
                    target: imported.id.clone(),
                    namespace: binding.value.kind != BindingKind::OpenImport
                        && binding.value.imported_name.is_none(),
                });
                if binding.value.kind == BindingKind::OpenImport {
                    for (name, candidate) in open_import_exports(
                        &imported.id,
                        module.root,
                        &module.interface,
                        &self.main.heap,
                        module.provenance.as_ref(),
                    )? {
                        open_candidates.entry(name).or_default().push(candidate);
                    }
                    continue;
                }
                let (selected_root, interface) = select_import_root(
                    module.root,
                    module.interface,
                    binding.value.imported_name.as_deref(),
                    &binding.value.name.value,
                    &self.main.heap,
                )?;
                external_roots.insert(binding.value.name.value.clone(), selected_root);
                external_interfaces.insert(binding.value.name.value.clone(), interface);
                continue;
            }
            let imported_id = imported.id.clone();
            let artifact = self.load_resolved_value(imported)?;
            semantic_imports.push(SemanticImport {
                name: if binding.value.kind == BindingKind::OpenImport {
                    "*".into()
                } else {
                    binding.value.name.value.clone()
                },
                location: binding.value.name.location,
                target: imported_id.clone(),
                namespace: binding.value.kind != BindingKind::OpenImport
                    && binding.value.imported_name.is_none(),
            });
            if binding.value.kind == BindingKind::OpenImport {
                for (name, candidate) in open_import_exports(
                    &imported_id,
                    artifact.root,
                    &artifact.interface,
                    &self.main.heap,
                    artifact.provenance.as_ref(),
                )? {
                    open_candidates.entry(name).or_default().push(candidate);
                }
                continue;
            }
            let (selected_root, selected_interface) = select_import_root(
                artifact.root,
                artifact.interface,
                binding.value.imported_name.as_deref(),
                &binding.value.name.value,
                &self.main.heap,
            )?;
            external_roots.insert(binding.value.name.value.clone(), selected_root);
            external_interfaces.insert(binding.value.name.value.clone(), selected_interface);
            if let Some(provenance) = artifact.provenance
                && !provenance.values.is_empty()
            {
                external_provenance.insert(binding.value.name.value.clone(), provenance);
            }
        }
        if module_id.to_string() != PRELUDE_MODULE
            && let Some(module) = self.core_modules.get(PRELUDE_MODULE)
        {
            let provider = ModuleCName::Builtin(PRELUDE_MODULE.into());
            if skeleton.is_some() {
                let target = self.main.modules.id(&provider).ok_or_else(|| {
                    ModuleError::new("prelude was not present during module graph discovery")
                })?;
                graph_imports.push(ImportEdge {
                    local: None,
                    target,
                });
            }
            for (name, candidate) in open_import_exports(
                &provider,
                module.root,
                &module.interface,
                &self.main.heap,
                module.provenance.as_ref(),
            )? {
                open_candidates.entry(name).or_default().push(candidate);
            }
        }
        if let Some(skeleton) = &skeleton
            && skeleton.imports != graph_imports
        {
            return Err(ModuleError::new(format!(
                "module {module_id} import graph changed after static discovery"
            )));
        }
        let explicit_names = program
            .value
            .body
            .value
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
                if program_references_name(&program, &name) {
                    let providers = candidates
                        .iter()
                        .map(|candidate| candidate.provider.to_string())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(ModuleError::new(format!(
                        "open import name {name:?} is ambiguous between {providers}"
                    )));
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
                    traits: BTreeMap::new(),
                    type_family_templates: candidate
                        .type_family_template
                        .map(|family| BTreeMap::from([(name.clone(), family)]))
                        .unwrap_or_default(),
                },
            );
            if let Some(provenance) = candidate.provenance
                && !provenance.values.is_empty()
            {
                external_provenance.insert(name.clone(), provenance);
            }
        }
        if let Some(binding) = program.value.body.value.bindings.iter().find(|binding| {
            matches!(
                binding.value.kind,
                BindingKind::Native | BindingKind::NativeType
            )
        }) {
            let message = if authority == ModuleAuthority::Ordinary {
                format!(
                    "native symbol {:?} is only allowed in built-in or *.native.telora modules",
                    binding.value.name.value
                )
            } else {
                format!(
                    "native symbol {:?} is not registered for this system module",
                    binding.value.name.value
                )
            };
            return Err(ModuleError::new(self.sources.render(
                &crate::source::Diagnostic::error(message, binding.location),
            )));
        }

        let mut dynamic_bindings = HashSet::new();
        if is_root && external_roots.contains_key("input") {
            dynamic_bindings.insert("input".to_owned());
        }
        let analysis = analyze_program_with_bindings_observed(
            &source_name,
            skeleton
                .as_ref()
                .map_or(ModuleId::ANONYMOUS, |skeleton| skeleton.id),
            &program,
            account,
            &external_roots
                .iter()
                .map(|(name, root)| (name.clone(), *root))
                .collect(),
            &dynamic_bindings,
            &self.sources,
            &external_provenance,
            &external_interfaces,
            &self.debug_sink,
            &mut self.main.heap,
            &mut self.main.types,
        )
        .map_err(|error| {
            error.diagnostic.as_ref().map_or_else(
                || ModuleError::new(error.to_string()),
                |diagnostic| ModuleError::new(self.sources.render(diagnostic)),
            )
        })?;
        install_type_family_roots(&mut external_roots, &analysis);
        let source_file = self.sources.get(source_id);
        let mut promoted_types = HashSet::new();
        let mut erased_metadata_bindings = HashSet::new();
        if let Some(metadata) = metadata_compilation_plan(&program) {
            erased_metadata_bindings = metadata.erased_bindings;
            promoted_types.extend(metadata.type_names);
        }
        let static_funcs = skeleton.as_ref().map_or_else(HashMap::new, |skeleton| {
            self.main.modules.static_funcs(skeleton.id)
        });
        let function = if promoted_types.is_empty() {
            compile_program_analyzed_in_module(source_file, &program, &analysis, &static_funcs)
        } else {
            compile_program_with_promoted_types_and_static_funcs(
                source_file,
                &program,
                &analysis,
                &promoted_types,
                &erased_metadata_bindings,
                &static_funcs,
            )
        }
        .map_err(|error| ModuleError::new(error.to_string()))?;
        let key = module_id.to_string();
        self.semantic_inputs.insert(
            key.clone(),
            SemanticModuleInput {
                key,
                path: (!synthetic).then(|| path.to_owned()),
                kind: WorkspaceModuleKind::Telora,
                source: Some(source_id),
                program: Some(program),
                analysis: Some(analysis.clone()),
                partial: None,
                interface: None,
                state: crate::semantic::WorkspaceModuleState::Available,
                imports: semantic_imports,
                diagnostics: Vec::new(),
            },
        );
        Ok(CompiledTeloraModule {
            analysis,
            function,
            externals: runtime_roots(&external_roots),
            options,
        })
    }

    fn load_native_module(&mut self, name: &str) -> Result<ModuleArtifact, ModuleError> {
        self.core_modules
            .get(name)
            .cloned()
            .ok_or_else(|| ModuleError::new(format!("unknown built-in module {name:?}")))
    }

    fn enter(&mut self, module_id: &ModuleCName) -> Result<(), ModuleError> {
        if let Some(index) = self
            .visiting
            .iter()
            .position(|candidate| candidate == module_id)
        {
            let mut cycle = self.visiting[index..]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            cycle.push(module_id.to_string());
            return Err(ModuleError::new(format!(
                "module import cycle: {}",
                cycle.join(" -> ")
            )));
        }
        self.visiting.push(module_id.clone());
        Ok(())
    }

    fn leave(&mut self, module_id: &ModuleCName) {
        let popped = self.visiting.pop();
        debug_assert_eq!(popped.as_ref(), Some(module_id));
    }
}

fn module_binding_diagnostics(program: &Program) -> Vec<Diagnostic> {
    let mut visible = HashMap::<String, (BindingKind, crate::Location)>::new();
    let mut diagnostics = Vec::new();
    for binding in &program.value.body.value.bindings {
        let kind = binding.value.kind;
        if matches!(
            kind,
            BindingKind::Let | BindingKind::OpenImport | BindingKind::Export
        ) {
            continue;
        }
        let name = &binding.value.name.value;
        let Some((previous_kind, previous_location)) = visible.get(name).copied() else {
            visible.insert(name.clone(), (kind, binding.value.name.location));
            continue;
        };
        if previous_kind == BindingKind::Decl && kind == BindingKind::Def {
            visible.insert(name.clone(), (kind, binding.value.name.location));
            continue;
        }
        diagnostics.push(
            Diagnostic::error(
                format!("module binding {name:?} conflicts with an earlier explicit binding"),
                binding.value.name.location,
            )
            .with_secondary("first bound here", previous_location),
        );
    }
    diagnostics
}

fn reject_nested_imports(program: &Program, source_name: &str) -> Result<(), ModuleError> {
    for binding in &program.value.body.value.bindings {
        if matches!(binding.value.kind, BindingKind::Let | BindingKind::Def)
            && expression_has_import(&binding.value.value)
        {
            return Err(ModuleError::new(format!(
                "{source_name}: imports and native declarations are only allowed at module top level"
            )));
        }
    }
    if expression_has_import(&program.value.body.value.result) {
        return Err(ModuleError::new(format!(
            "{source_name}: imports and native declarations are only allowed at module top level"
        )));
    }
    Ok(())
}

fn expression_has_import(expression: &Expr) -> bool {
    match &expression.value {
        ExprKind::Block(block) => {
            block.value.bindings.iter().any(|binding| {
                matches!(
                    binding.value.kind,
                    BindingKind::Import | BindingKind::OpenImport | BindingKind::Native
                )
            }) || block
                .value
                .bindings
                .iter()
                .any(|binding| expression_has_import(&binding.value.value))
                || expression_has_import(&block.value.result)
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => items.iter().any(expression_has_import),
        ExprKind::Spread(operand) => expression_has_import(operand),
        ExprKind::InterpolatedString(parts) => parts.iter().any(|part| match &part.value {
            StringPartKind::Text(_) => false,
            StringPartKind::Expression(expression) => expression_has_import(expression),
        }),
        ExprKind::Dict(fields) => fields
            .iter()
            .any(|field| expression_has_import(&field.value.value)),
        ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } => {
            expression_has_import(operand)
        }
        ExprKind::Return { value } => expression_has_import(value),
        ExprKind::Panic { message } => expression_has_import(message),
        ExprKind::Raise { error } => expression_has_import(error),
        ExprKind::Debug { value, .. } => expression_has_import(value),
        ExprKind::Binary { left, right, .. } => {
            expression_has_import(left) || expression_has_import(right)
        }
        ExprKind::Field { receiver, .. } => expression_has_import(receiver),
        ExprKind::Index { receiver, index } => {
            expression_has_import(receiver) || expression_has_import(index)
        }
        ExprKind::TupleProjection { receiver, .. } => expression_has_import(receiver),
        ExprKind::TypeAscription { value, target } | ExprKind::CheckedCast { value, target } => {
            expression_has_import(value) || expression_has_import(target)
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            expression_has_import(namespace)
                || expression_has_import(target)
                || expression_has_import(value)
        }
        ExprKind::Call { callee, arguments } => {
            expression_has_import(callee) || arguments.iter().any(expression_has_import)
        }
        ExprKind::TypeApply { callee, arguments } => {
            expression_has_import(callee)
                || arguments.iter().any(|argument| match &argument.value {
                    TypeArgumentKind::Explicit(argument) => expression_has_import(argument),
                    TypeArgumentKind::Infer => false,
                })
        }
        ExprKind::Interpreter { operand, .. } => expression_has_import(operand),
        ExprKind::Closure { body, .. } => {
            body.value.bindings.iter().any(|binding| {
                matches!(
                    binding.value.kind,
                    BindingKind::Import | BindingKind::OpenImport
                )
            }) || body
                .value
                .bindings
                .iter()
                .any(|binding| expression_has_import(&binding.value.value))
                || expression_has_import(&body.value.result)
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expression_has_import(condition)
                || then_branch
                    .value
                    .bindings
                    .iter()
                    .chain(&else_branch.value.bindings)
                    .any(|binding| {
                        matches!(
                            binding.value.kind,
                            BindingKind::Import | BindingKind::OpenImport
                        ) || expression_has_import(&binding.value.value)
                    })
                || expression_has_import(&then_branch.value.result)
                || expression_has_import(&else_branch.value.result)
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            expression_has_import(value)
                || expression_has_import(&then_branch.value.result)
                || expression_has_import(&else_branch.value.result)
                || then_branch
                    .value
                    .bindings
                    .iter()
                    .chain(&else_branch.value.bindings)
                    .any(|binding| expression_has_import(&binding.value.value))
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            expression_has_import(value)
                || expression_has_import(&else_branch.value.result)
                || expression_has_import(&body.value.result)
                || else_branch
                    .value
                    .bindings
                    .iter()
                    .chain(&body.value.bindings)
                    .any(|binding| expression_has_import(&binding.value.value))
        }
        ExprKind::Match { value, arms } => {
            expression_has_import(value)
                || arms.iter().any(|arm| {
                    arm.value.guard.as_ref().is_some_and(expression_has_import)
                        || expression_has_import(&arm.value.value)
                })
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_)
        | ExprKind::Variable(_) => false,
    }
}

fn read(path: &Path) -> Result<String, ModuleError> {
    fs::read_to_string(path).map_err(|error| {
        ModuleError::new(format!("cannot read module {}: {error}", path.display()))
    })
}

fn read_data_file(path: &Path, max_bytes: usize) -> Result<String, ModuleError> {
    use std::io::Read;

    let file = fs::File::open(path).map_err(|error| {
        ModuleError::new(format!(
            "cannot read data module {}: {error}",
            path.display()
        ))
    })?;
    let max_read = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    file.take(max_read)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ModuleError::new(format!(
                "cannot read data module {}: {error}",
                path.display()
            ))
        })?;
    if bytes.len() > max_bytes {
        return Err(ModuleError::new(format!(
            "data source exceeds file_size limit ({} > {max_bytes})",
            bytes.len()
        )));
    }
    String::from_utf8(bytes).map_err(|error| {
        ModuleError::new(format!(
            "cannot read data module {}: {error}",
            path.display()
        ))
    })
}

#[cfg(test)]
fn canonicalize(path: &Path) -> Result<PathBuf, ModuleError> {
    fs::canonicalize(path).map_err(|error| {
        ModuleError::new(format!("cannot resolve module {}: {error}", path.display()))
    })
}
