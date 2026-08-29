pub struct Engine {
    config: EngineConfig,
    debug_sink: Arc<dyn DebugSink>,
}

pub struct EngineBuilder {
    config: EngineConfig,
}

impl EngineBuilder {
    fn new(config: EngineConfig) -> Self {
        Self { config }
    }

    pub fn build(self) -> Engine {
        Engine {
            config: self.config,
            debug_sink: Arc::new(DiscardDebugSink),
        }
    }
}

impl fmt::Debug for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Engine")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Engine {
    pub fn new(config: EngineConfig) -> Self {
        Self::builder(config).build()
    }

    pub fn builder(config: EngineConfig) -> EngineBuilder {
        EngineBuilder::new(config)
    }

    pub fn with_debug_sink(mut self, debug_sink: Arc<dyn DebugSink>) -> Self {
        self.debug_sink = debug_sink;
        self
    }

    pub const fn config(&self) -> EngineConfig {
        self.config
    }

    pub fn load_module(
        &self,
        path: impl AsRef<Path>,
        external_bindings: BTreeMap<String, crate::DataWorld>,
    ) -> Result<LoadedModule, ModuleError> {
        load_module_with_policy(
            path,
            external_bindings,
            self.config.module_quota,
            self.config.data_limits,
            Arc::clone(&self.debug_sink),
            ModuleSourcePolicy::ExplicitExports,
        )
    }

    pub fn load_module_id(
        &self,
        cwd: impl AsRef<Path>,
        module_id: &str,
        external_bindings: BTreeMap<String, crate::DataWorld>,
    ) -> Result<LoadedModule, ModuleError> {
        let resolver = ModuleResolver::from_cwd(cwd.as_ref(), module_id)
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list());
        load_module_with_resolver(
            resolver,
            external_bindings,
            self.config.module_quota,
            self.config.data_limits,
            Arc::clone(&self.debug_sink),
            ModuleSourcePolicy::ExplicitExports,
        )
    }

    pub fn load_module_id_in_workspace(
        &self,
        workspace: Arc<crate::package::ResolvedWorkspace>,
        cwd: impl AsRef<Path>,
        module_id: &str,
        external_bindings: BTreeMap<String, crate::DataWorld>,
    ) -> Result<LoadedModule, ModuleError> {
        let resolver = ModuleResolver::from_workspace(workspace, cwd.as_ref(), module_id)
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list());
        load_module_with_resolver(
            resolver,
            external_bindings,
            self.config.module_quota,
            self.config.data_limits,
            Arc::clone(&self.debug_sink),
            ModuleSourcePolicy::ExplicitExports,
        )
    }

    pub fn load_standalone(
        &self,
        path: impl AsRef<Path>,
        external_bindings: BTreeMap<String, crate::DataWorld>,
    ) -> Result<LoadedModule, ModuleError> {
        let resolver = ModuleResolver::standalone(path.as_ref())
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list());
        load_module_with_resolver(
            resolver,
            external_bindings,
            self.config.module_quota,
            self.config.data_limits,
            Arc::clone(&self.debug_sink),
            ModuleSourcePolicy::ExplicitExports,
        )
    }

    pub fn prepare_module(&self, path: impl AsRef<Path>) -> Result<PendingModule, ModuleError> {
        let resolver = ModuleResolver::for_root(path.as_ref())
            .map_err(|error| ModuleError::new(error.to_string()))?;
        self.prepare_resolved_module(resolver)
    }

    pub fn prepare_standalone(&self, path: impl AsRef<Path>) -> Result<PendingModule, ModuleError> {
        let resolver = ModuleResolver::standalone(path.as_ref())
            .map_err(|error| ModuleError::new(error.to_string()))?;
        self.prepare_resolved_module(resolver)
    }

    pub fn prepare_module_id(
        &self,
        cwd: impl AsRef<Path>,
        module_id: &str,
    ) -> Result<PendingModule, ModuleError> {
        let resolver = ModuleResolver::from_cwd(cwd.as_ref(), module_id)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        self.prepare_resolved_module(resolver)
    }

    pub fn prepare_module_id_in_workspace(
        &self,
        workspace: Arc<crate::package::ResolvedWorkspace>,
        cwd: impl AsRef<Path>,
        module_id: &str,
    ) -> Result<PendingModule, ModuleError> {
        let resolver = ModuleResolver::from_workspace(workspace, cwd.as_ref(), module_id)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        self.prepare_resolved_module(resolver)
    }

    fn prepare_resolved_module(
        &self,
        resolver: ModuleResolver,
    ) -> Result<PendingModule, ModuleError> {
        let root = resolver
            .selected_root()
            .map_err(|error| ModuleError::new(error.to_string()))?;
        if root.format != ModuleFormat::Telora {
            return Err(ModuleError::new(
                "main module must have a .telora extension",
            ));
        }
        let physical = root
            .path()
            .ok_or_else(|| ModuleError::new("main module has no physical path"))?;
        let mut sources = SourceDatabase::default();
        let source_id = sources.add(root.id.to_string(), read(physical, &root.id.to_string())?);
        let parsed = parse_registered(&sources, source_id);
        if parsed.program.is_none() {
            return Err(ModuleError::new(
                parsed
                    .diagnostics
                    .iter()
                    .map(|diagnostic| sources.render(diagnostic))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
        }
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
                            sources.render(&Diagnostic::error(error.to_string(), option.location)),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PendingModule {
            inner: Arc::new(PendingModuleInner {
                path: physical.to_owned(),
                resolver,
                options,
                config: self.config,
                debug_sink: Arc::clone(&self.debug_sink),
                state: Mutex::new(PendingModuleState::Pending {
                    bindings: BTreeMap::new(),
                }),
            }),
        })
    }

    pub fn execute(
        &self,
        module: &LoadedModule,
    ) -> Result<crate::ExecutionWorld, crate::RuntimeError> {
        module.execute_with_quota_and_debug_sink(
            self.config.session_quota,
            Arc::clone(&self.debug_sink),
        )
    }

    pub fn check(&self, module: &LoadedModule) -> Result<(), crate::RuntimeError> {
        module.check_with_quota_and_debug_sink(
            self.config.session_quota,
            Arc::clone(&self.debug_sink),
        )
    }

    pub fn invoke_world(
        &self,
        module: &LoadedModule,
        callee: crate::ExecutionWorld,
        arguments: &[crate::DataWorld],
    ) -> Result<crate::ExecutionWorld, ModuleError> {
        let (main, world) = callee.into_parts();
        if !Arc::ptr_eq(&main, &module.runtime.main.heap) {
            return Err(ModuleError::new("callable belongs to another Main world"));
        }
        let argument_count = arguments.len();
        let mut instructions = vec![Instruction::Call {
            base: Register(0),
            argument_count,
        }];
        instructions.push(Instruction::Return { src: Register(0) });
        let wrapper = BytecodeFunction::with_signature(
            "<invoke world callable>",
            argument_count + 1,
            0,
            argument_count + 1,
            Vec::new(),
            instructions,
        );
        let mut world = world;
        let runtime_arguments = arguments
            .iter()
            .map(|argument| argument.relocate_into(world.heap_mut(), &main))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let mut account = QuotaAccount::new(self.config.session_quota);
        let world = Vm::new()
            .with_debug_sink(Arc::clone(&self.debug_sink))
            .execute_in_existing_world_with_runtime_args(
                &main,
                &module.runtime.externals,
                &wrapper,
                world,
                &runtime_arguments,
                &[],
                &mut account,
            )
            .map_err(|error| ModuleError::new(error.with_sources(&module.sources).to_string()))?;
        Ok(crate::ExecutionWorld::new(main, world))
    }

    pub async fn run_pending(
        &self,
        pending: PendingModule,
        entry_selector: &str,
        entry_args: &[String],
    ) -> Result<RunOutcome, ModuleError> {
        self.run_pending_with_host(pending, entry_selector, entry_args, &mut NoProcessRunHost)
            .await
    }

    pub async fn run_pending_with_host(
        &self,
        pending: PendingModule,
        entry_selector: &str,
        entry_args: &[String],
        host: &mut dyn RunHost,
    ) -> Result<RunOutcome, ModuleError> {
        self.run_pending_with_sources_and_host(
            pending,
            entry_selector,
            entry_args,
            &EntryDataSources::new(),
            host,
        )
        .await
    }

    pub async fn run_pending_with_sources_and_host(
        &self,
        pending: PendingModule,
        entry_selector: &str,
        entry_args: &[String],
        entry_sources: &EntryDataSources,
        host: &mut dyn RunHost,
    ) -> Result<RunOutcome, ModuleError> {
        let result = self
            .run_pending_with_host_inner(
                pending,
                entry_selector,
                entry_args,
                entry_sources,
                host,
            )
            .await;
        let finished = host
            .finish()
            .await
            .map_err(|error| ModuleError::new(format!("cannot finish run Host: {error}")));
        match (result, finished) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Err(error), Ok(())) => Err(error),
            (_, Err(error)) => Err(error),
        }
    }

    async fn run_pending_with_host_inner(
        &self,
        pending: PendingModule,
        entry_selector: &str,
        entry_args: &[String],
        entry_sources: &EntryDataSources,
        host: &mut dyn RunHost,
    ) -> Result<RunOutcome, ModuleError> {
        let resolver = pending.inner.resolver.clone();
        let (entry_id, entry_source) = if entry_selector == DEFAULT_ENTRY_MODULE {
            (
                ModuleCName::builtin("std/entry/default"),
                default_entry_source().to_owned(),
            )
        } else if entry_selector == SERVE_ENTRY_MODULE {
            (
                ModuleCName::builtin("std/entry/serve"),
                serve_entry_source().to_owned(),
            )
        } else {
            let entry = resolver
                .resolve_entry(entry_selector)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            let path = entry.path().map(Path::to_owned).ok_or_else(|| {
                ModuleError::new(format!("Entry {entry_selector:?} has no physical source"))
            })?;
            let source_name = entry.id.to_string();
            (entry.id, read(&path, &source_name)?)
        };
        let SelectedEntryLoader {
            mut loader,
            main_module,
            main_path,
            entry: entry_compiled,
        } = prepare_selected_entry(
            resolver,
            entry_id,
            &entry_source,
            self.config.module_quota,
            self.config.data_limits,
            Arc::clone(&self.debug_sink),
        )?;
        let mut account = QuotaAccount::new(self.config.session_quota);
        let entry_world = Vm::new()
            .with_debug_sink(Arc::clone(&self.debug_sink))
            .execute_in_work(
                &loader.main.heap,
                &entry_compiled.externals,
                &entry_compiled.function,
                &[],
                &mut account,
            )
            .map_err(|error| ModuleError::new(error.with_sources(&loader.sources).to_string()))?
            .seal_module()
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let expected = ["MainType", "State", "config"];
        if entry_world
            .module_fields(&loader.main.heap)
            .map_err(|error| ModuleError::new(error.to_string()))?
            != expected
        {
            return Err(ModuleError::new(
                "Entry must export exactly MainType, State, and config",
            ));
        }
        let exports = ["MainType", "State"]
            .into_iter()
            .map(|name| {
                entry_world
                    .module_member_ref(&loader.main.heap, name)
                    .map_err(|error| ModuleError::new(error.to_string()))?
                    .ok_or_else(|| ModuleError::new(format!("Entry has no export {name:?}")))
                    .map(|value| (name, value))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let main_type = crate::types::decode_type_ref(exports["MainType"], "Entry.MainType")
            .map_err(ModuleError::new)?;
        let state_type = crate::types::decode_type_ref(exports["State"], "Entry.State")
            .map_err(ModuleError::new)?;
        let (value_owner, value_type) =
            semantic_value_contract(&loader.builtin_modules, &loader.main.heap)?;
        validate_entry_interface(
            &entry_compiled.analysis.module_interface,
            &main_type,
            &state_type,
            &value_type,
        )?;
        if entry_world
            .member_function_arity(&loader.main.heap, "config")
            .map_err(|error| ModuleError::new(error.to_string()))?
            != Some(2)
        {
            return Err(ModuleError::new("Entry.config must accept 2 arguments"));
        }
        let mut entry_world = entry_world;
        let options = make_system_options(
            entry_world.heap_mut(),
            &loader.main.heap,
            &pending.inner.options,
        )?;
        let env = make_entry_env(
            entry_world.heap_mut(),
            &loader.main.heap,
            entry_args,
            entry_sources,
        );
        let configured = invoke_world_member_in(
            &loader.main.heap,
            &entry_compiled.externals,
            &loader.sources,
            entry_world,
            "config",
            &[options, env],
            false,
            self.config.session_quota,
            Arc::clone(&self.debug_sink),
        )
        .map_err(|error| ModuleError::new(format!("Entry.config failed: {error}")))?;
        let (entry_world, initializer) = configured
            .into_runtime_pair(
                &loader.main.heap,
                "Entry.config must return Tuple([SystemCaps, Initializer])",
                "Entry.config must return exactly SystemCaps and Initializer",
            )
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let caps = parse_system_caps(entry_world.root_ref(&loader.main.heap))?;
        let initializer_arity = entry_world
            .runtime_function_arity(&loader.main.heap, initializer)
            .map_err(|error| ModuleError::new(error.to_string()))?
            .ok_or_else(|| ModuleError::new("Entry.config initializer must be a function"))?;
        if initializer_arity != 2 {
            return Err(ModuleError::new(format!(
                "Entry.config initializer must accept 2 arguments, found {initializer_arity}"
            )));
        }
        host.configure(caps.clone()).await.map_err(|error| {
            ModuleError::new(format!("cannot satisfy Entry capabilities: {error}"))
        })?;
        let bindings = pending.begin_initialization()?;

        let main_source_name = main_module.id.to_string();
        let (compiled_main_path, main_compiled) = match loader.compile_root(main_module, bindings) {
            Ok(compiled) => compiled,
            Err(error) => {
                pending.finish_initialization(&Err(error.clone()));
                return Err(error);
            }
        };
        debug_assert_eq!(compiled_main_path, main_path);
        let actual_main_type =
            concrete_module_descriptor(&main_compiled.analysis.module_interface)?;
        if !matches!(main_type, TypeDescriptor::Dyn)
            && !crate::types::assignable(
                &crate::types::erase_declared_identity(&actual_main_type),
                &crate::types::erase_declared_identity(&main_type),
            ) {
                return Err(ModuleError::new(format!(
                    "Main export record {} is not assignable to Entry.MainType {}",
                    actual_main_type.display_name(),
                    main_type.display_name()
                )));
            }

        let workspace = WorkspaceSnapshot::build(
            loader.sources.clone(),
            loader.semantic_inputs.values().cloned().collect(),
        );
        let dependencies = loader.dependencies.iter().cloned().collect::<Vec<_>>();
        let sources = loader.sources.clone();
        let shared_main =
            Arc::new(std::mem::replace(&mut loader.main, MainWorld::building()).seal());
        let mut entry = loaded_from_compiled(
            main_path.clone(),
            dependencies.clone(),
            sources.clone(),
            workspace.clone(),
            Arc::clone(&shared_main),
            entry_compiled,
        );
        let main = loaded_from_compiled(
            compiled_main_path,
            dependencies,
            sources,
            workspace,
            Arc::clone(&shared_main),
            main_compiled,
        );
        let (main_world, _) =
            main.execute_world_observed(self.config.session_quota, Arc::clone(&self.debug_sink));
        let mut main_world = match main_world {
            Ok(world) => world,
            Err(error) => {
                let error = ModuleError::new(error.to_string());
                pending.finish_initialization(&Err(error.clone()));
                return Err(error);
            }
        };
        if matches!(main_type, TypeDescriptor::Dyn) {
            main_world = main_world
                .wrap_root_dyn(
                    &shared_main.heap,
                    &actual_main_type,
                    main_source_name,
                )
                .map_err(|error| ModuleError::new(error.to_string()))?;
        }
        let instantiated = InstantiatedModule {
            module: Arc::new(main),
            execution: Arc::new(main_world),
        };
        pending.finish_initialization(&Ok(instantiated.clone()));
        let (mut entry_world, main_argument) = entry_world
            .import_world_root(&shared_main.heap, &instantiated.execution)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let mut prepared_plans = Vec::new();
        for (key, request) in &caps.data_sources {
            let Some(source) = host
                .read_data_source(request, self.config.data_limits.file_size)
                .await
                .map_err(|error| {
                    ModuleError::new(format!(
                        "cannot read Entry data source {:?}: {error}",
                        request.src
                    ))
                })?
            else {
                continue;
            };
            let source_id = entry.sources.add(request.src.clone(), &source);
            let plan = validate_system_data_source(request.format, &entry.sources, source_id)
                .map_err(|diagnostics| {
                    ModuleError::new(
                        diagnostics
                            .iter()
                            .map(|diagnostic| entry.sources.render(diagnostic))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                })?;
            plan.enforce_limits(self.config.data_limits, source.len())
                .map_err(|error| {
                    ModuleError::new(format!("Entry data source {:?}: {error}", request.src))
                })?;
            prepared_plans.push((key.clone(), request.src.clone(), plan));
        }
        let type_id = semantic_value_type_id(
            entry_world.heap(),
            Some(&shared_main.heap),
            value_owner.runtime(),
        )
        .map_err(|error| ModuleError::new(error.to_string()))?;
        let mut prepared_fields = Vec::with_capacity(prepared_plans.len());
        for (key, source_name, plan) in prepared_plans {
            let materialized = materialize_data_plan(
                &plan,
                entry_world.heap_mut(),
                Some(SemanticDataTarget {
                    background: Some(&shared_main.heap),
                    type_id,
                }),
            );
            let data = materialized.value;
            let src = Val::unknown(
                entry_world
                    .heap_mut()
                    .string(Some(&shared_main.heap), &source_name),
            );
            let item = allocate_record(
                entry_world.heap_mut(),
                [("data".into(), data), ("src".into(), src)],
            );
            prepared_fields.push((key, item));
        }
        let prepared_data = allocate_record(entry_world.heap_mut(), prepared_fields);
        let caps_argument = entry_world.root_ref(&entry.runtime.main.heap).runtime();
        let resources_provider = entry_world
            .heap_mut()
            .native_closure(host.resources_provider(), []);
        let initialized = prepare_and_initialize_entry_in(
            &entry.runtime.main.heap,
            &entry.runtime.externals,
            &entry.sources,
            entry_world,
            resources_provider,
            caps_argument,
            value_owner.runtime(),
            prepared_data,
            initializer,
            main_argument,
            self.config.session_quota,
            Arc::clone(&self.debug_sink),
        )
        .map_err(|error| ModuleError::new(format!("Entry initialization failed: {error}")))?;
        let (state, reducer) = initialized
            .into_runtime_pair(
                &entry.runtime.main.heap,
                "Entry initializer must return Tuple([State, Reducer])",
                "Entry initializer must return exactly State and Reducer",
            )
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let mut state = state;
        let reducer_arity = state
            .runtime_function_arity(&entry.runtime.main.heap, reducer)
            .map_err(|error| ModuleError::new(error.to_string()))?
            .ok_or_else(|| ModuleError::new("Entry reducer must be a function"))?;
        if reducer_arity != 2 {
            return Err(ModuleError::new(format!(
                "Entry reducer must accept 2 arguments, found {reducer_arity}"
            )));
        }
        let mut events = std::collections::VecDeque::from([None]);
        let mut output = String::new();
        loop {
            let event = match events.pop_front() {
                Some(event) => event,
                None => host
                    .next_event()
                    .await
                    .map_err(|error| ModuleError::new(format!("run Host failed: {error}")))?
                    .map(Some)
                    .ok_or_else(|| {
                        ModuleError::new("Entry made no progress and the Host has no pending event")
                    })?,
            };
            let event = runtime_system_event(state.heap_mut(), &entry.runtime.main.heap, event)?;
            let transition = entry
                .invoke_reducer_in_work(
                    reducer,
                    state,
                    event,
                    self.config.session_quota,
                    Arc::clone(&self.debug_sink),
                )
                .map_err(|error| ModuleError::new(format!("Entry reducer failed: {error}")))?;
            let (next_state, effects) = transition
                .into_reducer_transition(&entry.runtime.main.heap)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            state = next_state;
            for effect in &effects {
                let effect = state.value_ref(&entry.runtime.main.heap, *effect);
                let (tag, _) = effect
                    .tagged_parts()
                    .ok_or_else(|| ModuleError::new("Entry returned an invalid SystemEffect"))?;
                if matches!(
                    tag.as_atom().as_deref(),
                    Some("SpawnStdioChild" | "PostStdin")
                ) && !caps.spawn_child
                {
                    return Err(ModuleError::new(
                        "Entry emitted a child-process effect without spawn_child capability",
                    ));
                }
            }
            let mut terminal = None;
            for effect in effects {
                let effect = state.value_ref(&entry.runtime.main.heap, effect);
                if terminal.is_some() {
                    return Err(ModuleError::new(
                        "Entry returned an effect after a terminal effect",
                    ));
                }
                let (tag, payload) = effect
                    .tagged_parts()
                    .ok_or_else(|| ModuleError::new("Entry returned an invalid SystemEffect"))?;
                match tag.as_atom().as_deref() {
                    Some("SpawnStdioChild") => {
                        let child = parse_spawn_stdio_child_ref(payload)?;
                        host.spawn_stdio_child(child).await.map_err(|error| {
                            ModuleError::new(format!("cannot spawn stdio child: {error}"))
                        })?;
                    }
                    Some("PostStdin") => {
                        let text = parse_child_text_ref(payload, "PostStdin")?;
                        host.post_stdin(text).await.map_err(|error| {
                            ModuleError::new(format!("cannot post child stdin: {error}"))
                        })?;
                    }
                    Some("Exec") => {
                        terminal = Some(RunTermination::Exec(parse_child_options_ref(
                            payload, "Exec",
                        )?));
                    }
                    Some("Output") => {
                        let text = payload
                            .as_str()
                            .ok_or_else(|| ModuleError::new("Output payload must be String"))?;
                        output.push_str(text.as_str());
                    }
                    Some("Exit") => {
                        let code = payload
                            .as_int()
                            .ok_or_else(|| ModuleError::new("Exit payload must be Int"))?;
                        terminal = Some(RunTermination::Exit(code));
                    }
                    _ => return Err(ModuleError::new("Entry returned an invalid SystemEffect")),
                }
            }
            if let Some(termination) = terminal {
                return Ok(RunOutcome {
                    output,
                    termination,
                });
            }
        }
    }

    pub fn recover_workspace(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        let resolver = ModuleResolver::for_root(path.as_ref())
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list());
        let root_module = resolver
            .resolve_root(path.as_ref())
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let opaque_modules = builtin_list()
            .into_iter()
            .map(|(name, _)| ModuleCName::builtin(name));
        let graph = ModuleGraph::discover(
            &resolver,
            vec![root_module.clone()],
            &BTreeMap::new(),
            opaque_modules,
            None,
            true,
        )?;
        let mut main = MainWorld::with_modules(graph);
        let mut sources = SourceDatabase::default();
        let builtin_modules = install_native_modules(&mut main, &mut sources, &self.debug_sink)?;
        let mut builder = WorkspaceBuilder {
            engine: self,
            resolver,
            overlays: &BTreeMap::new(),
            query: None,
            sources,
            main,
            builtin_modules,
            inputs: BTreeMap::new(),
            provenances: HashMap::new(),
            roots: HashMap::new(),
            interfaces: HashMap::new(),
            visiting: Vec::new(),
            cycle_members: HashSet::new(),
            cycle_reported: false,
        };
        block_on_recovery(builder.load_telora(root_module));
        Ok(WorkspaceSnapshot::build(
            builder.sources,
            builder.inputs.into_values().collect(),
        ))
    }

    pub fn recover_workspace_id(
        &self,
        cwd: impl AsRef<Path>,
        module_id: &str,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        let resolver = ModuleResolver::from_cwd(cwd.as_ref(), module_id)
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list());
        self.recover_with_resolver(resolver)
    }

    pub fn recover_workspace_id_in_workspace(
        &self,
        workspace: Arc<crate::package::ResolvedWorkspace>,
        cwd: impl AsRef<Path>,
        module_id: &str,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        let resolver = ModuleResolver::from_workspace(workspace, cwd.as_ref(), module_id)
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list());
        self.recover_with_resolver(resolver)
    }

    pub fn module_catalog(
        &self,
        cwd: impl AsRef<Path>,
    ) -> Result<Vec<ModuleCatalogEntry>, ModuleError> {
        ModuleResolver::catalog_from_cwd(cwd.as_ref(), builtin_list())
            .map_err(|error| ModuleError::new(error.to_string()))
    }

    pub fn module_catalog_in_workspace(
        &self,
        workspace: Arc<crate::package::ResolvedWorkspace>,
        cwd: impl AsRef<Path>,
    ) -> Result<Vec<ModuleCatalogEntry>, ModuleError> {
        ModuleResolver::catalog_from_workspace(workspace, cwd.as_ref(), builtin_list())
            .map_err(|error| ModuleError::new(error.to_string()))
    }

    pub fn recover_builtin_workspace(
        &self,
        module_id: &str,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        if !is_public_builtin_name(module_id)
            || !builtin_list()
            .iter()
            .any(|(name, _)| name == module_id)
        {
            return Err(ModuleError::new(format!(
                "unknown built-in module {module_id:?}"
            )));
        }
        let mut main = MainWorld::building();
        let mut sources = SourceDatabase::default();
        let mut inputs = BTreeMap::new();
        install_native_modules_observed(
            &mut main,
            &mut sources,
            &self.debug_sink,
            Some(&mut inputs),
        )?;
        Ok(WorkspaceSnapshot::build(
            sources,
            inputs.into_values().collect(),
        ))
    }

    pub fn recover_standalone(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        let resolver = ModuleResolver::standalone(path.as_ref())
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list());
        self.recover_with_resolver(resolver)
    }

    fn recover_with_resolver(
        &self,
        resolver: ModuleResolver,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        let root_module = resolver
            .selected_root()
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let opaque_modules = builtin_list()
            .into_iter()
            .map(|(name, _)| ModuleCName::builtin(name));
        let graph = ModuleGraph::discover(
            &resolver,
            vec![root_module.clone()],
            &BTreeMap::new(),
            opaque_modules,
            None,
            true,
        )?;
        let mut main = MainWorld::with_modules(graph);
        let mut sources = SourceDatabase::default();
        let builtin_modules = install_native_modules(&mut main, &mut sources, &self.debug_sink)?;
        let mut builder = WorkspaceBuilder {
            engine: self,
            resolver,
            overlays: &BTreeMap::new(),
            query: None,
            sources,
            main,
            builtin_modules,
            inputs: BTreeMap::new(),
            provenances: HashMap::new(),
            roots: HashMap::new(),
            interfaces: HashMap::new(),
            visiting: Vec::new(),
            cycle_members: HashSet::new(),
            cycle_reported: false,
        };
        block_on_recovery(builder.load_telora(root_module));
        Ok(WorkspaceSnapshot::build(
            builder.sources,
            builder.inputs.into_values().collect(),
        ))
    }

    pub async fn recover_workspace_async(
        &self,
        path: impl AsRef<Path>,
        overlays: &BTreeMap<PathBuf, crate::document::DocumentText>,
        context: &crate::query::QueryContext,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        context
            .checkpoint()
            .await
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let root_path = path.as_ref();
        let resolver = overlays
            .get(root_path)
            .map_or_else(
                || ModuleResolver::for_root(root_path),
                |source| ModuleResolver::for_root_with_source(root_path, source),
            )
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list());
        self.recover_with_resolver_async(resolver, overlays, context)
            .await
    }

    pub async fn recover_workspace_async_in_workspace(
        &self,
        workspace: Arc<crate::package::ResolvedWorkspace>,
        path: impl AsRef<Path>,
        overlays: &BTreeMap<PathBuf, crate::document::DocumentText>,
        context: &crate::query::QueryContext,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        context
            .checkpoint()
            .await
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let root_path = path.as_ref();
        let resolver = ModuleResolver::for_root_in_workspace(
            workspace,
            root_path,
            overlays.get(root_path),
        )
        .map_err(|error| ModuleError::new(error.to_string()))?
        .with_builtins(builtin_list());
        self.recover_with_resolver_async(resolver, overlays, context)
            .await
    }

    async fn recover_with_resolver_async(
        &self,
        resolver: ModuleResolver,
        overlays: &BTreeMap<PathBuf, crate::document::DocumentText>,
        context: &crate::query::QueryContext,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        let root_module = resolver
            .selected_root()
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let opaque_modules = builtin_list()
            .into_iter()
            .map(|(name, _)| ModuleCName::builtin(name));
        let graph = ModuleGraph::discover(
            &resolver,
            vec![root_module.clone()],
            &BTreeMap::new(),
            opaque_modules,
            Some(overlays),
            true,
        )?;
        let mut main = MainWorld::with_modules(graph);
        let mut sources = SourceDatabase::default();
        let builtin_modules = install_native_modules(&mut main, &mut sources, &self.debug_sink)?;
        let mut builder = WorkspaceBuilder {
            engine: self,
            resolver,
            overlays,
            query: Some(context),
            sources,
            main,
            builtin_modules,
            inputs: BTreeMap::new(),
            provenances: HashMap::new(),
            roots: HashMap::new(),
            interfaces: HashMap::new(),
            visiting: Vec::new(),
            cycle_members: HashSet::new(),
            cycle_reported: false,
        };
        builder.load_telora(root_module).await;
        context
            .checkpoint()
            .await
            .map_err(|error| ModuleError::new(error.to_string()))?;
        Ok(WorkspaceSnapshot::build(
            builder.sources,
            builder.inputs.into_values().collect(),
        ))
    }
}
