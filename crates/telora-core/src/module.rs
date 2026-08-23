use crate::ast::{BindingKind, Expr, ExprKind, Program, StringPartKind, TypeArgumentKind};
use crate::compiler::{
    compile_program_analyzed_in_module, compile_program_with_promoted_types_and_static_funcs,
    function_contract_arity, metadata_compilation_plan, type_family_link_key,
    type_family_template_link_key,
};
use crate::core::{
    DEFAULT_ENTRY_MODULE, EDGE_RUNTIME_MODULE, PRELUDE_MODULE, default_entry_source,
    edge_runtime_source, module_specs,
};
use crate::heap::{DecodedValue, Heap, Object, PersistentValue, Val, semantic_value_type_id};
use crate::json::{
    Provenance, SemanticDataTarget, ValidatedDataPlan, materialize_data_plan,
    validate_json_registered,
};
use crate::module_id::{
    ModuleAuthority, ModuleCName, ModuleCatalogEntry, ModuleFormat, ModuleId, ModuleResolver,
    ResolvedModule, immediate_value,
};
use crate::parser::parse_registered;
use crate::semantic::{
    SemanticImport, SemanticModuleInput, SemanticModuleInterface, WorkspaceModuleKind,
    WorkspaceModuleState, WorkspaceSnapshot,
};
use crate::source::{Diagnostic, SourceDatabase};
use crate::toml::validate_toml_registered;
use crate::type_store::TypeStore;
use crate::types::{
    Analysis, ModuleInterface, PartialAnalysisControl, TypeDescriptor, TypeFamilyTemplate,
    TypeScheme, analyze_partial_types_recovered_with_query, analyze_program_with_bindings_observed,
    program_references_name, recovered_reference_locations,
};
#[cfg(test)]
use crate::vm::ValueRef;
use crate::vm::WorkWorld;
use crate::yaml::validate_yaml_registered;
use crate::{
    BuiltinAtom, BytecodeFunction, DebugSink, DiscardDebugSink, Instruction, Quota, QuotaAccount,
    Register, Vm,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct StaticDataParse {
    plan: Option<ValidatedDataPlan>,
    diagnostics: Vec<Diagnostic>,
    kind: WorkspaceModuleKind,
}

#[derive(Clone)]
struct ModuleArtifact {
    root: PersistentValue,
    interface: ModuleInterface,
    provenance: Option<Provenance>,
}

#[derive(Clone)]
struct OpenImportCandidate {
    provider: ModuleCName,
    root: PersistentValue,
    scheme: crate::types::TypeScheme,
    provenance: Option<Provenance>,
    concrete_types: BTreeMap<String, TypeDescriptor>,
    type_family_template: Option<TypeFamilyTemplate>,
}

#[derive(Clone)]
struct WorkspaceOpenImportCandidate {
    provider: ModuleCName,
    scheme: crate::types::TypeScheme,
    root: PersistentValue,
    concrete_types: BTreeMap<String, TypeDescriptor>,
    type_family_template: Option<TypeFamilyTemplate>,
}

fn workspace_open_import_exports(
    provider: &ModuleCName,
    interface: &ModuleInterface,
    root: PersistentValue,
    heap: &Heap,
) -> Result<Vec<(String, WorkspaceOpenImportCandidate)>, ModuleError> {
    interface
        .exports
        .iter()
        .map(|(name, scheme)| {
            let field_root = root
                .export_get(heap, name)
                .map_err(|error| ModuleError::new(error.to_string()))?
                .ok_or_else(|| {
                    ModuleError::new(format!("module {provider} has no root for export {name:?}"))
                })?;
            Ok((
                name.clone(),
                WorkspaceOpenImportCandidate {
                    provider: provider.clone(),
                    scheme: scheme.clone(),
                    root: field_root,
                    concrete_types: interface.concrete_types.clone(),
                    type_family_template: interface.type_family_templates.get(name).cloned(),
                },
            ))
        })
        .collect()
}

fn open_import_exports(
    provider: &ModuleCName,
    root: PersistentValue,
    interface: &ModuleInterface,
    heap: &Heap,
    provenance: Option<&Provenance>,
) -> Result<Vec<(String, OpenImportCandidate)>, ModuleError> {
    interface
        .exports
        .iter()
        .map(|(name, scheme)| {
            let root = root
                .export_get(heap, name)
                .map_err(|error| ModuleError::new(error.to_string()))?
                .ok_or_else(|| {
                    ModuleError::new(format!("module {provider} has no root for export {name:?}"))
                })?;
            Ok((
                name.clone(),
                OpenImportCandidate {
                    provider: provider.clone(),
                    root,
                    scheme: scheme.clone(),
                    provenance: provenance.cloned(),
                    concrete_types: interface.concrete_types.clone(),
                    type_family_template: interface.type_family_templates.get(name).cloned(),
                },
            ))
        })
        .collect()
}

fn static_data_kind(format: ModuleFormat) -> Option<WorkspaceModuleKind> {
    match format {
        ModuleFormat::Json => Some(WorkspaceModuleKind::Json),
        ModuleFormat::Toml => Some(WorkspaceModuleKind::Toml),
        ModuleFormat::Yaml => Some(WorkspaceModuleKind::Yaml),
        _ => None,
    }
}

fn parse_static_data_registered(
    format: ModuleFormat,
    sources: &SourceDatabase,
    source_id: crate::SourceId,
) -> Option<StaticDataParse> {
    let kind = static_data_kind(format)?;
    let result = match format {
        ModuleFormat::Json => validate_json_registered(sources, source_id),
        ModuleFormat::Toml => validate_toml_registered(sources, source_id),
        ModuleFormat::Yaml => validate_yaml_registered(sources, source_id),
        _ => unreachable!("kind exists only for static data formats"),
    };
    let (plan, diagnostics) = match result {
        Ok(plan) => (Some(plan), Vec::new()),
        Err(diagnostics) => (None, diagnostics),
    };
    Some(StaticDataParse {
        plan,
        diagnostics,
        kind,
    })
}

fn semantic_value_contract(
    core_modules: &HashMap<String, ModuleArtifact>,
    heap: &Heap,
) -> Result<(PersistentValue, TypeDescriptor), ModuleError> {
    let module = core_modules
        .get(crate::core::VALUE_MODULE)
        .ok_or_else(|| ModuleError::new("std/value is not installed"))?;
    let owner = module
        .root
        .export_get(heap, "Value")
        .map_err(|error| ModuleError::new(error.to_string()))?
        .ok_or_else(|| ModuleError::new("std/value has no Value export"))?;
    let descriptor = module
        .interface
        .exports
        .get("Value")
        .and_then(|scheme| match &scheme.body {
            TypeDescriptor::TypeOf(descriptor) => Some(descriptor.as_ref().clone()),
            _ => None,
        })
        .ok_or_else(|| ModuleError::new("std/value Value export is not TypeOf(Value)"))?;
    Ok((owner, descriptor))
}

fn static_data_interface(descriptor: TypeDescriptor) -> ModuleInterface {
    ModuleInterface {
        exports: BTreeMap::from([(
            "data".into(),
            TypeScheme {
                parameters: Vec::new(),
                body: descriptor.clone(),
            },
        )]),
        concrete_types: BTreeMap::from([("Value".into(), descriptor)]),
        type_family_templates: BTreeMap::new(),
    }
}

fn publish_static_data_module(
    plan: &ValidatedDataPlan,
    core_modules: &HashMap<String, ModuleArtifact>,
    heap: &mut Heap,
    source_bytes: usize,
    data_limits: DataLimits,
) -> Result<(PersistentValue, ModuleInterface, Provenance), ModuleError> {
    let (owner, descriptor) = semantic_value_contract(core_modules, heap)?;
    plan.enforce_limits(data_limits, source_bytes)
        .map_err(|error| ModuleError::new(error.to_string()))?;
    let type_id = semantic_value_type_id(heap, None, owner.runtime())
        .map_err(|error| ModuleError::new(error.to_string()))?;
    let sourced = materialize_data_plan(
        plan,
        heap,
        Some(SemanticDataTarget {
            background: None,
            type_id,
        }),
    );
    let data = sourced.value;
    let root = heap
        .module([("data".into(), data)])
        .and_then(|value| heap.persistent(value))
        .map_err(|error| ModuleError::new(error.to_string()))?;
    let interface = static_data_interface(descriptor);
    Ok((root, interface, sourced.provenance))
}

fn validate_system_data_source(
    format: SystemDataFormat,
    sources: &SourceDatabase,
    source_id: crate::SourceId,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    match format {
        SystemDataFormat::Json => validate_json_registered(sources, source_id),
        SystemDataFormat::Yaml => validate_yaml_registered(sources, source_id),
        SystemDataFormat::Toml => validate_toml_registered(sources, source_id),
    }
}

fn allocate_record(heap: &mut Heap, fields: impl IntoIterator<Item = (String, Val)>) -> Val {
    let mut fields = fields.into_iter().collect::<Vec<_>>();
    fields.sort_by(|left, right| left.0.cmp(&right.0));
    let names = fields
        .iter()
        .map(|(name, _)| heap.intern(name))
        .collect::<Vec<_>>();
    let values = fields
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let shape = heap.intern_shape(names);
    Val::unknown(DecodedValue::Dict(heap.allocate(Object::Dict {
        shape,
        values: values.into_boxed_slice(),
    })))
}

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
    native_modules: Arc<[RegisteredNativeModule]>,
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
                    return Err(ModuleError::new(
                        "module initialization is already in progress",
                    ));
                }
                PendingModuleState::Ready(_) => {
                    return Err(ModuleError::new("module is already initialized"));
                }
                PendingModuleState::Failed(error) => return Err(error.clone()),
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
            .with_builtins(builtin_list(&self.inner.native_modules));
        let result = load_module_with_resolver(
            resolver,
            bindings,
            self.inner.config.module_quota,
            self.inner.config.data_limits,
            Arc::clone(&self.inner.debug_sink),
            &self.inner.native_modules,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaticSlotKind {
    Func,
    TypeConstructor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StaticSlot {
    local: u32,
    name: String,
    kind: StaticSlotKind,
    type_arity: Option<u32>,
    declarations: u32,
    definitions: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ImportEdge {
    local: Option<String>,
    target: ModuleId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExportPlan {
    public: String,
    local: String,
}

#[derive(Clone, Debug)]
struct ModuleSkeleton {
    id: ModuleId,
    cname: ModuleCName,
    imports: Vec<ImportEdge>,
    exports: Vec<ExportPlan>,
    slots: Vec<StaticSlot>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ModuleBlueprint {
    imports: Vec<(Option<String>, ModuleCName)>,
    exports: Vec<ExportPlan>,
    slots: Vec<StaticSlot>,
}

#[derive(Clone, Debug, Default)]
struct ModuleGraph {
    modules: Vec<ModuleSkeleton>,
    by_cname: HashMap<ModuleCName, ModuleId>,
}

impl ModuleGraph {
    fn id(&self, cname: &ModuleCName) -> Option<ModuleId> {
        self.by_cname.get(cname).copied()
    }

    fn module(&self, id: ModuleId) -> &ModuleSkeleton {
        &self.modules[id.index()]
    }

    fn static_funcs(&self, id: ModuleId) -> HashMap<String, crate::FuncId> {
        if id.raw() < ModuleId::FIRST_DYNAMIC {
            return HashMap::new();
        }
        self.module(id)
            .slots
            .iter()
            .filter(|slot| slot.kind == StaticSlotKind::Func)
            .map(|slot| {
                (
                    slot.name.clone(),
                    crate::FuncId {
                        module: id,
                        local: slot.local,
                    },
                )
            })
            .collect()
    }

    fn discover(
        resolver: &ModuleResolver,
        roots: Vec<ResolvedModule>,
        synthetic: &BTreeMap<ModuleCName, (PathBuf, String)>,
        opaque: impl IntoIterator<Item = ModuleCName>,
        overlays: Option<&BTreeMap<PathBuf, crate::document::DocumentText>>,
        recover: bool,
    ) -> Result<Self, ModuleError> {
        let mut resolved = roots
            .into_iter()
            .map(|module| (module.id.clone(), module))
            .collect::<HashMap<_, _>>();
        let mut pending = resolved.keys().cloned().collect::<Vec<_>>();
        pending.extend(synthetic.keys().cloned());
        pending.extend(opaque);
        let mut blueprints = HashMap::new();
        let mut scan_sources = SourceDatabase::default();

        while let Some(cname) = pending.pop() {
            if blueprints.contains_key(&cname) {
                continue;
            }
            let source = if let Some((context, source)) = synthetic.get(&cname) {
                Some((context.clone(), source.clone()))
            } else if let Some(module) = resolved.get(&cname) {
                match (module.format, module.path()) {
                    (ModuleFormat::Telora, Some(path)) => Some((
                        path.to_owned(),
                        overlays
                            .and_then(|overlays| overlays.get(path))
                            .map(ToString::to_string)
                            .map(Ok)
                            .unwrap_or_else(|| read(path))?,
                    )),
                    _ => None,
                }
            } else {
                None
            };
            let Some((context_path, source)) = source else {
                blueprints.insert(cname, ModuleBlueprint::default());
                continue;
            };
            let source_id = scan_sources.add(cname.to_string(), source);
            let parsed = parse_registered(&scan_sources, source_id);
            if let Some(option) = parsed
                .options
                .iter()
                .find(|option| option.key.value.starts_with("crate.") && !resolver.is_standalone())
            {
                return Err(ModuleError::new(scan_sources.render(&Diagnostic::error(
                    format!(
                        "resolver option {:?} is only allowed in standalone mode",
                        option.key.value
                    ),
                    option.location,
                ))));
            }
            if let Some(option) = parsed.options.iter().find(|_| !resolver.is_root(&cname)) {
                return Err(ModuleError::new(scan_sources.render(&Diagnostic::error(
                    format!(
                        "option {:?} is only allowed in the selected root",
                        option.key.value
                    ),
                    option.location,
                ))));
            }
            let mut blueprint = match parsed.program.as_ref() {
                Some(program) => {
                    reject_nested_imports(program, &cname.to_string())?;
                    if !recover
                        && let Some(diagnostic) =
                            module_binding_diagnostics(program).into_iter().next()
                    {
                        return Err(ModuleError::new(scan_sources.render(&diagnostic)));
                    }
                    match ModuleBlueprint::from_program(program) {
                        Ok(blueprint) => blueprint,
                        Err(_) if recover => ModuleBlueprint::default(),
                        Err(message) => {
                            return Err(ModuleError::new(format!(
                                "module {cname} has an invalid skeleton: {message}"
                            )));
                        }
                    }
                }
                None if recover => ModuleBlueprint::default(),
                None => {
                    return Err(ModuleError::new(
                        parsed
                            .diagnostics
                            .iter()
                            .map(|diagnostic| scan_sources.render(diagnostic))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ));
                }
            };
            let bindings = parsed
                .program
                .as_ref()
                .map_or(parsed.recovered.bindings.as_slice(), |program| {
                    program.value.body.value.bindings.as_slice()
                });
            for binding in bindings {
                if !matches!(
                    binding.value.kind,
                    BindingKind::Import | BindingKind::OpenImport
                ) {
                    continue;
                }
                let ExprKind::String(target) = &binding.value.value.value else {
                    return Err(ModuleError::new("import path must be a string"));
                };
                let imported = match resolver.resolve_import(&cname, target) {
                    Ok(imported) => imported,
                    Err(_) if recover => continue,
                    Err(error) => {
                        return Err(ModuleError::new(scan_sources.render(&Diagnostic::error(
                            error.to_string(),
                            binding.value.value.location,
                        ))));
                    }
                };
                let imported_cname = imported.id.clone();
                blueprint.imports.push((
                    (binding.value.kind == BindingKind::Import)
                        .then(|| binding.value.name.value.clone()),
                    imported_cname.clone(),
                ));
                resolved.entry(imported_cname.clone()).or_insert(imported);
                pending.push(imported_cname);
            }
            if cname.to_string() != PRELUDE_MODULE {
                let prelude = ModuleCName::builtin(PRELUDE_MODULE);
                blueprint.imports.push((None, prelude.clone()));
                pending.push(prelude);
            }
            let _ = context_path;
            blueprints.insert(cname, blueprint);
        }

        let mut cnames = blueprints.keys().cloned().collect::<Vec<_>>();
        cnames.sort_by(|left, right| {
            left.to_string()
                .as_bytes()
                .cmp(right.to_string().as_bytes())
        });
        let by_cname = cnames
            .iter()
            .enumerate()
            .map(|(index, cname)| (cname.clone(), ModuleId::from_index(index)))
            .collect::<HashMap<_, _>>();
        let modules = cnames
            .into_iter()
            .map(|cname| {
                let id = by_cname[&cname];
                let blueprint = blueprints.remove(&cname).expect("cname was discovered");
                let imports = blueprint
                    .imports
                    .into_iter()
                    .map(|(local, target)| ImportEdge {
                        local,
                        target: by_cname[&target],
                    })
                    .collect();
                ModuleSkeleton {
                    id,
                    cname,
                    imports,
                    exports: blueprint.exports,
                    slots: blueprint.slots,
                }
            })
            .collect();
        Ok(Self { modules, by_cname })
    }
}

impl ModuleBlueprint {
    fn from_program(program: &Program) -> Result<Self, String> {
        #[derive(Clone, Copy)]
        enum VisibleBinding {
            Decl,
            Def,
            Other,
        }

        let mut blueprint = Self::default();
        let mut next_func = crate::FIRST_DYNAMIC_MODULE_LOCAL;
        let mut next_type = crate::FIRST_DYNAMIC_MODULE_LOCAL;
        let mut funcs = HashMap::<String, usize>::new();
        let mut type_constructors = HashSet::new();
        let mut visible = HashMap::<String, VisibleBinding>::new();
        for binding in &program.value.body.value.bindings {
            let name = binding.value.name.value.clone();
            match binding.value.kind {
                BindingKind::Decl => {
                    if visible.contains_key(&name) {
                        return Err(format!(
                            "declaration {name:?} cannot shadow a visible binding"
                        ));
                    }
                    visible.insert(name.clone(), VisibleBinding::Decl);
                }
                BindingKind::Def => match visible.get(&name) {
                    None | Some(VisibleBinding::Decl) => {
                        visible.insert(name.clone(), VisibleBinding::Def);
                    }
                    Some(VisibleBinding::Def | VisibleBinding::Other) => {
                        return Err(format!(
                            "definition {name:?} cannot shadow a visible binding"
                        ));
                    }
                },
                BindingKind::Let => {
                    visible.insert(name.clone(), VisibleBinding::Other);
                }
                BindingKind::Import
                | BindingKind::Native
                | BindingKind::NativeType
                | BindingKind::Type => {
                    if visible
                        .insert(name.clone(), VisibleBinding::Other)
                        .is_some()
                    {
                        return Err(format!(
                            "module binding {name:?} conflicts with an earlier explicit binding"
                        ));
                    }
                }
                BindingKind::OpenImport | BindingKind::Export => {}
            }
            match binding.value.kind {
                BindingKind::Decl | BindingKind::Def
                    if binding
                        .value
                        .annotation
                        .as_ref()
                        .and_then(function_contract_arity)
                        .or_else(|| match &binding.value.value.value {
                            ExprKind::Closure { parameters, .. } => {
                                u32::try_from(parameters.len()).ok()
                            }
                            _ => None,
                        })
                        .is_some() =>
                {
                    if let Some(index) = funcs.get(&name).copied() {
                        let slot = &mut blueprint.slots[index];
                        slot.declarations += u32::from(binding.value.kind == BindingKind::Decl);
                        slot.definitions += u32::from(binding.value.kind == BindingKind::Def);
                    } else {
                        funcs.insert(name.clone(), blueprint.slots.len());
                        blueprint.slots.push(StaticSlot {
                            local: next_func,
                            name,
                            kind: StaticSlotKind::Func,
                            type_arity: None,
                            declarations: u32::from(binding.value.kind == BindingKind::Decl),
                            definitions: u32::from(binding.value.kind == BindingKind::Def),
                        });
                        next_func += 1;
                    }
                }
                BindingKind::Type
                    if binding.value.declared_initializer.is_some()
                        && type_constructors.insert(binding.value.name.value.clone()) =>
                {
                    blueprint.slots.push(StaticSlot {
                        local: next_type,
                        name: binding.value.name.value.clone(),
                        kind: StaticSlotKind::TypeConstructor,
                        type_arity: Some(
                            u32::try_from(binding.value.type_parameters.len())
                                .map_err(|_| "type constructor arity exceeds u32".to_owned())?,
                        ),
                        declarations: 1,
                        definitions: 1,
                    });
                    next_type += 1;
                }
                BindingKind::Export => blueprint.exports.push(ExportPlan {
                    public: binding.value.name.value.clone(),
                    local: binding
                        .value
                        .imported_name
                        .as_ref()
                        .expect("parser exports retain their local name")
                        .value
                        .clone(),
                }),
                _ => {}
            }
        }
        for slot in &blueprint.slots {
            if slot.kind != StaticSlotKind::Func {
                continue;
            }
            if slot.declarations > 1 {
                return Err(format!(
                    "function {:?} is declared more than once",
                    slot.name
                ));
            }
            match slot.definitions {
                0 => return Err(format!("function {:?} has no definition", slot.name)),
                1 => {}
                _ => {
                    return Err(format!(
                        "function {:?} is defined more than once",
                        slot.name
                    ));
                }
            }
        }
        Ok(blueprint)
    }
}

struct MainWorld {
    heap: Heap,
    modules: ModuleGraph,
    types: TypeStore,
    failures: Vec<crate::RuntimeError>,
}

impl MainWorld {
    fn building() -> Self {
        Self::with_modules(ModuleGraph::default())
    }

    fn with_modules(modules: ModuleGraph) -> Self {
        let mut heap = Heap::main();
        for module in &modules.modules {
            for slot in module
                .slots
                .iter()
                .filter(|slot| slot.kind == StaticSlotKind::Func)
            {
                heap.preallocate_func(crate::FuncId {
                    module: module.id,
                    local: slot.local,
                })
                .expect("module graph contains unique function slots");
            }
        }
        let mut types = TypeStore::default();
        for module in &modules.modules {
            for slot in module.slots.iter().filter(|slot| {
                slot.kind == StaticSlotKind::TypeConstructor && slot.type_arity == Some(0)
            }) {
                let constructor = crate::TypeConstructorId {
                    module: module.id,
                    local: slot.local,
                };
                assert!(matches!(
                    types.begin(constructor, []),
                    crate::type_store::InternType::Reserved(_)
                ));
            }
        }
        Self {
            heap,
            modules,
            types,
            failures: Vec::new(),
        }
    }

    fn seal(self) -> FrozenMainWorld {
        FrozenMainWorld {
            heap: Arc::new(self.heap),
        }
    }
}

struct FrozenMainWorld {
    heap: Arc<Heap>,
}

fn declared_native_types(
    program: &Program,
    native_module: crate::value::NativeModuleId,
    module_name: &str,
    sources: &SourceDatabase,
) -> Result<BTreeMap<u32, (String, crate::NativeType)>, ModuleError> {
    let mut native_types = BTreeMap::new();
    for binding in program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| binding.value.kind == BindingKind::NativeType)
    {
        let ExprKind::Int(slot) = binding.value.value.value else {
            unreachable!("native type grammar supplies an integer slot")
        };
        let local = u32::try_from(slot).map_err(|_| {
            ModuleError::new(sources.render(&crate::source::Diagnostic::error(
                "native type slot must fit the u32 range",
                binding.value.value.location,
            )))
        })?;
        let name = binding.value.name.value.clone();
        let native_type = crate::NativeType::bind(
            crate::value::NativeTypeId {
                module: native_module,
                local,
            },
            format!("{module_name}#{name}"),
        );
        if native_types.insert(local, (name, native_type)).is_some() {
            return Err(ModuleError::new(sources.render(
                &crate::source::Diagnostic::error(
                    format!("duplicate native type slot @{local} in {module_name}"),
                    binding.value.value.location,
                ),
            )));
        }
    }
    Ok(native_types)
}

struct TrustedNativeModule {
    id: u32,
    name: String,
    source: String,
    functions: Vec<(String, crate::NativeFunction)>,
    core: bool,
}

fn builtin_list(host_modules: &[RegisteredNativeModule]) -> Vec<(String, u32)> {
    module_specs()
        .into_iter()
        .map(|spec| (spec.name.to_owned(), spec.native_id))
        .chain(host_modules.iter().map(|spec| (spec.name.clone(), spec.id)))
        .collect()
}

fn install_native_modules(
    main: &mut MainWorld,
    sources: &mut SourceDatabase,
    debug_sink: &Arc<dyn DebugSink>,
    host_modules: &[RegisteredNativeModule],
) -> Result<HashMap<String, ModuleArtifact>, ModuleError> {
    install_native_modules_observed(main, sources, debug_sink, host_modules, None)
}

fn install_native_modules_observed(
    main: &mut MainWorld,
    sources: &mut SourceDatabase,
    debug_sink: &Arc<dyn DebugSink>,
    host_modules: &[RegisteredNativeModule],
    mut semantic_inputs: Option<&mut BTreeMap<String, SemanticModuleInput>>,
) -> Result<HashMap<String, ModuleArtifact>, ModuleError> {
    let mut modules: HashMap<String, ModuleArtifact> = HashMap::new();
    let mut specs = module_specs()
        .into_iter()
        .map(|spec| TrustedNativeModule {
            id: spec.native_id,
            name: spec.name.to_owned(),
            source: spec.source.to_owned(),
            functions: spec
                .functions
                .into_iter()
                .map(|(name, function)| (name.to_owned(), function))
                .collect(),
            core: true,
        })
        .collect::<Vec<_>>();
    specs.extend(host_modules.iter().map(|spec| TrustedNativeModule {
        id: spec.id,
        name: spec.name.clone(),
        source: spec.source.clone(),
        functions: spec.functions.clone(),
        core: false,
    }));
    specs.sort_by_key(|spec| u8::from(spec.name != PRELUDE_MODULE));
    let mut native_module_ids = HashMap::new();
    for spec in &specs {
        let valid_id = if spec.core {
            spec.id > 0 && spec.id <= crate::value::RESERVED_NATIVE_MODULE_MAX
        } else {
            spec.id > crate::value::RESERVED_NATIVE_MODULE_MAX
        };
        if !valid_id {
            return Err(ModuleError::new(format!(
                "native module {} has invalid {} module ID {}",
                spec.name,
                if spec.core { "reserved" } else { "Host" },
                spec.id
            )));
        }
        if let Some(previous) = native_module_ids.insert(spec.id, spec.name.clone()) {
            return Err(ModuleError::new(format!(
                "native modules {previous} and {} use duplicate native module ID {}",
                spec.name, spec.id
            )));
        }
    }
    let mut default_prelude: Option<BTreeMap<String, PersistentValue>> = None;
    for spec in specs {
        let source_name = format!("<{}.native.telora", spec.name);
        let source_id = sources.add(source_name.clone(), &spec.source);
        let parsed = parse_registered(sources, source_id);
        let program = parsed.program.ok_or_else(|| {
            ModuleError::new(
                parsed
                    .diagnostics
                    .iter()
                    .map(|diagnostic| sources.render(diagnostic))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })?;
        let implementations = spec.functions.into_iter().collect::<HashMap<_, _>>();
        let mut external_roots = HashMap::new();
        let mut external_interfaces = BTreeMap::new();
        for binding in &program.value.body.value.bindings {
            if binding.value.kind != BindingKind::Import {
                continue;
            }
            let ExprKind::String(request) = &binding.value.value.value else {
                return Err(ModuleError::new("built-in import path must be a String"));
            };
            let module = modules.get(request.as_str()).ok_or_else(|| {
                ModuleError::new(sources.render(&Diagnostic::error(
                    format!(
                        "built-in module {} imports unavailable earlier built-in {request:?}",
                        spec.name
                    ),
                    binding.value.value.location,
                )))
            })?;
            let (root, interface) = select_import_root(
                module.root,
                module.interface.clone(),
                binding.value.imported_name.as_deref(),
                &binding.value.name.value,
                &main.heap,
            )?;
            external_roots.insert(binding.value.name.value.clone(), root);
            external_interfaces.insert(binding.value.name.value.clone(), interface);
        }
        let native_module = crate::value::NativeModuleId(spec.id);
        let native_types = declared_native_types(&program, native_module, &spec.name, sources)?;
        for (name, native_type) in native_types.values() {
            let value = main.heap.native_type_value(native_type.clone());
            let root = main
                .heap
                .persistent(value)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            external_roots.insert(name.clone(), root);
        }
        for binding in &program.value.body.value.bindings {
            if binding.value.kind != BindingKind::Native {
                continue;
            }
            let symbol = binding.value.name.value.as_str();
            let implementation = implementations.get(symbol).copied().ok_or_else(|| {
                ModuleError::new(sources.render(&crate::source::Diagnostic::error(
                    format!(
                        "native symbol {symbol:?} is not registered for {}",
                        spec.name
                    ),
                    binding.location,
                )))
            })?;
            let declared_arity = binding
                .value
                .annotation
                .as_ref()
                .and_then(function_contract_arity)
                .expect("native grammar requires a function contract");
            if declared_arity as usize != implementation.arity() {
                return Err(ModuleError::new(sources.render(
                    &crate::source::Diagnostic::error(
                        format!(
                            "native symbol {symbol:?} declares arity {declared_arity}, but its implementation has arity {}",
                            implementation.arity()
                        ),
                        binding.location,
                    ),
                )));
            }
            let upvalues = if let Some(local) = implementation.native_type_local() {
                let (_, native_type) = native_types.get(&local).ok_or_else(|| {
                    ModuleError::new(format!(
                        "native symbol {symbol:?} references undeclared native type slot @{local}"
                    ))
                })?;
                vec![main.heap.native_type_value(native_type.clone())]
            } else {
                Vec::new()
            };
            let value = main.heap.native_closure(implementation, upvalues);
            let root = main
                .heap
                .persistent(value)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            external_roots.insert(symbol.to_owned(), root);
        }
        if let Some(undeclared) = implementations.keys().find(|symbol| {
            !program.value.body.value.bindings.iter().any(|binding| {
                binding.value.kind == BindingKind::Native
                    && binding.value.name.value.as_str() == symbol.as_str()
            })
        }) {
            return Err(ModuleError::new(format!(
                "native symbol {undeclared:?} for {} has no Telora declaration",
                spec.name
            )));
        }
        if let Some(prelude) = &default_prelude {
            for (name, root) in prelude {
                external_roots.entry(name.clone()).or_insert(*root);
            }
        }
        let mut account = QuotaAccount::new(Quota::new(100_000, 1_000, u64::MAX));
        let module_id = main
            .modules
            .id(&ModuleCName::builtin(&spec.name))
            .unwrap_or(ModuleId::ANONYMOUS);
        let analysis = analyze_program_with_bindings_observed(
            &source_name,
            module_id,
            &program,
            &mut account,
            &external_roots
                .iter()
                .map(|(name, root)| (name.clone(), *root))
                .collect(),
            &HashSet::new(),
            sources,
            &BTreeMap::new(),
            &external_interfaces,
            debug_sink,
            &mut main.heap,
            &mut main.types,
        )
        .map_err(|error| {
            error.diagnostic.as_ref().map_or_else(
                || ModuleError::new(error.to_string()),
                |diagnostic| ModuleError::new(sources.render(diagnostic)),
            )
        })?;
        install_type_family_roots(&mut external_roots, &analysis);
        let static_funcs = main.modules.static_funcs(module_id);
        let metadata = metadata_compilation_plan(&program);
        let promoted_types = metadata
            .as_ref()
            .map(|metadata| metadata.type_names.iter().cloned().collect())
            .unwrap_or_default();
        let erased_bindings = metadata
            .map(|metadata| metadata.erased_bindings)
            .unwrap_or_default();
        let function = compile_program_with_promoted_types_and_static_funcs(
            sources.get(source_id),
            &program,
            &analysis,
            &promoted_types,
            &erased_bindings,
            &static_funcs,
        )
        .map_err(|error| ModuleError::new(error.to_string()))?;
        let arena = Vm::new()
            .with_debug_sink(Arc::clone(debug_sink))
            .execute_in_work(
                &main.heap,
                &runtime_roots(&external_roots),
                &function,
                &[],
                &mut account,
            )
            .map_err(|error| ModuleError::new(error.with_sources(sources).to_string()))?;
        let root = arena
            .publish_module(&mut main.heap)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        if spec.name == PRELUDE_MODULE {
            crate::types::audit_default_prelude_interface(&analysis.module_interface)
                .map_err(ModuleError::new)?;
            default_prelude = Some(default_prelude_exports(
                root,
                &analysis.module_interface,
                &main.heap,
            )?);
        }
        let interface = analysis.module_interface.clone();
        if let Some(inputs) = semantic_inputs.as_deref_mut() {
            let imports = program
                .value
                .body
                .value
                .bindings
                .iter()
                .filter(|binding| {
                    matches!(
                        binding.value.kind,
                        BindingKind::Import | BindingKind::OpenImport
                    )
                })
                .filter_map(|binding| {
                    let ExprKind::String(target) = &binding.value.value.value else {
                        return None;
                    };
                    Some(SemanticImport {
                        name: if binding.value.kind == BindingKind::OpenImport {
                            "*".into()
                        } else {
                            binding.value.name.value.clone()
                        },
                        location: binding.value.name.location,
                        target: ModuleCName::builtin(target),
                        namespace: binding.value.kind != BindingKind::OpenImport
                            && binding.value.imported_name.is_none(),
                    })
                })
                .collect();
            inputs.insert(
                spec.name.clone(),
                SemanticModuleInput {
                    key: spec.name.clone(),
                    path: None,
                    kind: WorkspaceModuleKind::Core,
                    source: Some(source_id),
                    program: Some(program),
                    analysis: Some(analysis),
                    partial: None,
                    interface: None,
                    state: WorkspaceModuleState::Available,
                    imports,
                    diagnostics: Vec::new(),
                },
            );
        }
        modules.insert(
            spec.name,
            ModuleArtifact {
                root,
                interface,
                provenance: None,
            },
        );
    }
    Ok(modules)
}

fn default_prelude_exports(
    root: PersistentValue,
    interface: &ModuleInterface,
    heap: &Heap,
) -> Result<BTreeMap<String, PersistentValue>, ModuleError> {
    interface
        .exports
        .keys()
        .map(|name| {
            root.export_get(heap, name)
                .map_err(|error| ModuleError::new(error.to_string()))?
                // Prelude bindings are semantic constants. Their use site, not
                // the prelude implementation, supplies the root provenance.
                .map(|value| (name.clone(), value.without_location()))
                .ok_or_else(|| ModuleError::new(format!("core/prelude has no export {name:?}")))
        })
        .collect()
}

fn select_import_root(
    root: PersistentValue,
    interface: ModuleInterface,
    exported: Option<&crate::ast::Identifier>,
    local: &str,
    heap: &Heap,
) -> Result<(PersistentValue, ModuleInterface), ModuleError> {
    let Some(exported) = exported else {
        return Ok((root, interface));
    };
    let selected = root
        .export_get(heap, &exported.value)
        .map_err(|error| ModuleError::new(error.to_string()))?
        .ok_or_else(|| ModuleError::new(format!("module has no export {:?}", exported.value)))?;
    let scheme = interface
        .exports
        .get(&exported.value)
        .cloned()
        .ok_or_else(|| {
            ModuleError::new(format!(
                "module interface has no export {:?}",
                exported.value
            ))
        })?;
    Ok((
        selected,
        ModuleInterface {
            exports: BTreeMap::from([(local.to_owned(), scheme)]),
            concrete_types: interface.concrete_types,
            type_family_templates: interface
                .type_family_templates
                .get(&exported.value)
                .cloned()
                .map(|family| BTreeMap::from([(local.to_owned(), family)]))
                .unwrap_or_default(),
        },
    ))
}

impl fmt::Debug for ModuleRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModuleRuntime")
            .finish_non_exhaustive()
    }
}

#[allow(clippy::too_many_arguments)]
fn invoke_world_member_in(
    main: &Heap,
    externals: &HashMap<String, Val>,
    sources: &SourceDatabase,
    world: WorkWorld,
    member: &str,
    runtime_arguments: &[Val],
    retain_module: bool,
    quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
) -> Result<WorkWorld, ModuleError> {
    let argument_count = runtime_arguments.len();
    let base = argument_count + 1;
    let mut instructions = vec![Instruction::GetField {
        dst: Register(base),
        dict: Register(0),
        field: member.to_owned(),
    }];
    instructions.extend((0..argument_count).map(|index| Instruction::Move {
        dst: Register(base + 1 + index),
        src: Register(1 + index),
    }));
    instructions.push(Instruction::Call {
        base: Register(base),
        argument_count,
    });
    let result = if retain_module {
        instructions.push(Instruction::MakeTuple {
            dst: Register(base + 1),
            items: vec![Register(0), Register(base)],
        });
        Register(base + 1)
    } else {
        Register(base)
    };
    instructions.push(Instruction::Return { src: result });
    let wrapper = BytecodeFunction::with_signature(
        format!("<invoke module export {member}>"),
        argument_count + 1,
        0,
        (base + argument_count + 2).max(result.0 + 1),
        Vec::new(),
        instructions,
    );
    let mut account = QuotaAccount::new(quota);
    Vm::new()
        .with_debug_sink(debug_sink)
        .execute_in_existing_world_with_runtime_args(
            main,
            externals,
            &wrapper,
            world,
            runtime_arguments,
            &[],
            &mut account,
        )
        .map_err(|error| ModuleError::new(error.with_sources(sources).to_string()))
}

#[allow(clippy::too_many_arguments)]
fn prepare_and_initialize_entry_in(
    main: &Heap,
    externals: &HashMap<String, Val>,
    sources: &SourceDatabase,
    world: WorkWorld,
    provider: Val,
    caps: Val,
    value_owner: Val,
    prepared_data: Val,
    initializer: Val,
    main_argument: Val,
    quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
) -> Result<WorkWorld, ModuleError> {
    let wrapper = BytecodeFunction::with_signature(
        "<prepare resources and invoke Entry initializer>",
        7,
        0,
        14,
        Vec::new(),
        vec![
            Instruction::Move {
                dst: Register(7),
                src: Register(1),
            },
            Instruction::Move {
                dst: Register(8),
                src: Register(2),
            },
            Instruction::Move {
                dst: Register(9),
                src: Register(3),
            },
            Instruction::Move {
                dst: Register(10),
                src: Register(4),
            },
            Instruction::Call {
                base: Register(7),
                argument_count: 3,
            },
            Instruction::Move {
                dst: Register(11),
                src: Register(5),
            },
            Instruction::Move {
                dst: Register(12),
                src: Register(7),
            },
            Instruction::Move {
                dst: Register(13),
                src: Register(6),
            },
            Instruction::Call {
                base: Register(11),
                argument_count: 2,
            },
            Instruction::Return { src: Register(11) },
        ],
    );
    let arguments = [
        provider,
        caps,
        value_owner,
        prepared_data,
        initializer,
        main_argument,
    ];
    let mut account = QuotaAccount::new(quota);
    Vm::new()
        .with_debug_sink(debug_sink)
        .execute_in_existing_world_with_runtime_args(
            main,
            externals,
            &wrapper,
            world,
            &arguments,
            &[],
            &mut account,
        )
        .map_err(|error| ModuleError::new(error.with_sources(sources).to_string()))
}

impl LoadedModule {
    pub const fn uses_explicit_exports(&self) -> bool {
        self.analysis.explicit_exports
    }

    pub fn options(&self, key: &str) -> impl Iterator<Item = crate::ValueRef<'_>> {
        self.options
            .iter()
            .filter(move |option| option.key == key)
            .map(|option| option.value.value())
    }

    pub fn option_actions(&self) -> &[LoadedOptionAction] {
        &self.options
    }

    pub fn execute(
        &self,
        evaluation_fuel: usize,
    ) -> Result<crate::ExecutionWorld, crate::RuntimeError> {
        self.execute_with_quota(Quota::with_fuel(evaluation_fuel))
    }

    pub fn execute_with_quota(
        &self,
        quota: Quota,
    ) -> Result<crate::ExecutionWorld, crate::RuntimeError> {
        self.execute_with_quota_and_debug_sink(quota, Arc::new(DiscardDebugSink))
    }

    pub fn execute_with_quota_and_debug_sink(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<crate::ExecutionWorld, crate::RuntimeError> {
        self.execute_observed(quota, debug_sink).0
    }

    fn check_with_quota_and_debug_sink(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<(), crate::RuntimeError> {
        self.check_observed(quota, debug_sink).0
    }

    fn check_observed(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> (Result<(), crate::RuntimeError>, Vec<Diagnostic>) {
        let mut account = QuotaAccount::new(quota);
        let result = Vm::new()
            .with_debug_sink(debug_sink)
            .execute_in_work(
                &self.runtime.main.heap,
                &self.runtime.externals,
                &self.function,
                &[],
                &mut account,
            )
            .map(|_| ())
            .map_err(|error| error.with_sources(&self.sources));
        (result, account.take_diagnostics())
    }

    fn execute_observed(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> (
        Result<crate::ExecutionWorld, crate::RuntimeError>,
        Vec<Diagnostic>,
    ) {
        let (result, diagnostics) = self.execute_raw_world_observed(quota, debug_sink);
        let result = result
            .map(|world| crate::ExecutionWorld::new(Arc::clone(&self.runtime.main.heap), world));
        (result, diagnostics)
    }

    fn execute_world_observed(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> (Result<WorkWorld, crate::RuntimeError>, Vec<Diagnostic>) {
        let (result, diagnostics) = self.execute_raw_world_observed(quota, debug_sink);
        let result = result.and_then(|world| {
            if self.analysis.explicit_exports {
                world
                    .seal_module()
                    .map_err(|error| crate::RuntimeError::from_heap_error(&self.function, error))
            } else {
                Ok(world)
            }
        });
        (result, diagnostics)
    }

    fn execute_raw_world_observed(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> (Result<WorkWorld, crate::RuntimeError>, Vec<Diagnostic>) {
        let mut account = QuotaAccount::new(quota);
        let result = Vm::new()
            .with_debug_sink(debug_sink)
            .execute_in_work(
                &self.runtime.main.heap,
                &self.runtime.externals,
                &self.function,
                &[],
                &mut account,
            )
            .map_err(|error| error.with_sources(&self.sources));
        (result, account.take_diagnostics())
    }

    fn invoke_reducer_in_work(
        &self,
        reducer: Val,
        state: WorkWorld,
        event: Val,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<WorkWorld, ModuleError> {
        let wrapper = BytecodeFunction::with_signature(
            "<invoke Entry reducer>",
            3,
            0,
            6,
            Vec::new(),
            vec![
                Instruction::Move {
                    dst: Register(3),
                    src: Register(1),
                },
                Instruction::Move {
                    dst: Register(4),
                    src: Register(0),
                },
                Instruction::Move {
                    dst: Register(5),
                    src: Register(2),
                },
                Instruction::Call {
                    base: Register(3),
                    argument_count: 2,
                },
                Instruction::Return { src: Register(3) },
            ],
        );
        let mut account = QuotaAccount::new(quota);
        let result = Vm::new()
            .with_debug_sink(debug_sink)
            .execute_in_existing_world_with_runtime_args(
                &self.runtime.main.heap,
                &self.runtime.externals,
                &wrapper,
                state,
                &[reducer, event],
                &[],
                &mut account,
            );
        result.map_err(|error| ModuleError::new(error.with_sources(&self.sources).to_string()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    pub module_quota: Quota,
    pub session_quota: Quota,
    pub data_limits: DataLimits,
}

/// Admission limits applied independently to each static or Entry data source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataLimits {
    /// Maximum raw source bytes read before parsing.
    pub file_size: usize,
    /// Maximum logical Value occurrences after alias and merge expansion.
    pub nodes: usize,
    /// Maximum logical graph depth, with the root at depth one.
    pub depth: usize,
    /// Maximum element or field count of any one Array or Object.
    pub container_size: usize,
    /// Maximum decoded byte length of any one Bytes value.
    pub bytes_len: usize,
    /// Maximum decoded UTF-8 byte length of any String, object key, or temporal value.
    pub string_len: usize,
    /// Maximum total decoded bytes in Strings, object keys, temporal values, and Bytes.
    pub payloads_bytes: usize,
}

impl Default for DataLimits {
    fn default() -> Self {
        Self {
            file_size: 256 * 1024 * 1024,
            nodes: 1_000_000,
            depth: 256,
            container_size: 1_000_000,
            bytes_len: 64 * 1024 * 1024,
            string_len: 64 * 1024 * 1024,
            payloads_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildStdinMode {
    Piped,
    Inherit,
    Null,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildOutputMode {
    PipedLine,
    PipedToEnd,
    Inherit,
    Null,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildOptions {
    pub bin: String,
    pub cwd: Option<String>,
    pub envs: BTreeMap<String, Option<String>>,
    pub clear_env: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChildStdio {
    pub stdin: ChildStdinMode,
    pub stdout: ChildOutputMode,
    pub stderr: ChildOutputMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnStdioChild {
    pub key: String,
    pub opts: ChildOptions,
    pub stdio: ChildStdio,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildText {
    pub key: String,
    pub data: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildSpawnResult {
    pub key: String,
    pub result: Result<i64, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChildExit {
    Code(i64),
    Signal(Option<i64>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemDataFormat {
    Json,
    Yaml,
    Toml,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemDataSource {
    pub src: String,
    pub format: SystemDataFormat,
    pub has_default: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemTextSource {
    pub src: String,
    pub default: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemStdin {
    Text,
    Lined,
    Null,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemCaps {
    pub data_sources: BTreeMap<String, SystemDataSource>,
    pub spawn_child: bool,
    pub text_sources: BTreeMap<String, SystemTextSource>,
    pub vars: Vec<String>,
    pub stdin: SystemStdin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemEvent {
    StdinLine(Option<String>),
    ChildStdout(ChildText),
    ChildStderr(ChildText),
    ChildSpawnResult(ChildSpawnResult),
    ChildExited { key: String, exited: ChildExit },
}

pub type RunHostFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;

pub trait RunHost {
    fn resources_provider(&mut self) -> crate::NativeFunction;

    fn configure(&mut self, caps: SystemCaps) -> RunHostFuture<'_, Result<(), String>>;

    /// Reads a configured data source without decoding it into a Telora value.
    /// The runtime registers and materializes the returned source directly in
    /// the Entry WorkWorld.
    fn read_data_source(
        &mut self,
        source: &SystemDataSource,
        max_bytes: usize,
    ) -> RunHostFuture<'_, Result<Option<String>, String>>;

    fn spawn_stdio_child(
        &mut self,
        child: SpawnStdioChild,
    ) -> RunHostFuture<'_, Result<(), String>>;

    fn post_stdin(&mut self, text: ChildText) -> RunHostFuture<'_, Result<(), String>>;

    fn next_event(&mut self) -> RunHostFuture<'_, Result<Option<SystemEvent>, String>>;

    fn finish(&mut self) -> RunHostFuture<'_, Result<(), String>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunTermination {
    Exit(i64),
    Exec(ChildOptions),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunOutcome {
    pub output: String,
    pub termination: RunTermination,
}

struct NoProcessRunHost;

fn empty_system_resources(
    context: &mut crate::CallContext<'_, '_>,
) -> Result<(), crate::NativeError> {
    let data = context.scratch()?;
    let texts = context.scratch()?;
    let vars = context.scratch()?;
    let stdin = context.scratch()?;
    context.make_dict(data, &[])?;
    context.make_dict(texts, &[])?;
    context.make_dict(vars, &[])?;
    context.set_none(stdin)?;
    context.make_dict(
        context.result(),
        &[
            ("data".into(), data),
            ("texts".into(), texts),
            ("vars".into(), vars),
            ("stdin".into(), stdin),
        ],
    )
}

impl RunHost for NoProcessRunHost {
    fn resources_provider(&mut self) -> crate::NativeFunction {
        crate::NativeFunction::new(
            "host.prepare_system_resources.empty",
            3,
            empty_system_resources,
        )
    }

    fn configure(&mut self, caps: SystemCaps) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if !caps.data_sources.is_empty()
                || !caps.text_sources.is_empty()
                || !caps.vars.is_empty()
                || caps.stdin != SystemStdin::Null
                || caps.spawn_child
            {
                return Err("this Host does not provide initialization capabilities".into());
            }
            Ok(())
        })
    }

    fn read_data_source(
        &mut self,
        _source: &SystemDataSource,
        _max_bytes: usize,
    ) -> RunHostFuture<'_, Result<Option<String>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn spawn_stdio_child(
        &mut self,
        _child: SpawnStdioChild,
    ) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async { Err("this Host does not provide stdio child processes".into()) })
    }

    fn post_stdin(&mut self, _text: ChildText) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async { Err("this Host does not provide stdio child processes".into()) })
    }

    fn next_event(&mut self) -> RunHostFuture<'_, Result<Option<SystemEvent>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn finish(&mut self) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

pub struct Engine {
    config: EngineConfig,
    debug_sink: Arc<dyn DebugSink>,
    native_modules: Arc<[RegisteredNativeModule]>,
}

pub struct NativeModuleSpec {
    name: String,
    source: String,
    functions: Vec<(String, crate::NativeFunction)>,
}

impl NativeModuleSpec {
    pub fn new(
        name: impl Into<String>,
        source: impl Into<String>,
        functions: Vec<(&'static str, crate::NativeFunction)>,
    ) -> Self {
        Self {
            name: name.into(),
            source: source.into(),
            functions: functions
                .into_iter()
                .map(|(name, function)| (name.to_owned(), function))
                .collect(),
        }
    }
}

#[derive(Clone)]
struct RegisteredNativeModule {
    id: u32,
    name: String,
    source: String,
    functions: Vec<(String, crate::NativeFunction)>,
}

pub struct EngineBuilder {
    config: EngineConfig,
    modules: BTreeMap<u32, RegisteredNativeModule>,
    names: HashSet<String>,
}

impl EngineBuilder {
    fn new(config: EngineConfig) -> Self {
        Self {
            config,
            modules: BTreeMap::new(),
            names: HashSet::new(),
        }
    }

    pub fn register_native_module(
        &mut self,
        id: Option<u32>,
        spec: NativeModuleSpec,
    ) -> Result<u32, ModuleError> {
        let valid_name = spec.name.split('/').count() >= 2
            && spec
                .name
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
            && !spec.name.starts_with(['.', '@', '/']);
        if !valid_name {
            return Err(ModuleError::new(
                "Host native module name must be an absolute module path such as acme/runtime",
            ));
        }
        if module_specs().iter().any(|core| core.name == spec.name) {
            return Err(ModuleError::new(format!(
                "built-in module name {:?} is already registered by Telora",
                spec.name
            )));
        }
        if self.names.contains(&spec.name) {
            return Err(ModuleError::new(format!(
                "Host native module name {:?} is already registered",
                spec.name
            )));
        }
        let id = match id {
            Some(id) if id <= crate::value::RESERVED_NATIVE_MODULE_MAX => {
                return Err(ModuleError::new(format!(
                    "Host native module ID {id} is in Telora's reserved range"
                )));
            }
            Some(id) => id,
            None => (crate::value::RESERVED_NATIVE_MODULE_MAX + 1..=u32::MAX)
                .find(|candidate| !self.modules.contains_key(candidate))
                .ok_or_else(|| ModuleError::new("Host native module ID space is exhausted"))?,
        };
        if self.modules.contains_key(&id) {
            return Err(ModuleError::new(format!(
                "Host native module ID {id} is already registered"
            )));
        }
        let module = RegisteredNativeModule {
            id,
            name: spec.name.clone(),
            source: spec.source,
            functions: spec.functions,
        };
        self.names.insert(spec.name);
        self.modules.insert(id, module);
        Ok(id)
    }

    pub fn build(self) -> Engine {
        Engine {
            config: self.config,
            debug_sink: Arc::new(DiscardDebugSink),
            native_modules: self.modules.into_values().collect(),
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
        load_module_with_native_modules(
            path,
            external_bindings,
            self.config.module_quota,
            self.config.data_limits,
            Arc::clone(&self.debug_sink),
            &self.native_modules,
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
            .with_builtins(builtin_list(&self.native_modules));
        load_module_with_resolver(
            resolver,
            external_bindings,
            self.config.module_quota,
            self.config.data_limits,
            Arc::clone(&self.debug_sink),
            &self.native_modules,
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
            .with_builtins(builtin_list(&self.native_modules));
        load_module_with_resolver(
            resolver,
            external_bindings,
            self.config.module_quota,
            self.config.data_limits,
            Arc::clone(&self.debug_sink),
            &self.native_modules,
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
        let source_id = sources.add(physical.display().to_string(), read(physical)?);
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
                native_modules: Arc::clone(&self.native_modules),
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
        let result = self
            .run_pending_with_host_inner(pending, entry_selector, entry_args, host)
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
        host: &mut dyn RunHost,
    ) -> Result<RunOutcome, ModuleError> {
        let resolver = pending.inner.resolver.clone();
        let (entry_id, entry_source) = if entry_selector == DEFAULT_ENTRY_MODULE {
            (
                ModuleCName::builtin("std/entry/default.entry.telora"),
                default_entry_source().to_owned(),
            )
        } else {
            let entry = resolver
                .resolve_entry(entry_selector)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            let path = entry.path().map(Path::to_owned).ok_or_else(|| {
                ModuleError::new(format!("Entry {entry_selector:?} has no physical source"))
            })?;
            (entry.id, read(&path)?)
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
            &self.native_modules,
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
            semantic_value_contract(&loader.core_modules, &loader.main.heap)?;
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
        let env = make_entry_env(entry_world.heap_mut(), &loader.main.heap, entry_args);
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
        if !matches!(main_type, TypeDescriptor::Dyn) {
            if !crate::types::assignable(
                &crate::types::erase_declared_identity(&actual_main_type),
                &crate::types::erase_declared_identity(&main_type),
            ) {
                return Err(ModuleError::new(format!(
                    "Main export record {} is not assignable to Entry.MainType {}",
                    actual_main_type.display_name(),
                    main_type.display_name()
                )));
            }
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
                    main.path.display().to_string(),
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
            .with_builtins(builtin_list(&self.native_modules));
        let root_module = resolver
            .resolve_root(path.as_ref())
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let opaque_modules = builtin_list(&self.native_modules)
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
        let core_modules = install_native_modules(
            &mut main,
            &mut sources,
            &self.debug_sink,
            &self.native_modules,
        )?;
        let mut builder = WorkspaceBuilder {
            engine: self,
            resolver,
            overlays: &BTreeMap::new(),
            query: None,
            sources,
            main,
            core_modules,
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
            .with_builtins(builtin_list(&self.native_modules));
        self.recover_with_resolver(resolver)
    }

    pub fn module_catalog(
        &self,
        cwd: impl AsRef<Path>,
    ) -> Result<Vec<ModuleCatalogEntry>, ModuleError> {
        ModuleResolver::catalog_from_cwd(cwd.as_ref(), builtin_list(&self.native_modules))
            .map_err(|error| ModuleError::new(error.to_string()))
    }

    pub fn recover_builtin_workspace(
        &self,
        module_id: &str,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        if !builtin_list(&self.native_modules)
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
            &self.native_modules,
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
            .with_builtins(builtin_list(&self.native_modules));
        self.recover_with_resolver(resolver)
    }

    fn recover_with_resolver(
        &self,
        resolver: ModuleResolver,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        let root_module = resolver
            .selected_root()
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let opaque_modules = builtin_list(&self.native_modules)
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
        let core_modules = install_native_modules(
            &mut main,
            &mut sources,
            &self.debug_sink,
            &self.native_modules,
        )?;
        let mut builder = WorkspaceBuilder {
            engine: self,
            resolver,
            overlays: &BTreeMap::new(),
            query: None,
            sources,
            main,
            core_modules,
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
            .with_builtins(builtin_list(&self.native_modules));
        let root_module = resolver
            .resolve_root(path.as_ref())
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let opaque_modules = builtin_list(&self.native_modules)
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
        let core_modules = install_native_modules(
            &mut main,
            &mut sources,
            &self.debug_sink,
            &self.native_modules,
        )?;
        let mut builder = WorkspaceBuilder {
            engine: self,
            resolver,
            overlays,
            query: Some(context),
            sources,
            main,
            core_modules,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModuleSourcePolicy {
    ExplicitExports,
    ExpressionHarness,
}

#[derive(Clone, Copy)]
enum TeloraModuleSource<'a> {
    File(&'a Path),
    Synthetic {
        name: &'a str,
        context_path: &'a Path,
        source: &'a str,
    },
}

impl<'a> TeloraModuleSource<'a> {
    fn context_path(self) -> &'a Path {
        match self {
            Self::File(path)
            | Self::Synthetic {
                context_path: path, ..
            } => path,
        }
    }
}

fn load_module_with_native_modules(
    path: impl AsRef<Path>,
    external_bindings: BTreeMap<String, crate::DataWorld>,
    module_quota: Quota,
    data_limits: DataLimits,
    debug_sink: Arc<dyn DebugSink>,
    native_modules: &[RegisteredNativeModule],
    source_policy: ModuleSourcePolicy,
) -> Result<LoadedModule, ModuleError> {
    let resolver = ModuleResolver::for_root(path.as_ref())
        .map_err(|error| ModuleError::new(error.to_string()))?
        .with_builtins(builtin_list(native_modules));
    load_module_with_resolver(
        resolver,
        external_bindings,
        module_quota,
        data_limits,
        debug_sink,
        native_modules,
        source_policy,
    )
}

fn load_module_with_resolver(
    resolver: ModuleResolver,
    external_bindings: BTreeMap<String, crate::DataWorld>,
    module_quota: Quota,
    data_limits: DataLimits,
    debug_sink: Arc<dyn DebugSink>,
    native_modules: &[RegisteredNativeModule],
    source_policy: ModuleSourcePolicy,
) -> Result<LoadedModule, ModuleError> {
    let root_module = resolver
        .selected_root()
        .map_err(|error| ModuleError::new(error.to_string()))?;
    if root_module.format != ModuleFormat::Telora {
        return Err(ModuleError::new(
            "root module must have a .telora extension",
        ));
    }
    let opaque_modules = builtin_list(native_modules)
        .into_iter()
        .map(|(name, _)| ModuleCName::builtin(name));
    let graph = ModuleGraph::discover(
        &resolver,
        vec![root_module.clone()],
        &BTreeMap::new(),
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
        source_policy,
    };
    loader.load_root(root_module, external_bindings)
}

fn protocol_ref(mut value: crate::ValueRef<'_>) -> crate::ValueRef<'_> {
    while let Some((_, payload)) = value.declared_value_parts() {
        value = payload;
    }
    value
}

fn expect_protocol_record_ref<'a>(
    value: crate::ValueRef<'a>,
    path: &str,
    fields: &[&str],
) -> Result<crate::ValueRef<'a>, ModuleError> {
    let value = protocol_ref(value);
    let actual = value
        .dict_fields()
        .ok_or_else(|| ModuleError::new(format!("{path} must be a record")))?;
    if !actual.iter().copied().eq(fields.iter().copied()) {
        return Err(ModuleError::new(format!(
            "{path} has an invalid field shape"
        )));
    }
    Ok(value)
}

fn protocol_string_ref(value: crate::ValueRef<'_>, path: &str) -> Result<String, ModuleError> {
    protocol_ref(value)
        .as_str()
        .map(|value| value.as_str().to_owned())
        .ok_or_else(|| ModuleError::new(format!("{path} must be String")))
}

fn protocol_bool_ref(value: crate::ValueRef<'_>, path: &str) -> Result<bool, ModuleError> {
    match protocol_ref(value).as_atom().as_deref() {
        Some("True") => Ok(true),
        Some("False") => Ok(false),
        _ => Err(ModuleError::new(format!("{path} must be Bool"))),
    }
}

fn protocol_option_string_ref(
    value: crate::ValueRef<'_>,
    path: &str,
) -> Result<Option<String>, ModuleError> {
    let value = protocol_ref(value);
    if value.as_atom().as_deref() == Some("None") {
        return Ok(None);
    }
    let (tag, payload) = value
        .tagged_parts()
        .ok_or_else(|| ModuleError::new(format!("{path} must be Option(String)")))?;
    if tag.as_atom().as_deref() != Some("Some") {
        return Err(ModuleError::new(format!("{path} must be Option(String)")));
    }
    protocol_string_ref(payload, path).map(Some)
}

fn parse_child_options_ref(
    value: crate::ValueRef<'_>,
    path: &str,
) -> Result<ChildOptions, ModuleError> {
    let opts = expect_protocol_record_ref(value, path, &["bin", "clear_env", "cwd", "envs"])?;
    let envs = protocol_ref(opts.get("envs").expect("field shape checked"));
    let names = envs
        .dict_fields()
        .ok_or_else(|| ModuleError::new(format!("{path}.envs must be a Dict")))?;
    let envs = names
        .into_iter()
        .map(|name| {
            protocol_option_string_ref(envs.get(name).unwrap(), &format!("{path}.envs.{name}"))
                .map(|value| (name.to_owned(), value))
        })
        .collect::<Result<_, _>>()?;
    Ok(ChildOptions {
        bin: protocol_string_ref(opts.get("bin").unwrap(), &format!("{path}.bin"))?,
        cwd: protocol_option_string_ref(opts.get("cwd").unwrap(), &format!("{path}.cwd"))?,
        envs,
        clear_env: protocol_bool_ref(opts.get("clear_env").unwrap(), &format!("{path}.clear_env"))?,
    })
}

fn parse_stdin_mode_ref(value: crate::ValueRef<'_>) -> Result<ChildStdinMode, ModuleError> {
    match protocol_ref(value).as_atom().as_deref() {
        Some("Piped") => Ok(ChildStdinMode::Piped),
        Some("Inherit") => Ok(ChildStdinMode::Inherit),
        Some("Null") => Ok(ChildStdinMode::Null),
        _ => Err(ModuleError::new("SpawnStdioChild.stdio.stdin is invalid")),
    }
}

fn parse_output_mode_ref(
    value: crate::ValueRef<'_>,
    path: &str,
) -> Result<ChildOutputMode, ModuleError> {
    match protocol_ref(value).as_atom().as_deref() {
        Some("PipedLine") => Ok(ChildOutputMode::PipedLine),
        Some("PipedToEnd") => Ok(ChildOutputMode::PipedToEnd),
        Some("Inherit") => Ok(ChildOutputMode::Inherit),
        Some("Null") => Ok(ChildOutputMode::Null),
        _ => Err(ModuleError::new(format!("{path} is invalid"))),
    }
}

fn parse_spawn_stdio_child_ref(value: crate::ValueRef<'_>) -> Result<SpawnStdioChild, ModuleError> {
    let child = expect_protocol_record_ref(value, "SpawnStdioChild", &["key", "opts", "stdio"])?;
    let stdio = expect_protocol_record_ref(
        child.get("stdio").unwrap(),
        "SpawnStdioChild.stdio",
        &["stderr", "stdin", "stdout"],
    )?;
    Ok(SpawnStdioChild {
        key: protocol_string_ref(child.get("key").unwrap(), "SpawnStdioChild.key")?,
        opts: parse_child_options_ref(child.get("opts").unwrap(), "SpawnStdioChild.opts")?,
        stdio: ChildStdio {
            stdin: parse_stdin_mode_ref(stdio.get("stdin").unwrap())?,
            stdout: parse_output_mode_ref(
                stdio.get("stdout").unwrap(),
                "SpawnStdioChild.stdio.stdout",
            )?,
            stderr: parse_output_mode_ref(
                stdio.get("stderr").unwrap(),
                "SpawnStdioChild.stdio.stderr",
            )?,
        },
    })
}

fn parse_child_text_ref(value: crate::ValueRef<'_>, path: &str) -> Result<ChildText, ModuleError> {
    let text = expect_protocol_record_ref(value, path, &["data", "key"])?;
    Ok(ChildText {
        key: protocol_string_ref(text.get("key").unwrap(), &format!("{path}.key"))?,
        data: protocol_option_string_ref(text.get("data").unwrap(), &format!("{path}.data"))?,
    })
}

fn runtime_record(heap: &mut Heap, fields: Vec<(&str, Val)>) -> Val {
    let mut fields = fields;
    fields.sort_by(|left, right| left.0.cmp(right.0));
    let names = fields
        .iter()
        .map(|(name, _)| heap.intern(name))
        .collect::<Vec<_>>();
    let values = fields
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let shape = heap.intern_shape(names);
    Val::unknown(DecodedValue::Dict(heap.allocate(Object::Dict {
        shape,
        values: values.into_boxed_slice(),
    })))
}

fn runtime_string(heap: &mut Heap, main: &Heap, value: &str) -> Val {
    Val::unknown(heap.string(Some(main), value))
}

fn runtime_atom(heap: &mut Heap, main: &Heap, value: &str) -> Val {
    Val::unknown(heap.atom(Some(main), value))
}

fn runtime_tagged(heap: &mut Heap, tag: Val, payload: Val) -> Val {
    Val::unknown(DecodedValue::Tagged(
        heap.allocate(Object::Tagged { tag, payload }),
    ))
}

fn runtime_option_string(heap: &mut Heap, main: &Heap, value: Option<String>) -> Val {
    match value {
        None => Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
        Some(value) => {
            let tag = Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Some));
            let payload = runtime_string(heap, main, &value);
            runtime_tagged(heap, tag, payload)
        }
    }
}

fn runtime_child_text(heap: &mut Heap, main: &Heap, text: ChildText) -> Val {
    let key = runtime_string(heap, main, &text.key);
    let data = runtime_option_string(heap, main, text.data);
    runtime_record(heap, vec![("key", key), ("data", data)])
}

fn runtime_system_event(
    heap: &mut Heap,
    main: &Heap,
    event: Option<SystemEvent>,
) -> Result<Val, ModuleError> {
    let Some(event) = event else {
        return Ok(runtime_atom(heap, main, "Initialize"));
    };
    let (tag, payload) = match event {
        SystemEvent::StdinLine(line) => {
            let line = match line {
                Some(line) => {
                    let line = runtime_string(heap, main, &line);
                    runtime_tagged(
                        heap,
                        Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Some)),
                        line,
                    )
                }
                None => Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
            };
            ("StdinLine", line)
        }
        SystemEvent::ChildStdout(text) => ("ChildStdout", runtime_child_text(heap, main, text)),
        SystemEvent::ChildStderr(text) => ("ChildStderr", runtime_child_text(heap, main, text)),
        SystemEvent::ChildSpawnResult(result) => {
            let key = runtime_string(heap, main, &result.key);
            let (tag, payload) = match result.result {
                Ok(pid) => (
                    Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Ok)),
                    Val::unknown(DecodedValue::Int(pid)),
                ),
                Err(error) => (
                    Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Err)),
                    runtime_string(heap, main, &error),
                ),
            };
            let result = runtime_tagged(heap, tag, payload);
            (
                "ChildSpawnResult",
                runtime_record(heap, vec![("key", key), ("result", result)]),
            )
        }
        SystemEvent::ChildExited { key, exited } => {
            let key = runtime_string(heap, main, &key);
            let exited = match exited {
                ChildExit::Code(code) => runtime_tagged(
                    heap,
                    Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Ok)),
                    Val::unknown(DecodedValue::Int(code)),
                ),
                ChildExit::Signal(signal) => {
                    let payload = match signal {
                        None => Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
                        Some(signal) => runtime_tagged(
                            heap,
                            Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Some)),
                            Val::unknown(DecodedValue::Int(signal)),
                        ),
                    };
                    runtime_tagged(
                        heap,
                        Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::Err)),
                        payload,
                    )
                }
            };
            (
                "ChildExited",
                runtime_record(heap, vec![("key", key), ("exited", exited)]),
            )
        }
    };
    let tag = runtime_atom(heap, main, tag);
    Ok(runtime_tagged(heap, tag, payload))
}

fn validate_entry_interface(
    interface: &ModuleInterface,
    main_type: &TypeDescriptor,
    state_type: &TypeDescriptor,
    value_type: &TypeDescriptor,
) -> Result<(), ModuleError> {
    let unit_enum = |names: &[&str]| {
        TypeDescriptor::Enum(
            names
                .iter()
                .map(|name| ((*name).to_owned(), None))
                .collect(),
        )
    };
    let bool_type = unit_enum(&["False", "True"]);
    let option_string = TypeDescriptor::Enum(BTreeMap::from([
        ("None".into(), None),
        ("Some".into(), Some(Box::new(TypeDescriptor::String))),
    ]));
    let option_action = TypeDescriptor::Struct(BTreeMap::from([
        ("key".into(), TypeDescriptor::String),
        ("value".into(), TypeDescriptor::Dyn),
    ]));
    let options_type = TypeDescriptor::Array(Box::new(option_action));
    let platform_type = TypeDescriptor::Struct(BTreeMap::from([
        ("arch".into(), TypeDescriptor::String),
        ("os".into(), TypeDescriptor::String),
    ]));
    let env_type = TypeDescriptor::Struct(BTreeMap::from([
        (
            "args".into(),
            TypeDescriptor::Array(Box::new(TypeDescriptor::String)),
        ),
        ("platform".into(), platform_type),
    ]));
    let data_format = unit_enum(&["Json", "Toml", "Yaml"]);
    let data_source = TypeDescriptor::Struct(BTreeMap::from([
        (
            "default".into(),
            TypeDescriptor::Enum(BTreeMap::from([
                ("None".into(), None),
                ("Some".into(), Some(Box::new(value_type.clone()))),
            ])),
        ),
        ("fmt".into(), data_format),
        ("src".into(), TypeDescriptor::String),
    ]));
    let text_source = TypeDescriptor::Struct(BTreeMap::from([
        ("default".into(), option_string.clone()),
        ("src".into(), TypeDescriptor::String),
    ]));
    let stdin = unit_enum(&["Lined", "Null", "Text"]);
    let caps_type = TypeDescriptor::Struct(BTreeMap::from([
        (
            "data_srcs".into(),
            TypeDescriptor::Dict(Box::new(data_source)),
        ),
        ("spawn_child".into(), bool_type.clone()),
        ("stdin".into(), stdin),
        (
            "text_srcs".into(),
            TypeDescriptor::Dict(Box::new(text_source)),
        ),
        (
            "vars".into(),
            TypeDescriptor::Array(Box::new(TypeDescriptor::String)),
        ),
    ]));
    let source_item = |data| {
        TypeDescriptor::Struct(BTreeMap::from([
            ("data".into(), data),
            ("src".into(), TypeDescriptor::String),
        ]))
    };
    let resources_type = TypeDescriptor::Struct(BTreeMap::from([
        (
            "data".into(),
            TypeDescriptor::Dict(Box::new(source_item(value_type.clone()))),
        ),
        ("stdin".into(), option_string.clone()),
        (
            "texts".into(),
            TypeDescriptor::Dict(Box::new(source_item(TypeDescriptor::String))),
        ),
        (
            "vars".into(),
            TypeDescriptor::Dict(Box::new(TypeDescriptor::String)),
        ),
    ]));
    let child_text = TypeDescriptor::Struct(BTreeMap::from([
        ("data".into(), option_string.clone()),
        ("key".into(), TypeDescriptor::String),
    ]));
    let child_opts = TypeDescriptor::Struct(BTreeMap::from([
        ("bin".into(), TypeDescriptor::String),
        ("clear_env".into(), unit_enum(&["False", "True"])),
        (
            "cwd".into(),
            TypeDescriptor::Enum(BTreeMap::from([
                ("None".into(), None),
                ("Some".into(), Some(Box::new(TypeDescriptor::String))),
            ])),
        ),
        (
            "envs".into(),
            TypeDescriptor::Dict(Box::new(option_string.clone())),
        ),
    ]));
    let stdin_type = unit_enum(&["Inherit", "Null", "Piped"]);
    let stdout_type = unit_enum(&["Inherit", "Null", "PipedLine", "PipedToEnd"]);
    let stdio_type = TypeDescriptor::Struct(BTreeMap::from([
        ("stderr".into(), stdout_type.clone()),
        ("stdin".into(), stdin_type),
        ("stdout".into(), stdout_type),
    ]));
    let spawn_child = TypeDescriptor::Struct(BTreeMap::from([
        ("key".into(), TypeDescriptor::String),
        ("opts".into(), child_opts.clone()),
        ("stdio".into(), stdio_type),
    ]));
    let child_spawn_result = TypeDescriptor::Struct(BTreeMap::from([
        ("key".into(), TypeDescriptor::String),
        (
            "result".into(),
            TypeDescriptor::Enum(BTreeMap::from([
                ("Ok".into(), Some(Box::new(TypeDescriptor::Int))),
                ("Err".into(), Some(Box::new(TypeDescriptor::String))),
            ])),
        ),
    ]));
    let child_exited = TypeDescriptor::Struct(BTreeMap::from([
        (
            "exited".into(),
            TypeDescriptor::Enum(BTreeMap::from([
                ("Ok".into(), Some(Box::new(TypeDescriptor::Int))),
                (
                    "Err".into(),
                    Some(Box::new(TypeDescriptor::Enum(BTreeMap::from([
                        ("None".into(), None),
                        ("Some".into(), Some(Box::new(TypeDescriptor::Int))),
                    ])))),
                ),
            ])),
        ),
        ("key".into(), TypeDescriptor::String),
    ]));
    let event_type = TypeDescriptor::Enum(BTreeMap::from([
        ("Initialize".into(), None),
        ("StdinLine".into(), Some(Box::new(option_string.clone()))),
        ("ChildStdout".into(), Some(Box::new(child_text.clone()))),
        ("ChildStderr".into(), Some(Box::new(child_text.clone()))),
        (
            "ChildSpawnResult".into(),
            Some(Box::new(child_spawn_result)),
        ),
        ("ChildExited".into(), Some(Box::new(child_exited))),
    ]));
    let effect_type = TypeDescriptor::Enum(BTreeMap::from([
        ("Exec".into(), Some(Box::new(child_opts))),
        ("Exit".into(), Some(Box::new(TypeDescriptor::Int))),
        ("Output".into(), Some(Box::new(TypeDescriptor::String))),
        ("PostStdin".into(), Some(Box::new(child_text))),
        ("SpawnStdioChild".into(), Some(Box::new(spawn_child))),
    ]));
    let transition_type = TypeDescriptor::Tuple(vec![
        state_type.clone(),
        TypeDescriptor::Array(Box::new(effect_type)),
    ]);
    let reducer_type = TypeDescriptor::Function {
        parameters: vec![state_type.clone(), event_type],
        result: Box::new(transition_type),
    };
    let initializer_type = TypeDescriptor::Function {
        parameters: vec![resources_type, main_type.clone()],
        result: Box::new(TypeDescriptor::Tuple(vec![
            state_type.clone(),
            reducer_type,
        ])),
    };
    let expected = BTreeMap::from([
        (
            "MainType",
            TypeDescriptor::TypeOf(Box::new(main_type.clone())),
        ),
        (
            "State",
            TypeDescriptor::TypeOf(Box::new(state_type.clone())),
        ),
        (
            "config",
            TypeDescriptor::Function {
                parameters: vec![options_type, env_type],
                result: Box::new(TypeDescriptor::Tuple(vec![caps_type, initializer_type])),
            },
        ),
    ]);
    for (name, expected) in expected {
        let scheme = interface
            .exports
            .get(name)
            .ok_or_else(|| ModuleError::new(format!("Entry interface omitted {name}")))?;
        let actual = crate::types::erase_declared_identity(&scheme.body);
        let expected = crate::types::erase_declared_identity(&expected);
        if !scheme.parameters.is_empty()
            || !crate::types::assignable(&actual, &expected)
            || !crate::types::assignable(&expected, &actual)
        {
            return Err(ModuleError::new(format!(
                "Entry.{name} has type {}, expected {}",
                scheme.body.display_name(),
                expected.display_name()
            )));
        }
    }
    Ok(())
}

fn runtime_array(heap: &mut Heap, values: Vec<Val>) -> Val {
    Val::unknown(DecodedValue::Array(
        heap.allocate(Object::Array(values.into_boxed_slice())),
    ))
}

fn runtime_dyn(
    heap: &mut Heap,
    main: &Heap,
    value: Val,
    origin: impl Into<Arc<str>>,
) -> Result<Val, ModuleError> {
    let descriptor = crate::types::infer_value_ref(crate::ValueRef::work(value, heap, main));
    let descriptor_value = heap
        .type_descriptor_value(Some(main), &descriptor)
        .map_err(|error| ModuleError::new(error.to_string()))?;
    Ok(
        value.with_value(DecodedValue::Dyn(heap.allocate(Object::Dyn {
            identity: Arc::new(()),
            descriptor: descriptor_value,
            value,
            scheme: Some(crate::TypeScheme {
                parameters: Vec::new(),
                body: descriptor,
            }),
            origin: Some(origin.into()),
        }))),
    )
}

fn make_system_options(
    heap: &mut Heap,
    main: &Heap,
    options: &[LoadedOptionAction],
) -> Result<Val, ModuleError> {
    let options = options
        .iter()
        .map(|option| {
            let value = option
                .value
                .relocate_into(heap, main)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            let value = runtime_dyn(heap, main, value, format!("option {:?}", option.key))?;
            let key = runtime_string(heap, main, &option.key);
            Ok(runtime_record(heap, vec![("key", key), ("value", value)]))
        })
        .collect::<Result<Vec<_>, ModuleError>>()?;
    Ok(runtime_array(heap, options))
}

fn make_entry_env(heap: &mut Heap, main: &Heap, arguments: &[String]) -> Val {
    let arguments = arguments
        .iter()
        .map(|argument| runtime_string(heap, main, argument))
        .collect();
    let arguments = runtime_array(heap, arguments);
    let arch = runtime_string(heap, main, std::env::consts::ARCH);
    let os = runtime_string(heap, main, std::env::consts::OS);
    let platform = runtime_record(heap, vec![("arch", arch), ("os", os)]);
    runtime_record(heap, vec![("args", arguments), ("platform", platform)])
}

fn parse_system_caps(value: crate::ValueRef<'_>) -> Result<SystemCaps, ModuleError> {
    fn dict<'a>(
        value: crate::ValueRef<'a>,
        path: &str,
    ) -> Result<(crate::ValueRef<'a>, Vec<&'a str>), ModuleError> {
        let value = protocol_ref(value);
        let fields = value
            .dict_fields()
            .ok_or_else(|| ModuleError::new(format!("{path} must be a Dict")))?;
        Ok((value, fields))
    }

    let caps = expect_protocol_record_ref(
        value,
        "Entry.config SystemCaps",
        &["data_srcs", "spawn_child", "stdin", "text_srcs", "vars"],
    )?;
    let (data, data_keys) = dict(caps.get("data_srcs").unwrap(), "SystemCaps.data_srcs")?;
    let data_sources = data_keys
        .into_iter()
        .map(|key| {
            let path = format!("SystemCaps.data_srcs.{key}");
            let value = expect_protocol_record_ref(
                data.get(key).unwrap(),
                &path,
                &["default", "fmt", "src"],
            )?;
            let format = match protocol_ref(value.get("fmt").unwrap()).as_atom().as_deref() {
                Some("Json") => SystemDataFormat::Json,
                Some("Yaml") => SystemDataFormat::Yaml,
                Some("Toml") => SystemDataFormat::Toml,
                _ => return Err(ModuleError::new(format!("{path}.fmt is invalid"))),
            };
            let src = protocol_string_ref(value.get("src").unwrap(), &format!("{path}.src"))?;
            let default = protocol_ref(value.get("default").unwrap());
            let has_default = if default.as_atom().as_deref() == Some("None") {
                false
            } else {
                let (tag, _) = default.tagged_parts().ok_or_else(|| {
                    ModuleError::new(format!("{path}.default must be Option(Value)"))
                })?;
                if tag.as_atom().as_deref() != Some("Some") {
                    return Err(ModuleError::new(format!(
                        "{path}.default must be Option(Value)"
                    )));
                }
                true
            };
            if key.is_empty() || src.is_empty() {
                return Err(ModuleError::new(format!(
                    "{path} must use non-empty key and src"
                )));
            }
            Ok((
                key.to_owned(),
                SystemDataSource {
                    src,
                    format,
                    has_default,
                },
            ))
        })
        .collect::<Result<_, _>>()?;

    let (texts, text_keys) = dict(caps.get("text_srcs").unwrap(), "SystemCaps.text_srcs")?;
    let text_sources = text_keys
        .into_iter()
        .map(|key| {
            let path = format!("SystemCaps.text_srcs.{key}");
            let value =
                expect_protocol_record_ref(texts.get(key).unwrap(), &path, &["default", "src"])?;
            let src = protocol_string_ref(value.get("src").unwrap(), &format!("{path}.src"))?;
            let default = protocol_option_string_ref(
                value.get("default").unwrap(),
                &format!("{path}.default"),
            )?;
            if key.is_empty() || src.is_empty() {
                return Err(ModuleError::new(
                    "SystemCaps.text_srcs must use non-empty keys and paths",
                ));
            }
            Ok((key.to_owned(), SystemTextSource { src, default }))
        })
        .collect::<Result<_, _>>()?;

    let vars = protocol_ref(caps.get("vars").unwrap());
    let length = vars
        .sequence_len()
        .ok_or_else(|| ModuleError::new("SystemCaps.vars must be Array(String)"))?;
    let mut names = Vec::with_capacity(length);
    let mut unique = BTreeSet::new();
    for index in 0..length {
        let name = protocol_string_ref(
            vars.sequence_get(index).expect("index is in range"),
            &format!("SystemCaps.vars[{index}]"),
        )?;
        if name.is_empty() || !unique.insert(name.clone()) {
            return Err(ModuleError::new(
                "SystemCaps.vars must contain unique non-empty names",
            ));
        }
        names.push(name);
    }
    let stdin = match protocol_ref(caps.get("stdin").unwrap())
        .as_atom()
        .as_deref()
    {
        Some("Text") => SystemStdin::Text,
        Some("Lined") => SystemStdin::Lined,
        Some("Null") => SystemStdin::Null,
        _ => return Err(ModuleError::new("SystemCaps.stdin is invalid")),
    };
    let spawn_child =
        protocol_bool_ref(caps.get("spawn_child").unwrap(), "SystemCaps.spawn_child")?;
    Ok(SystemCaps {
        data_sources,
        spawn_child,
        text_sources,
        vars: names,
        stdin,
    })
}

fn concrete_module_descriptor(interface: &ModuleInterface) -> Result<TypeDescriptor, ModuleError> {
    let mut fields = BTreeMap::new();
    for (name, scheme) in &interface.exports {
        if !scheme.parameters.is_empty() {
            return Err(ModuleError::new(format!(
                "Main export {name:?} is generic and has no concrete Entry boundary type"
            )));
        }
        fields.insert(name.clone(), scheme.body.clone());
    }
    Ok(TypeDescriptor::Struct(fields))
}

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

#[cfg(test)]
#[path = "module/tests/mod.rs"]
mod tests;
