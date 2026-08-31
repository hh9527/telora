#[derive(Clone, Debug)]
pub struct ModuleError {
    message: String,
}

impl ModuleError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ModuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ModuleError {}

#[derive(Clone, Debug)]
pub struct LoadedModule {
    pub path: PathBuf,
    pub dependencies: Vec<PathBuf>,
    pub analysis: Analysis,
    pub function: BytecodeFunction,
    pub sources: SourceDatabase,
    pub workspace: WorkspaceSnapshot,
    options: Vec<LoadedOptionAction>,
    runtime: Arc<ModuleRuntime>,
}

#[derive(Clone, Debug)]
pub struct LoadedOptionAction {
    pub key: String,
    pub value: crate::DataWorld,
}

#[derive(Clone)]
pub struct PendingModule {
    inner: Arc<PendingModuleInner>,
}

struct PendingModuleInner {
    path: PathBuf,
    resolver: ModuleResolver,
    options: Vec<LoadedOptionAction>,
    config: EngineConfig,
    debug_sink: Arc<dyn DebugSink>,
    state: Mutex<PendingModuleState>,
}

enum PendingModuleState {
    Pending {
        bindings: BTreeMap<String, crate::DataWorld>,
    },
    Initializing,
    Ready(InstantiatedModule),
    Failed(ModuleError),
}

#[derive(Clone)]
pub struct InstantiatedModule {
    module: Arc<LoadedModule>,
    execution: Arc<WorkWorld>,
}

impl fmt::Debug for InstantiatedModule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstantiatedModule")
            .field("module", &self.module.path)
            .finish_non_exhaustive()
    }
}

impl PendingModule {
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn option_actions(&self) -> &[LoadedOptionAction] {
        &self.inner.options
    }

    fn begin_initialization(&self) -> Result<BTreeMap<String, crate::DataWorld>, ModuleError> {
        {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("pending module state poisoned");
            match &mut *state {
                PendingModuleState::Pending { bindings } => {
                    let bindings = std::mem::take(bindings);
                    *state = PendingModuleState::Initializing;
                    Ok(bindings)
                }
                PendingModuleState::Initializing => {
                    Err(ModuleError::new(
                        "module initialization is already in progress",
                    ))
                }
                PendingModuleState::Ready(_) => {
                    Err(ModuleError::new("module is already initialized"))
                }
                PendingModuleState::Failed(error) => Err(error.clone()),
            }
        }
    }

    fn finish_initialization(&self, result: &Result<InstantiatedModule, ModuleError>) {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("pending module state poisoned");
        *state = match result {
            Ok(module) => PendingModuleState::Ready(module.clone()),
            Err(error) => PendingModuleState::Failed(error.clone()),
        };
    }

    pub fn initialize(&self) -> Result<InstantiatedModule, ModuleError> {
        {
            let state = self
                .inner
                .state
                .lock()
                .expect("pending module state poisoned");
            if let PendingModuleState::Ready(module) = &*state {
                return Ok(module.clone());
            }
        }
        let bindings = self.begin_initialization()?;
        let resolver = self
            .inner
            .resolver
            .clone()
            .with_builtins(builtin_list());
        let result = load_module_with_resolver(
            resolver,
            bindings,
            self.inner.config.module_quota,
            self.inner.config.data_limits,
            Arc::clone(&self.inner.debug_sink),
            ModuleSourcePolicy::ExplicitExports,
        )
        .and_then(|module| {
            let (execution, _) = module.execute_world_observed(
                self.inner.config.session_quota,
                Arc::clone(&self.inner.debug_sink),
            );
            let execution = execution.map_err(|error| ModuleError::new(error.to_string()))?;
            Ok(InstantiatedModule {
                module: Arc::new(module),
                execution: Arc::new(execution),
            })
        });
        self.finish_initialization(&result);
        result
    }
}

impl InstantiatedModule {
    pub fn module(&self) -> &LoadedModule {
        &self.module
    }
}

struct CompiledTeloraModule {
    analysis: Analysis,
    function: BytecodeFunction,
    externals: HashMap<String, Val>,
    options: Vec<LoadedOptionAction>,
}

fn loaded_from_compiled(
    path: PathBuf,
    dependencies: Vec<PathBuf>,
    sources: SourceDatabase,
    workspace: WorkspaceSnapshot,
    main: Arc<FrozenMainWorld>,
    compiled: CompiledTeloraModule,
) -> LoadedModule {
    LoadedModule {
        path,
        dependencies,
        analysis: compiled.analysis,
        function: compiled.function,
        sources,
        workspace,
        options: compiled.options,
        runtime: Arc::new(ModuleRuntime {
            main,
            externals: compiled.externals,
        }),
    }
}

struct ModuleRuntime {
    main: Arc<FrozenMainWorld>,
    externals: HashMap<String, Val>,
}

fn runtime_roots(roots: &HashMap<String, PersistentValue>) -> HashMap<String, Val> {
    roots
        .iter()
        .map(|(name, root)| (name.clone(), root.runtime()))
        .collect()
}

fn install_type_family_roots(roots: &mut HashMap<String, PersistentValue>, analysis: &Analysis) {
    roots.extend(
        analysis
            .runtime_roots
            .iter()
            .map(|(name, root)| (name.clone(), *root)),
    );
    roots.extend(
        analysis
            .type_family_values
            .iter()
            .flat_map(|(name, family)| {
                [
                    (type_family_link_key(name), family.root()),
                    (type_family_template_link_key(name), family.template()),
                ]
            }),
    );
}
