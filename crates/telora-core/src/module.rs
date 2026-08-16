use crate::ast::{BindingKind, Expr, ExprKind, Program, StringPartKind, TypeArgumentKind};
use crate::compiler::{
    collect_runtime_names, compile_expression_with_bindings, compile_metadata_initializer,
    compile_program_analyzed_in, compile_program_with_promoted_types, function_contract_arity,
    type_link_key,
};
use crate::core::{
    EDGE_RUNTIME_MODULE, PRELUDE_MODULE, edge_runtime_source, module_specs, run_entry_source,
};
use crate::heap::{Heap, PersistentValue, publish_root, publish_value};
use crate::json::{Provenance, SourcedValue, parse_json_registered};
use crate::module_id::{
    ModuleAuthority, ModuleFormat, ModuleId, ModuleResolver, ResolvedModule, immediate_value,
};
use crate::parser::parse_registered;
use crate::semantic::{
    SemanticImport, SemanticModuleInput, WorkspaceModuleKind, WorkspaceModuleState,
    WorkspaceSnapshot,
};
use crate::source::{Diagnostic, SourceDatabase};
use crate::toml::parse_toml_registered;
use crate::types::{
    Analysis, ModuleInterface, PartialAnalysisControl, TypeDescriptor, TypeFamilyTemplate,
    analyze_partial_types_recovered_with_query, analyze_program_with_bindings_observed,
    program_references_name, recovered_reference_locations,
};
use crate::vm::WorkWorld;
use crate::yaml::parse_yaml_registered;
use crate::{
    Atom, BuiltinAtom, BytecodeFunction, Closure, DebugSink, DiscardDebugSink, Prototype, Quota,
    QuotaAccount, Value, Vm,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct StaticDataParse {
    value: Option<SourcedValue>,
    diagnostics: Vec<Diagnostic>,
    kind: WorkspaceModuleKind,
}

#[derive(Clone)]
struct OpenImportCandidate {
    provider: ModuleId,
    value: Value,
    root: PersistentValue,
    scheme: crate::types::TypeScheme,
    provenance: Option<Provenance>,
    opaque: bool,
    concrete_types: BTreeMap<String, TypeDescriptor>,
    type_family_template: Option<TypeFamilyTemplate>,
}

#[derive(Clone)]
struct RecoveryOpenImportCandidate {
    provider: ModuleId,
    value: Value,
    scheme: crate::types::TypeScheme,
    sourced: Option<SourcedValue>,
    root: PersistentValue,
    concrete_types: BTreeMap<String, TypeDescriptor>,
    type_family_template: Option<TypeFamilyTemplate>,
}

fn recovery_open_import_exports(
    provider: &ModuleId,
    value: &Value,
    interface: &ModuleInterface,
    sourced: Option<&SourcedValue>,
    root: PersistentValue,
    heap: &Heap,
) -> Result<Vec<(String, RecoveryOpenImportCandidate)>, ModuleError> {
    let Value::Dict(exports) = value else {
        return Err(ModuleError::new(format!(
            "cannot open non-module value {provider}"
        )));
    };
    interface
        .exports
        .iter()
        .map(|(name, scheme)| {
            let value = exports.get(name).cloned().ok_or_else(|| {
                ModuleError::new(format!(
                    "module {provider} has no value for export {name:?}"
                ))
            })?;
            let field_root = root
                .dict_get(heap, name)
                .map_err(|error| ModuleError::new(error.to_string()))?
                .ok_or_else(|| {
                    ModuleError::new(format!("module {provider} has no root for export {name:?}"))
                })?;
            Ok((
                name.clone(),
                RecoveryOpenImportCandidate {
                    provider: provider.clone(),
                    value,
                    scheme: scheme.clone(),
                    sourced: sourced.cloned(),
                    root: field_root,
                    concrete_types: interface.concrete_types.clone(),
                    type_family_template: interface.type_family_templates.get(name).cloned(),
                },
            ))
        })
        .collect()
}

fn open_import_exports(
    provider: &ModuleId,
    value: &Value,
    root: PersistentValue,
    interface: &ModuleInterface,
    heap: &Heap,
    provenance: Option<&Provenance>,
    opaque: bool,
) -> Result<Vec<(String, OpenImportCandidate)>, ModuleError> {
    let Value::Dict(exports) = value else {
        return Err(ModuleError::new(format!(
            "cannot open non-module value {provider}"
        )));
    };
    interface
        .exports
        .iter()
        .map(|(name, scheme)| {
            let value = exports.get(name).cloned().ok_or_else(|| {
                ModuleError::new(format!(
                    "module {provider} has no value for export {name:?}"
                ))
            })?;
            let root = root
                .dict_get(heap, name)
                .map_err(|error| ModuleError::new(error.to_string()))?
                .ok_or_else(|| {
                    ModuleError::new(format!("module {provider} has no root for export {name:?}"))
                })?;
            Ok((
                name.clone(),
                OpenImportCandidate {
                    provider: provider.clone(),
                    value,
                    root,
                    scheme: scheme.clone(),
                    provenance: provenance.cloned(),
                    opaque,
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
    let (value, diagnostics) = match format {
        ModuleFormat::Json => {
            let parsed = parse_json_registered(sources, source_id);
            (parsed.value, parsed.diagnostics)
        }
        ModuleFormat::Toml => {
            let parsed = parse_toml_registered(sources, source_id);
            (parsed.value, parsed.diagnostics)
        }
        ModuleFormat::Yaml => {
            let parsed = parse_yaml_registered(sources, source_id);
            (parsed.value, parsed.diagnostics)
        }
        _ => unreachable!("kind exists only for static data formats"),
    };
    Some(StaticDataParse {
        value,
        diagnostics,
        kind,
    })
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
    pub value: Value,
}

#[derive(Clone)]
pub struct PendingModule {
    inner: Arc<PendingModuleInner>,
}

struct PendingModuleInner {
    path: PathBuf,
    resolver: ModuleResolver,
    config: EngineConfig,
    debug_sink: Arc<dyn DebugSink>,
    native_modules: Arc<[RegisteredNativeModule]>,
    state: Mutex<PendingModuleState>,
}

enum PendingModuleState {
    Pending { bindings: BTreeMap<String, Value> },
    Initializing,
    Ready(InstantiatedModule),
    Failed(ModuleError),
}

#[derive(Clone, Debug)]
struct InjectedValueModule {
    value: Value,
    interface: ModuleInterface,
}

#[derive(Clone, Debug)]
pub struct InstantiatedModule {
    module: Arc<LoadedModule>,
    exports: Value,
}

impl PendingModule {
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    fn bind_external(&self, name: String, value: Value) -> Result<(), ModuleError> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("pending module state poisoned");
        let PendingModuleState::Pending { bindings, .. } = &mut *state else {
            return Err(ModuleError::new(
                "cannot bind an external after initialization has started",
            ));
        };
        if bindings.insert(name.clone(), value).is_some() {
            return Err(ModuleError::new(format!(
                "external binding {name:?} is already installed"
            )));
        }
        Ok(())
    }

    pub fn initialize(&self) -> Result<InstantiatedModule, ModuleError> {
        let bindings = {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("pending module state poisoned");
            match &mut *state {
                PendingModuleState::Pending { bindings } => {
                    let bindings = std::mem::take(bindings);
                    *state = PendingModuleState::Initializing;
                    bindings
                }
                PendingModuleState::Initializing => {
                    return Err(ModuleError::new(
                        "module initialization is already in progress",
                    ));
                }
                PendingModuleState::Ready(module) => return Ok(module.clone()),
                PendingModuleState::Failed(error) => return Err(error.clone()),
            }
        };
        let resolver = self
            .inner
            .resolver
            .clone()
            .with_builtins(builtin_list(&self.inner.native_modules));
        let result = load_module_with_resolver(
            resolver,
            bindings,
            BTreeMap::new(),
            self.inner.config.module_quota,
            Arc::clone(&self.inner.debug_sink),
            &self.inner.native_modules,
            ModuleSourcePolicy::ExplicitExports,
        )
        .and_then(|module| {
            let exports = module
                .execute_with_quota_and_debug_sink(
                    self.inner.config.session_quota,
                    Arc::clone(&self.inner.debug_sink),
                )
                .map_err(|error| ModuleError::new(error.to_string()))?;
            Ok(InstantiatedModule {
                module: Arc::new(module),
                exports,
            })
        });
        let mut state = self
            .inner
            .state
            .lock()
            .expect("pending module state poisoned");
        *state = match &result {
            Ok(module) => PendingModuleState::Ready(module.clone()),
            Err(error) => PendingModuleState::Failed(error.clone()),
        };
        result
    }
}

impl InstantiatedModule {
    pub fn module(&self) -> &LoadedModule {
        &self.module
    }

    pub fn export(&self, name: &str) -> Option<(&Value, &crate::TypeScheme)> {
        let Value::Dict(exports) = &self.exports else {
            return None;
        };
        exports
            .get(name)
            .zip(self.module.analysis.module_interface.exports.get(name))
    }
}

struct CompiledTeloraModule {
    analysis: Analysis,
    function: BytecodeFunction,
    externals: HashMap<String, PersistentValue>,
    options: Vec<LoadedOptionAction>,
}

struct ModuleRuntime {
    main: FrozenMainWorld,
    externals: HashMap<String, PersistentValue>,
}

struct MainWorld {
    heap: Heap,
}

impl MainWorld {
    fn building() -> Self {
        Self { heap: Heap::main() }
    }

    fn seal(self) -> FrozenMainWorld {
        FrozenMainWorld { heap: self.heap }
    }
}

struct FrozenMainWorld {
    heap: Heap,
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
) -> Result<HashMap<String, (Value, PersistentValue, ModuleInterface)>, ModuleError> {
    install_native_modules_observed(main, sources, debug_sink, host_modules, None)
}

fn install_native_modules_observed(
    main: &mut MainWorld,
    sources: &mut SourceDatabase,
    debug_sink: &Arc<dyn DebugSink>,
    host_modules: &[RegisteredNativeModule],
    mut semantic_inputs: Option<&mut BTreeMap<String, SemanticModuleInput>>,
) -> Result<HashMap<String, (Value, PersistentValue, ModuleInterface)>, ModuleError> {
    let mut modules: HashMap<String, (Value, PersistentValue, ModuleInterface)> = HashMap::new();
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
    let mut default_prelude = None;
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
        let mut external_values = BTreeMap::new();
        let mut external_roots = HashMap::new();
        let mut external_interfaces = BTreeMap::new();
        for binding in &program.value.body.value.bindings {
            if binding.value.kind != BindingKind::Import {
                continue;
            }
            let ExprKind::String(request) = &binding.value.value.value else {
                return Err(ModuleError::new("built-in import path must be a String"));
            };
            let (module_value, module_root, module_interface) =
                modules.get(request.as_str()).ok_or_else(|| {
                    ModuleError::new(sources.render(&Diagnostic::error(
                        format!(
                            "built-in module {} imports unavailable earlier built-in {request:?}",
                            spec.name
                        ),
                        binding.value.value.location,
                    )))
                })?;
            let (value, root, interface) =
                if let Some(imported_name) = binding.value.imported_name.as_ref() {
                    let (value, interface) = select_import_value(
                        module_value.clone(),
                        module_interface.clone(),
                        Some(imported_name),
                        &binding.value.name.value,
                    )?;
                    let root = module_root
                        .dict_get(&main.heap, &imported_name.value)
                        .map_err(|error| ModuleError::new(error.to_string()))?
                        .ok_or_else(|| {
                            ModuleError::new(format!(
                                "built-in module {request:?} has no root for export {:?}",
                                imported_name.value
                            ))
                        })?;
                    (value, root, interface)
                } else {
                    (module_value.clone(), *module_root, module_interface.clone())
                };
            external_values.insert(binding.value.name.value.clone(), value);
            external_roots.insert(binding.value.name.value.clone(), root);
            external_interfaces.insert(binding.value.name.value.clone(), interface);
        }
        let native_module = crate::value::NativeModuleId(spec.id);
        let native_types = declared_native_types(&program, native_module, &spec.name, sources)?;
        for (name, native_type) in native_types.values() {
            let value = Value::NativeType(native_type.clone());
            let root = publish_value(&mut main.heap, &value)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            external_values.insert(name.clone(), value);
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
            let value = if let Some(local) = implementation.native_type_local() {
                let (_, native_type) = native_types.get(&local).ok_or_else(|| {
                    ModuleError::new(format!(
                        "native symbol {symbol:?} references undeclared native type slot @{local}"
                    ))
                })?;
                Value::Func(Arc::new(Closure::native_with_upvalues(
                    implementation,
                    vec![Value::NativeType(native_type.clone())],
                )))
            } else {
                Value::Func(Arc::new(Closure::native(implementation)))
            };
            let root = publish_value(&mut main.heap, &value)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            external_values.insert(symbol.to_owned(), value);
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
        let mut account = QuotaAccount::new(Quota::new(100_000, 1_000, u64::MAX));
        let mut analysis = analyze_program_with_bindings_observed(
            &source_name,
            &program,
            &mut account,
            &external_values,
            &HashSet::new(),
            sources,
            &BTreeMap::new(),
            &external_interfaces,
            debug_sink,
        )
        .map_err(|error| {
            error.diagnostic.as_ref().map_or_else(
                || ModuleError::new(error.to_string()),
                |diagnostic| ModuleError::new(sources.render(diagnostic)),
            )
        })?;
        if let Some(exports) = &default_prelude {
            project_default_prelude(&mut analysis, exports);
        }
        let function = compile_program_analyzed_in(sources.get(source_id), &program, &analysis)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let arena = Vm::new()
            .with_debug_sink(Arc::clone(debug_sink))
            .execute_in_work(&main.heap, &external_roots, &function, &[], &mut account)
            .map_err(|error| ModuleError::new(error.with_sources(sources).to_string()))?;
        let value = arena
            .export(&main.heap)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let root = arena
            .publish(&mut main.heap)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        if spec.name == PRELUDE_MODULE {
            crate::types::audit_default_prelude_interface(&analysis.module_interface)
                .map_err(ModuleError::new)?;
            default_prelude = Some(default_prelude_exports(&value, &analysis.module_interface)?);
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
                        target: ModuleId::builtin(target),
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
                    state: WorkspaceModuleState::Known,
                    imports,
                    diagnostics: Vec::new(),
                },
            );
        }
        modules.insert(spec.name, (value, root, interface));
    }
    Ok(modules)
}

fn default_prelude_exports(
    value: &Value,
    interface: &ModuleInterface,
) -> Result<BTreeMap<String, Value>, ModuleError> {
    let Value::Dict(exports) = value else {
        return Err(ModuleError::new("core/prelude must export a Dict"));
    };
    let values = exports
        .shape()
        .fields()
        .iter()
        .cloned()
        .zip(exports.values().iter().cloned())
        .collect::<BTreeMap<_, _>>();
    let value_names = values.keys().collect::<BTreeSet<_>>();
    let interface_names = interface.exports.keys().collect::<BTreeSet<_>>();
    if value_names != interface_names {
        return Err(ModuleError::new(
            "core/prelude value and interface exports differ",
        ));
    }
    Ok(values)
}

fn project_default_prelude(analysis: &mut Analysis, exports: &BTreeMap<String, Value>) {
    for (name, value) in exports {
        analysis.prelude.insert(name.clone(), value.clone());
    }
}

fn select_import_value(
    value: Value,
    interface: ModuleInterface,
    exported: Option<&crate::ast::Identifier>,
    local: &str,
) -> Result<(Value, ModuleInterface), ModuleError> {
    let Some(exported) = exported else {
        return Ok((value, interface));
    };
    let Value::Dict(module) = value else {
        return Err(ModuleError::new(format!(
            "cannot selectively import {:?} from a non-module value",
            exported.value
        )));
    };
    let selected = module
        .get(&exported.value)
        .cloned()
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

fn project_module_value(
    root: PersistentValue,
    interface: &ModuleInterface,
    heap: &Heap,
) -> Result<Value, ModuleError> {
    let mut vm = Vm::new();
    let recursive_projection = TypeDescriptor::Any.to_value(&mut vm);
    let mut fields = Vec::with_capacity(interface.exports.len());
    for (name, scheme) in &interface.exports {
        let field_root = root
            .dict_get(heap, name)
            .map_err(|error| ModuleError::new(error.to_string()))?
            .ok_or_else(|| ModuleError::new(format!("module has no root for export {name:?}")))?;
        let value = match heap
            .persistent_contains_up_link(field_root)
            .map_err(|error| ModuleError::new(error.to_string()))?
        {
            true => match &scheme.body {
                TypeDescriptor::TypeOf(_) => recursive_projection.clone(),
                _ => match heap
                    .export_persistent_projecting_up_links(field_root, &recursive_projection)
                {
                    Ok(value) => value,
                    Err(error) if error.is_legacy_cycle() => Value::none(),
                    Err(error) => return Err(ModuleError::new(error.to_string())),
                },
            },
            false => match heap.export_persistent(field_root) {
                Ok(value) => value,
                Err(error) => return Err(ModuleError::new(error.to_string())),
            },
        };
        fields.push((name.clone(), value));
    }
    vm.make_dict(fields)
        .map_err(|error| ModuleError::new(error.to_string()))
}

impl fmt::Debug for ModuleRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModuleRuntime")
            .finish_non_exhaustive()
    }
}

impl LoadedModule {
    pub const fn uses_explicit_exports(&self) -> bool {
        self.analysis.explicit_exports
    }

    pub fn options(&self, key: &str) -> impl Iterator<Item = &Value> {
        self.options
            .iter()
            .filter(move |option| option.key == key)
            .map(|option| &option.value)
    }

    pub fn option_actions(&self) -> &[LoadedOptionAction] {
        &self.options
    }

    pub fn execute(&self, evaluation_fuel: usize) -> Result<Value, crate::RuntimeError> {
        self.execute_with_quota(Quota::with_fuel(evaluation_fuel))
    }

    pub fn execute_with_quota(&self, quota: Quota) -> Result<Value, crate::RuntimeError> {
        self.execute_with_quota_and_debug_sink(quota, Arc::new(DiscardDebugSink))
    }

    pub fn execute_with_quota_and_debug_sink(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<Value, crate::RuntimeError> {
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
    ) -> (Result<Value, crate::RuntimeError>, Vec<Diagnostic>) {
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
            .map_err(|error| error.with_sources(&self.sources))
            .and_then(|arena| {
                arena
                    .export(&self.runtime.main.heap)
                    .map_err(|error| crate::RuntimeError::from_heap_error(&self.function, error))
            });
        (result, account.take_diagnostics())
    }

    pub fn invoke_with_quota_and_debug_sink(
        &self,
        callee: &Value,
        arguments: &[Value],
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<Value, ModuleError> {
        let Value::Func(closure) = callee else {
            return Err(ModuleError::new(format!(
                "module result must be a function, found {}",
                callee.type_name()
            )));
        };
        let Prototype::Bytecode(function) = closure.prototype() else {
            return Err(ModuleError::new(
                "module result must be an Telora function, found a native function",
            ));
        };
        let mut account = QuotaAccount::new(quota);
        let arena = Vm::new()
            .with_debug_sink(debug_sink)
            .execute_function_with_captures_in_work(
                &self.runtime.main.heap,
                &self.runtime.externals,
                function,
                closure.upvalues(),
                arguments,
                &mut account,
            )
            .map_err(|error| ModuleError::new(error.with_sources(&self.sources).to_string()))?;
        arena
            .export(&self.runtime.main.heap)
            .map_err(|error| ModuleError::new(error.to_string()))
    }

    fn invoke_reducer_in_work(
        &self,
        reducer: &Value,
        state: WorkWorld,
        event: Value,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<WorkWorld, ModuleError> {
        let Value::Func(closure) = reducer else {
            return Err(ModuleError::new("Entry reducer must be a function"));
        };
        let Prototype::Bytecode(function) = closure.prototype() else {
            return Err(ModuleError::new(
                "Entry reducer must be a Telora bytecode function",
            ));
        };
        let mut account = QuotaAccount::new(quota);
        let result = Vm::new()
            .with_debug_sink(debug_sink)
            .execute_function_with_captures_and_work_state_in_work(
                &self.runtime.main.heap,
                &self.runtime.externals,
                function,
                closure.upvalues(),
                state,
                &[event],
                &mut account,
            );
        result.map_err(|error| ModuleError::new(error.with_sources(&self.sources).to_string()))
    }

    fn invoke_initialize_in_work(
        &self,
        initialize: &Value,
        main: Value,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<WorkWorld, ModuleError> {
        let Value::Func(closure) = initialize else {
            return Err(ModuleError::new("Entry.initialize must be a function"));
        };
        let Prototype::Bytecode(function) = closure.prototype() else {
            return Err(ModuleError::new(
                "Entry.initialize must be a Telora bytecode function",
            ));
        };
        let mut account = QuotaAccount::new(quota);
        Vm::new()
            .with_debug_sink(debug_sink)
            .execute_function_with_captures_in_work(
                &self.runtime.main.heap,
                &self.runtime.externals,
                function,
                closure.upvalues(),
                &[main],
                &mut account,
            )
            .map_err(|error| ModuleError::new(error.with_sources(&self.sources).to_string()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    pub module_quota: Quota,
    pub session_quota: Quota,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemEvent {
    ChildStdout(ChildText),
    ChildStderr(ChildText),
    ChildSpawnResult(ChildSpawnResult),
    ChildExited { key: String, exited: ChildExit },
}

pub type RunHostFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;

pub trait RunHost {
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

impl RunHost for NoProcessRunHost {
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
        external_bindings: BTreeMap<String, Value>,
    ) -> Result<LoadedModule, ModuleError> {
        load_module_with_native_modules(
            path,
            external_bindings,
            BTreeMap::new(),
            self.config.module_quota,
            Arc::clone(&self.debug_sink),
            &self.native_modules,
            ModuleSourcePolicy::ExplicitExports,
        )
    }

    pub fn load_module_id(
        &self,
        cwd: impl AsRef<Path>,
        module_id: &str,
        external_bindings: BTreeMap<String, Value>,
    ) -> Result<LoadedModule, ModuleError> {
        let resolver = ModuleResolver::from_cwd(cwd.as_ref(), module_id)
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list(&self.native_modules));
        load_module_with_resolver(
            resolver,
            external_bindings,
            BTreeMap::new(),
            self.config.module_quota,
            Arc::clone(&self.debug_sink),
            &self.native_modules,
            ModuleSourcePolicy::ExplicitExports,
        )
    }

    pub fn load_standalone(
        &self,
        path: impl AsRef<Path>,
        external_bindings: BTreeMap<String, Value>,
    ) -> Result<LoadedModule, ModuleError> {
        let resolver = ModuleResolver::standalone(path.as_ref())
            .map_err(|error| ModuleError::new(error.to_string()))?
            .with_builtins(builtin_list(&self.native_modules));
        load_module_with_resolver(
            resolver,
            external_bindings,
            BTreeMap::new(),
            self.config.module_quota,
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
        Ok(PendingModule {
            inner: Arc::new(PendingModuleInner {
                path: physical.to_owned(),
                resolver,
                config: self.config,
                debug_sink: Arc::clone(&self.debug_sink),
                native_modules: Arc::clone(&self.native_modules),
                state: Mutex::new(PendingModuleState::Pending {
                    bindings: BTreeMap::new(),
                }),
            }),
        })
    }

    pub fn execute(&self, module: &LoadedModule) -> Result<Value, crate::RuntimeError> {
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

    fn check_observed(
        &self,
        module: &LoadedModule,
    ) -> (Result<(), crate::RuntimeError>, Vec<Diagnostic>) {
        module.check_observed(self.config.session_quota, Arc::clone(&self.debug_sink))
    }

    pub fn invoke(
        &self,
        module: &LoadedModule,
        callee: &Value,
        arguments: &[Value],
    ) -> Result<Value, ModuleError> {
        module.invoke_with_quota_and_debug_sink(
            callee,
            arguments,
            self.config.session_quota,
            Arc::clone(&self.debug_sink),
        )
    }

    pub async fn run_pending(
        &self,
        pending: PendingModule,
        input: Option<Value>,
        entry_path: Option<&Path>,
    ) -> Result<RunOutcome, ModuleError> {
        self.run_pending_with_host(pending, input, entry_path, &mut NoProcessRunHost)
            .await
    }

    pub async fn run_pending_with_host(
        &self,
        pending: PendingModule,
        input: Option<Value>,
        entry_path: Option<&Path>,
        host: &mut dyn RunHost,
    ) -> Result<RunOutcome, ModuleError> {
        let result = self
            .run_pending_with_host_inner(pending, input, entry_path, host)
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
        input: Option<Value>,
        entry_path: Option<&Path>,
        host: &mut dyn RunHost,
    ) -> Result<RunOutcome, ModuleError> {
        let resolver = pending.inner.resolver.clone();
        let (entry_id, entry_source) = match entry_path {
            Some(path) => {
                if path.extension().and_then(|extension| extension.to_str()) != Some("telora") {
                    return Err(ModuleError::new("--entry must name a .telora file"));
                }
                let path = fs::canonicalize(path).map_err(|error| {
                    ModuleError::new(format!("cannot resolve entry {}: {error}", path.display()))
                })?;
                let source = read(&path)?;
                (ModuleId::builtin("host/user-entry.telora"), source)
            }
            None => (
                ModuleId::builtin("host/run-entry.telora"),
                run_entry_source().to_owned(),
            ),
        };
        let entry = load_selected_entry(
            resolver,
            entry_id,
            &entry_source,
            self.config.module_quota,
            Arc::clone(&self.debug_sink),
            &self.native_modules,
        )?;
        let exports = self
            .execute(&entry)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let Value::Dict(exports) = exports else {
            return Err(ModuleError::new("Entry must export a module record"));
        };
        let expected = ["MainType", "State", "initialize", "prepare"];
        if exports.shape().fields() != expected {
            return Err(ModuleError::new(
                "Entry must export exactly MainType, State, initialize, and prepare",
            ));
        }
        let main_type = crate::types::decode_type(
            exports.get("MainType").expect("field shape checked"),
            "Entry.MainType",
        )
        .map_err(ModuleError::new)?;
        let state_type = crate::types::decode_type(
            exports.get("State").expect("field shape checked"),
            "Entry.State",
        )
        .map_err(ModuleError::new)?;
        validate_entry_interface(&entry, &main_type, &state_type)?;
        let prepare = exports.get("prepare").expect("field shape checked");
        require_function_arity(prepare, 1, "Entry.prepare")?;
        let options = make_system_options(input.as_ref())?;
        let caps = self
            .invoke(&entry, prepare, &[options])
            .map_err(|error| ModuleError::new(format!("Entry.prepare failed: {error}")))?;
        let wants_input = parse_system_caps(&caps)?;
        match (wants_input, input) {
            (true, Some(input)) => pending.bind_external("input".into(), input)?,
            (true, None) => {
                return Err(ModuleError::new(
                    "Entry requested input, but telora run received no --input",
                ));
            }
            (false, _) => {}
        }

        let main = pending.initialize()?;
        let actual_main_type = concrete_module_descriptor(&main)?;
        let main_argument = if matches!(main_type, TypeDescriptor::Dyn) {
            pack_dyn(
                main.exports.clone(),
                actual_main_type,
                main.module.path.display().to_string(),
            )
        } else {
            if !crate::types::assignable(&actual_main_type, &main_type) {
                return Err(ModuleError::new(format!(
                    "Main export record {} is not assignable to Entry.MainType {}",
                    actual_main_type.display_name(),
                    main_type.display_name()
                )));
            }
            main.exports.clone()
        };
        let initialize = exports.get("initialize").expect("field shape checked");
        require_function_arity(initialize, 1, "Entry.initialize")?;
        let initialized = entry
            .invoke_initialize_in_work(
                initialize,
                main_argument,
                self.config.session_quota,
                Arc::clone(&self.debug_sink),
            )
            .map_err(|error| ModuleError::new(format!("Entry.initialize failed: {error}")))?;
        let (state, reducer) = initialized
            .into_entry_initialization(&entry.runtime.main.heap)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let mut state = state;
        require_function_arity(&reducer, 2, "Entry reducer")?;
        let mut events = std::collections::VecDeque::from([Value::atom("Initialize")]);
        let mut output = String::new();
        loop {
            let event = match events.pop_front() {
                Some(event) => event,
                None => host
                    .next_event()
                    .await
                    .map_err(|error| ModuleError::new(format!("run Host failed: {error}")))?
                    .map(system_event_value)
                    .transpose()?
                    .ok_or_else(|| {
                        ModuleError::new("Entry made no progress and the Host has no pending event")
                    })?,
            };
            let transition = entry
                .invoke_reducer_in_work(
                    &reducer,
                    state,
                    event,
                    self.config.session_quota,
                    Arc::clone(&self.debug_sink),
                )
                .map_err(|error| ModuleError::new(format!("Entry reducer failed: {error}")))?;
            let (next_state, effects) = transition
                .into_reducer_transition(&entry.runtime.main.heap)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            let Value::Array(effects) = effects else {
                return Err(ModuleError::new("Entry reducer effects must be an Array"));
            };
            state = next_state;
            let mut terminal = None;
            for effect in effects.iter() {
                if terminal.is_some() {
                    return Err(ModuleError::new(
                        "Entry returned an effect after a terminal effect",
                    ));
                }
                match effect {
                    Value::Tagged { tag, payload } if tag.name() == "SpawnStdioChild" => {
                        let child = parse_spawn_stdio_child(payload)?;
                        host.spawn_stdio_child(child).await.map_err(|error| {
                            ModuleError::new(format!("cannot spawn stdio child: {error}"))
                        })?;
                    }
                    Value::Tagged { tag, payload } if tag.name() == "PostStdin" => {
                        let text = parse_child_text(payload, "PostStdin")?;
                        host.post_stdin(text).await.map_err(|error| {
                            ModuleError::new(format!("cannot post child stdin: {error}"))
                        })?;
                    }
                    Value::Tagged { tag, payload } if tag.name() == "Exec" => {
                        terminal =
                            Some(RunTermination::Exec(parse_child_options(payload, "Exec")?));
                    }
                    Value::Tagged { tag, payload } if tag.name() == "Output" => {
                        let Value::String(text) = payload.as_ref() else {
                            return Err(ModuleError::new("Output payload must be String"));
                        };
                        output.push_str(text);
                    }
                    Value::Tagged { tag, payload } if tag.name() == "Exit" => {
                        let Value::Int(code) = payload.as_ref() else {
                            return Err(ModuleError::new("Exit payload must be Int"));
                        };
                        terminal = Some(RunTermination::Exit(*code));
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
        let root = root_module
            .path()
            .expect("local root has a path")
            .to_owned();
        if let Ok(mut module) = self.load_module(&root, BTreeMap::new()) {
            let (result, diagnostics) = self.check_observed(&module);
            match result {
                Ok(_) => {
                    module.workspace.extend_diagnostics(diagnostics);
                    return Ok(module.workspace);
                }
                Err(error)
                    if error.failure_class() == crate::evaluation::FailureClass::Recoverable => {}
                Err(_) => return Ok(module.workspace),
            }
        }
        let mut main = MainWorld::building();
        let mut sources = SourceDatabase::default();
        let core_modules = install_native_modules(
            &mut main,
            &mut sources,
            &self.debug_sink,
            &self.native_modules,
        )?
        .into_iter()
        .map(|(name, (value, root, interface))| (name.to_owned(), (value, root, interface)))
        .collect();
        let mut builder = RecoverableWorkspaceBuilder {
            engine: self,
            resolver,
            overlays: &BTreeMap::new(),
            query: None,
            sources,
            main,
            core_modules,
            inputs: BTreeMap::new(),
            values: HashMap::new(),
            sourced_values: HashMap::new(),
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
        if let Ok(mut module) = load_module_with_resolver(
            resolver.clone(),
            BTreeMap::new(),
            BTreeMap::new(),
            self.config.module_quota,
            Arc::clone(&self.debug_sink),
            &self.native_modules,
            ModuleSourcePolicy::ExplicitExports,
        ) {
            let (result, diagnostics) = self.check_observed(&module);
            module.workspace.extend_diagnostics(diagnostics);
            match result {
                Ok(_) => return Ok(module.workspace),
                Err(error)
                    if error.failure_class() == crate::evaluation::FailureClass::Recoverable => {}
                Err(error) => {
                    if let Some(diagnostic) = error.diagnostic() {
                        module.workspace.extend_diagnostics([diagnostic]);
                    }
                    return Ok(module.workspace);
                }
            }
        }
        let mut main = MainWorld::building();
        let mut sources = SourceDatabase::default();
        let core_modules = install_native_modules(
            &mut main,
            &mut sources,
            &self.debug_sink,
            &self.native_modules,
        )?
        .into_iter()
        .map(|(name, (value, root, interface))| (name.to_owned(), (value, root, interface)))
        .collect();
        let mut builder = RecoverableWorkspaceBuilder {
            engine: self,
            resolver,
            overlays: &BTreeMap::new(),
            query: None,
            sources,
            main,
            core_modules,
            inputs: BTreeMap::new(),
            values: HashMap::new(),
            sourced_values: HashMap::new(),
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
        let mut main = MainWorld::building();
        let mut sources = SourceDatabase::default();
        let core_modules = install_native_modules(
            &mut main,
            &mut sources,
            &self.debug_sink,
            &self.native_modules,
        )?
        .into_iter()
        .map(|(name, (value, root, interface))| (name.to_owned(), (value, root, interface)))
        .collect();
        let mut builder = RecoverableWorkspaceBuilder {
            engine: self,
            resolver,
            overlays,
            query: Some(context),
            sources,
            main,
            core_modules,
            inputs: BTreeMap::new(),
            values: HashMap::new(),
            sourced_values: HashMap::new(),
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

struct RecoverableWorkspaceBuilder<'a> {
    engine: &'a Engine,
    resolver: ModuleResolver,
    overlays: &'a BTreeMap<PathBuf, crate::document::DocumentText>,
    query: Option<&'a crate::query::QueryContext>,
    sources: SourceDatabase,
    main: MainWorld,
    core_modules: HashMap<String, (Value, PersistentValue, ModuleInterface)>,
    inputs: BTreeMap<String, SemanticModuleInput>,
    values: HashMap<ModuleId, Value>,
    sourced_values: HashMap<ModuleId, SourcedValue>,
    roots: HashMap<ModuleId, PersistentValue>,
    interfaces: HashMap<ModuleId, ModuleInterface>,
    visiting: Vec<ModuleId>,
    cycle_members: HashSet<ModuleId>,
    cycle_reported: bool,
}

impl RecoverableWorkspaceBuilder<'_> {
    fn load_telora<'a>(
        &'a mut self,
        module: ResolvedModule,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Value>> + 'a>> {
        Box::pin(async move {
            if let Some(context) = self.query
                && context.checkpoint().await.is_err()
            {
                return None;
            }
            let path = module.path()?.to_owned();
            let authority = module.authority;
            let module_id = module.id;
            if let Some(value) = self.values.get(&module_id) {
                return Some(value.clone());
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
            let mut external_values = BTreeMap::new();
            let mut external_roots = HashMap::new();
            let mut external_sourced_values = BTreeMap::new();
            let mut external_interfaces = BTreeMap::new();
            let mut unavailable_imports = HashSet::new();
            let mut open_candidates: BTreeMap<String, Vec<RecoveryOpenImportCandidate>> =
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
                    if let Some((value, root, interface)) = self.core_modules.get(&target) {
                        if open {
                            match recovery_open_import_exports(
                                &target_module.id,
                                value,
                                interface,
                                None,
                                *root,
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
                        match select_import_value(
                            value.clone(),
                            interface.clone(),
                            imported_name.as_deref(),
                            &name,
                        ) {
                            Ok((value, interface)) => {
                                let root = if let Some(imported_name) = imported_name.as_ref() {
                                    root.dict_get(&self.main.heap, &imported_name.value)
                                        .expect("built-in module root is valid")
                                        .expect("selected built-in export has a root")
                                } else {
                                    *root
                                };
                                external_roots.insert(name.clone(), root);
                                external_values.insert(name.clone(), value);
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
                let value = match target_module.format {
                    ModuleFormat::Telora => self.load_telora(target_module.clone()).await,
                    ModuleFormat::Json | ModuleFormat::Toml | ModuleFormat::Yaml => {
                        self.load_static_data(target_module.clone()).await
                    }
                };
                if let Some(value) = value {
                    if open {
                        let sourced = self.sourced_values.get(&target_module.id);
                        let interface = self
                            .interfaces
                            .get(&target_module.id)
                            .cloned()
                            .unwrap_or_default();
                        match recovery_open_import_exports(
                            &target_module.id,
                            &value,
                            &interface,
                            sourced,
                            *self
                                .roots
                                .get(&target_module.id)
                                .expect("loaded module has a root"),
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
                    if let Some(sourced) = self.sourced_values.get(&target_module.id) {
                        external_sourced_values.insert(name.clone(), sourced.clone());
                    }
                    let interface = self
                        .interfaces
                        .get(&target_module.id)
                        .cloned()
                        .unwrap_or_default();
                    match select_import_value(value, interface, imported_name.as_deref(), &name) {
                        Ok((value, interface)) => {
                            if let Some(root) = self.roots.get(&target_module.id).copied() {
                                let root = if let Some(imported_name) = imported_name.as_ref() {
                                    root.dict_get(&self.main.heap, &imported_name.value)
                                        .expect("loaded module root is valid")
                                        .expect("selected module export has a root")
                                } else {
                                    root
                                };
                                external_roots.insert(name.clone(), root);
                            }
                            external_interfaces.insert(name.clone(), interface);
                            external_values.insert(name, value);
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
                && let Some((value, root, interface)) = self.core_modules.get(PRELUDE_MODULE)
            {
                let provider = ModuleId::Builtin(PRELUDE_MODULE.into());
                if let Ok(exports) = recovery_open_import_exports(
                    &provider,
                    value,
                    interface,
                    None,
                    *root,
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
                if explicit_names.contains(name.as_str()) || external_values.contains_key(&name) {
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
                if let Some(sourced) = candidate.sourced {
                    external_sourced_values.insert(name.clone(), sourced);
                }
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
                external_values.insert(name, candidate.value);
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
                &external_values,
                PartialAnalysisControl {
                    unavailable_imports: &unavailable_imports,
                    query: self.query,
                },
            );
            let mut runtime_diagnostics = Vec::new();
            let mut recovered_analysis = None;
            let strict = if self.cycle_members.contains(&module_id)
                || !invalid_scoped_options.is_empty()
                || missing_exports
            {
                None
            } else {
                program.as_ref().and_then(|program| {
                    match self.analyze_and_evaluate(
                        source_id,
                        program,
                        &external_values,
                        &external_roots,
                        &external_interfaces,
                    ) {
                        Ok((analysis, value, root, emitted)) => {
                            runtime_diagnostics.extend(emitted);
                            Some((analysis, value, root))
                        }
                        Err(RecoveryEvaluationError::Runtime {
                            analysis,
                            error,
                            emitted,
                        }) if error.failure_class()
                            == crate::evaluation::FailureClass::Recoverable =>
                        {
                            if emitted.is_empty() {
                                if let Some(diagnostic) = error.diagnostic() {
                                    runtime_diagnostics.push(diagnostic);
                                }
                            } else {
                                runtime_diagnostics.extend(emitted);
                            }
                            self.evaluate_independent_bindings(
                                source_id,
                                program,
                                &analysis,
                                &external_values,
                                &external_roots,
                                &mut runtime_diagnostics,
                            );
                            recovered_analysis = Some(analysis);
                            None
                        }
                        Err(_) => None,
                    }
                })
            };
            diagnostics.extend(runtime_diagnostics);
            let partial_empty =
                partial.hir.definitions().is_empty() && partial.hir.expressions().is_empty();
            let analysis = strict
                .as_ref()
                .map(|(analysis, _, _)| analysis.clone())
                .or(recovered_analysis);
            let partial = analysis.is_none().then_some(partial);
            let strict_value = strict.as_ref().map(|(_, value, _)| value);
            let state = if missing_exports {
                WorkspaceModuleState::Unavailable
            } else if strict_value.is_some() {
                WorkspaceModuleState::Known
            } else if self.cycle_members.contains(&module_id) || partial_empty {
                WorkspaceModuleState::Unavailable
            } else {
                WorkspaceModuleState::Partial
            };
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
                    state,
                    imports: semantic_imports,
                    diagnostics,
                },
            );
            if let Some((_, value, root)) = strict {
                let interface = self.inputs[&key]
                    .analysis
                    .as_ref()
                    .expect("strict module has analysis")
                    .module_interface
                    .clone();
                self.interfaces.insert(module_id.clone(), interface);
                self.roots.insert(module_id.clone(), root);
                self.values.insert(module_id, value.clone());
                Some(value)
            } else {
                None
            }
        })
    }

    async fn load_static_data(&mut self, module: ResolvedModule) -> Option<Value> {
        if let Some(context) = self.query
            && context.checkpoint().await.is_err()
        {
            return None;
        }
        let path = module.path()?.to_owned();
        let module_id = module.id;
        if let Some(value) = self.values.get(&module_id) {
            return Some(value.clone());
        }
        let key = module_id.to_string();
        if self.inputs.contains_key(&key) {
            return None;
        }
        let source = match self.overlays.get(&path).cloned() {
            Some(source) => source,
            None => match fs::read_to_string(&path) {
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
        let parsed = parse_static_data_registered(module.format, &self.sources, source_id)?;
        let sourced = parsed.value;
        let value = sourced.as_ref().map(|sourced| sourced.value.clone());
        self.inputs.insert(
            key.clone(),
            SemanticModuleInput {
                key,
                path: Some(path),
                kind: parsed.kind,
                source: Some(source_id),
                program: None,
                analysis: None,
                partial: None,
                state: if value.is_some() {
                    WorkspaceModuleState::Known
                } else {
                    WorkspaceModuleState::Unavailable
                },
                imports: Vec::new(),
                diagnostics: parsed.diagnostics,
            },
        );
        if let Some(value) = &value {
            self.values.insert(module_id.clone(), value.clone());
        }
        if let Some(sourced) = sourced {
            let mut local = Heap::work();
            let local_root = local
                .import_sourced_value(Some(&self.main.heap), &sourced)
                .expect("parsed sourced value imports into a work heap");
            let root = publish_root(&mut self.main.heap, &local, local_root)
                .expect("parsed sourced value publishes into the main heap");
            self.roots.insert(module_id.clone(), root);
            self.sourced_values.insert(module_id, sourced);
        }
        value
    }

    fn analyze_and_evaluate(
        &mut self,
        source_id: crate::SourceId,
        program: &Program,
        external_values: &BTreeMap<String, Value>,
        external_roots: &HashMap<String, PersistentValue>,
        external_interfaces: &BTreeMap<String, ModuleInterface>,
    ) -> Result<(crate::Analysis, Value, PersistentValue, Vec<Diagnostic>), RecoveryEvaluationError>
    {
        let mut account = QuotaAccount::new(self.engine.config.module_quota);
        if let Some(query) = self.query {
            account = account.with_query(query.clone());
        }
        let source = self.sources.get(source_id);
        let analysis = analyze_program_with_bindings_observed(
            &source.name,
            program,
            &mut account,
            external_values,
            &HashSet::new(),
            &self.sources,
            &BTreeMap::new(),
            external_interfaces,
            &self.engine.debug_sink,
        )
        .map_err(|_| RecoveryEvaluationError::Module)?;
        let function = compile_program_analyzed_in(source, program, &analysis)
            .map_err(|_| RecoveryEvaluationError::Module)?;
        let arena = match Vm::new()
            .with_debug_sink(Arc::clone(&self.engine.debug_sink))
            .execute_in_work(
                &self.main.heap,
                external_roots,
                &function,
                &[],
                &mut account,
            ) {
            Ok(arena) => arena,
            Err(error) => {
                return Err(RecoveryEvaluationError::Runtime {
                    analysis,
                    error: Box::new(error),
                    emitted: account.take_diagnostics(),
                });
            }
        };
        let root = arena
            .publish(&mut self.main.heap)
            .map_err(|_| RecoveryEvaluationError::Module)?;
        let value = if self
            .main
            .heap
            .persistent_contains_up_link(root)
            .map_err(|_| RecoveryEvaluationError::Module)?
        {
            project_module_value(root, &analysis.module_interface, &self.main.heap)
                .map_err(|_| RecoveryEvaluationError::Module)?
        } else {
            self.main
                .heap
                .export_persistent(root)
                .map_err(|_| RecoveryEvaluationError::Module)?
        };
        Ok((analysis, value, root, account.take_diagnostics()))
    }

    fn evaluate_independent_bindings(
        &self,
        source_id: crate::SourceId,
        program: &Program,
        analysis: &crate::Analysis,
        external_values: &BTreeMap<String, Value>,
        external_roots: &HashMap<String, PersistentValue>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let source = self.sources.get(source_id);
        let mut graph_account = QuotaAccount::new(self.engine.config.module_quota);
        if let Some(query) = self.query {
            graph_account = graph_account.with_query(query.clone());
        }
        if let Ok(function) = compile_program_analyzed_in(source, program, analysis) {
            if let Ok(execution) = Vm::new()
                .with_debug_sink(Arc::clone(&self.engine.debug_sink))
                .execute_in_work_best_effort(
                    &self.main.heap,
                    external_roots,
                    &function,
                    &[],
                    &mut graph_account,
                )
            {
                merge_runtime_diagnostics(diagnostics, graph_account.take_diagnostics());
                merge_runtime_errors(diagnostics, execution.failures);
            }
        }
        let mut values = external_values.clone();
        let mut failed = HashSet::new();
        let mut account = QuotaAccount::new(self.engine.config.module_quota);
        if let Some(query) = self.query {
            account = account.with_query(query.clone());
        }
        const MAX_RUNTIME_DIAGNOSTICS: usize = 16;
        for binding in &program.value.body.value.bindings {
            if !matches!(binding.value.kind, BindingKind::Let | BindingKind::Def) {
                continue;
            }
            if diagnostics.len() >= MAX_RUNTIME_DIAGNOSTICS {
                return;
            }
            let mut dependencies = HashSet::new();
            collect_runtime_names(&binding.value.value, &mut dependencies);
            if dependencies.iter().any(|name| failed.contains(name)) {
                failed.insert(binding.value.name.value.clone());
                continue;
            }
            let Ok(function) = compile_expression_with_bindings(
                &source.name,
                &format!("<best-effort:{}>", binding.value.name.value),
                &binding.value.value,
                &values,
                source,
            ) else {
                failed.insert(binding.value.name.value.clone());
                continue;
            };
            let (result, failures) = Vm::new()
                .with_debug_sink(Arc::clone(&self.engine.debug_sink))
                .execute_with_account_best_effort(&function, &[], &mut account);
            let emitted = account.take_diagnostics();
            merge_runtime_diagnostics(diagnostics, emitted);
            merge_runtime_errors(diagnostics, failures);
            match result {
                Ok(value) => {
                    values.insert(binding.value.name.value.clone(), value);
                }
                Err(error)
                    if error.failure_class() == crate::evaluation::FailureClass::Recoverable =>
                {
                    failed.insert(binding.value.name.value.clone());
                    if let Some(diagnostic) = error.diagnostic() {
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
                Err(_) => break,
            }
        }
    }
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

enum RecoveryEvaluationError {
    Module,
    Runtime {
        analysis: crate::Analysis,
        error: Box<crate::RuntimeError>,
        emitted: Vec<Diagnostic>,
    },
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
    external_bindings: BTreeMap<String, Value>,
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
    external_bindings: BTreeMap<String, Value>,
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
    external_bindings: BTreeMap<String, Value>,
    module_quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
) -> Result<LoadedModule, ModuleError> {
    load_module_with_native_modules(
        path,
        external_bindings,
        BTreeMap::new(),
        module_quota,
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
    external_bindings: BTreeMap<String, Value>,
    injected_modules: BTreeMap<String, InjectedValueModule>,
    module_quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
    native_modules: &[RegisteredNativeModule],
    source_policy: ModuleSourcePolicy,
) -> Result<LoadedModule, ModuleError> {
    let resolver = ModuleResolver::for_root(path.as_ref())
        .map_err(|error| ModuleError::new(error.to_string()))?
        .with_builtins(builtin_list(native_modules))
        .with_virtual_modules(injected_modules.keys().cloned());
    load_module_with_resolver(
        resolver,
        external_bindings,
        injected_modules,
        module_quota,
        debug_sink,
        native_modules,
        source_policy,
    )
}

fn load_module_with_resolver(
    resolver: ModuleResolver,
    external_bindings: BTreeMap<String, Value>,
    injected_modules: BTreeMap<String, InjectedValueModule>,
    module_quota: Quota,
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
    let mut main = MainWorld::building();
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
        debug_sink,
        sources,
        semantic_inputs: BTreeMap::new(),
        source_policy,
    };
    loader.install_injected_values(injected_modules)?;
    loader.load_root(root_module, external_bindings)
}

fn expect_protocol_record<'a>(
    value: &'a Value,
    path: &str,
    fields: &[&str],
) -> Result<&'a crate::Dict, ModuleError> {
    let Value::Dict(record) = value else {
        return Err(ModuleError::new(format!(
            "{path} must be a record, found {}",
            value.type_name()
        )));
    };
    if !record
        .shape()
        .fields()
        .iter()
        .map(String::as_str)
        .eq(fields.iter().copied())
    {
        return Err(ModuleError::new(format!(
            "{path} has an invalid field shape"
        )));
    }
    Ok(record)
}

fn protocol_string(value: &Value, path: &str) -> Result<String, ModuleError> {
    let Value::String(value) = value else {
        return Err(ModuleError::new(format!("{path} must be String")));
    };
    Ok(value.to_string())
}

fn protocol_bool(value: &Value, path: &str) -> Result<bool, ModuleError> {
    match value {
        Value::Atom(value) if value.name() == "True" => Ok(true),
        Value::Atom(value) if value.name() == "False" => Ok(false),
        _ => Err(ModuleError::new(format!("{path} must be Bool"))),
    }
}

fn protocol_option_string(value: &Value, path: &str) -> Result<Option<String>, ModuleError> {
    match value {
        Value::Atom(tag) if tag.name() == "None" => Ok(None),
        Value::Tagged { tag, payload } if tag.name() == "Some" => {
            protocol_string(payload, path).map(Some)
        }
        _ => Err(ModuleError::new(format!("{path} must be Option(String)"))),
    }
}

fn parse_child_options(value: &Value, path: &str) -> Result<ChildOptions, ModuleError> {
    let opts = expect_protocol_record(value, path, &["bin", "clear_env", "cwd", "envs"])?;
    let Value::Dict(envs) = opts.get("envs").expect("field shape checked") else {
        return Err(ModuleError::new(format!("{path}.envs must be a Dict")));
    };
    let envs = envs
        .shape()
        .fields()
        .iter()
        .zip(envs.values())
        .map(|(name, value)| {
            protocol_option_string(value, &format!("{path}.envs.{name}"))
                .map(|value| (name.clone(), value))
        })
        .collect::<Result<_, _>>()?;
    Ok(ChildOptions {
        bin: protocol_string(
            opts.get("bin").expect("field shape checked"),
            &format!("{path}.bin"),
        )?,
        cwd: protocol_option_string(
            opts.get("cwd").expect("field shape checked"),
            &format!("{path}.cwd"),
        )?,
        envs,
        clear_env: protocol_bool(
            opts.get("clear_env").expect("field shape checked"),
            &format!("{path}.clear_env"),
        )?,
    })
}

fn parse_stdin_mode(value: &Value) -> Result<ChildStdinMode, ModuleError> {
    match value {
        Value::Atom(tag) if tag.name() == "Piped" => Ok(ChildStdinMode::Piped),
        Value::Atom(tag) if tag.name() == "Inherit" => Ok(ChildStdinMode::Inherit),
        Value::Atom(tag) if tag.name() == "Null" => Ok(ChildStdinMode::Null),
        _ => Err(ModuleError::new("SpawnStdioChild.stdio.stdin is invalid")),
    }
}

fn parse_output_mode(value: &Value, path: &str) -> Result<ChildOutputMode, ModuleError> {
    match value {
        Value::Atom(tag) if tag.name() == "PipedLine" => Ok(ChildOutputMode::PipedLine),
        Value::Atom(tag) if tag.name() == "PipedToEnd" => Ok(ChildOutputMode::PipedToEnd),
        Value::Atom(tag) if tag.name() == "Inherit" => Ok(ChildOutputMode::Inherit),
        Value::Atom(tag) if tag.name() == "Null" => Ok(ChildOutputMode::Null),
        _ => Err(ModuleError::new(format!("{path} is invalid"))),
    }
}

fn parse_spawn_stdio_child(value: &Value) -> Result<SpawnStdioChild, ModuleError> {
    let child = expect_protocol_record(value, "SpawnStdioChild", &["key", "opts", "stdio"])?;
    let stdio = expect_protocol_record(
        child.get("stdio").expect("field shape checked"),
        "SpawnStdioChild.stdio",
        &["stderr", "stdin", "stdout"],
    )?;
    Ok(SpawnStdioChild {
        key: protocol_string(
            child.get("key").expect("field shape checked"),
            "SpawnStdioChild.key",
        )?,
        opts: parse_child_options(
            child.get("opts").expect("field shape checked"),
            "SpawnStdioChild.opts",
        )?,
        stdio: ChildStdio {
            stdin: parse_stdin_mode(stdio.get("stdin").expect("field shape checked"))?,
            stdout: parse_output_mode(
                stdio.get("stdout").expect("field shape checked"),
                "SpawnStdioChild.stdio.stdout",
            )?,
            stderr: parse_output_mode(
                stdio.get("stderr").expect("field shape checked"),
                "SpawnStdioChild.stdio.stderr",
            )?,
        },
    })
}

fn parse_child_text(value: &Value, path: &str) -> Result<ChildText, ModuleError> {
    let text = expect_protocol_record(value, path, &["data", "key"])?;
    Ok(ChildText {
        key: protocol_string(
            text.get("key").expect("field shape checked"),
            &format!("{path}.key"),
        )?,
        data: protocol_option_string(
            text.get("data").expect("field shape checked"),
            &format!("{path}.data"),
        )?,
    })
}

fn protocol_record(fields: Vec<(String, Value)>) -> Result<Value, ModuleError> {
    Vm::new()
        .make_dict(fields)
        .map_err(|error| ModuleError::new(error.to_string()))
}

fn option_string_value(value: Option<String>) -> Value {
    value.map_or_else(Value::none, |value| {
        Value::tagged(Atom::builtin(BuiltinAtom::Some), Value::string(value))
    })
}

fn child_text_value(text: ChildText) -> Result<Value, ModuleError> {
    protocol_record(vec![
        ("key".into(), Value::string(text.key)),
        ("data".into(), option_string_value(text.data)),
    ])
}

fn system_event_value(event: SystemEvent) -> Result<Value, ModuleError> {
    let (tag, payload) = match event {
        SystemEvent::ChildStdout(text) => ("ChildStdout", child_text_value(text)?),
        SystemEvent::ChildStderr(text) => ("ChildStderr", child_text_value(text)?),
        SystemEvent::ChildSpawnResult(result) => (
            "ChildSpawnResult",
            protocol_record(vec![
                ("key".into(), Value::string(result.key)),
                (
                    "result".into(),
                    match result.result {
                        Ok(pid) => Value::tagged(Atom::builtin(BuiltinAtom::Ok), Value::Int(pid)),
                        Err(error) => {
                            Value::tagged(Atom::builtin(BuiltinAtom::Err), Value::string(error))
                        }
                    },
                ),
            ])?,
        ),
        SystemEvent::ChildExited { key, exited } => {
            let exited = match exited {
                ChildExit::Code(code) => {
                    Value::tagged(Atom::builtin(BuiltinAtom::Ok), Value::Int(code))
                }
                ChildExit::Signal(signal) => Value::tagged(
                    Atom::builtin(BuiltinAtom::Err),
                    signal.map_or_else(Value::none, |signal| {
                        Value::tagged(Atom::builtin(BuiltinAtom::Some), Value::Int(signal))
                    }),
                ),
            };
            (
                "ChildExited",
                protocol_record(vec![
                    ("key".into(), Value::string(key)),
                    ("exited".into(), exited),
                ])?,
            )
        }
    };
    Ok(Value::tagged(Atom::named(tag), payload))
}

fn require_function_arity(value: &Value, arity: usize, name: &str) -> Result<(), ModuleError> {
    let Value::Func(function) = value else {
        return Err(ModuleError::new(format!("{name} must be a function")));
    };
    let actual = match function.prototype() {
        Prototype::Bytecode(function) => function.parameter_count(),
        Prototype::Native(function) => function.arity(),
    };
    if actual != arity {
        return Err(ModuleError::new(format!(
            "{name} must accept {arity} arguments, found {actual}"
        )));
    }
    Ok(())
}

fn validate_entry_interface(
    entry: &LoadedModule,
    main_type: &TypeDescriptor,
    state_type: &TypeDescriptor,
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
    let options_type = TypeDescriptor::Struct(BTreeMap::from([(
        "input".into(),
        TypeDescriptor::Enum(BTreeMap::from([
            ("None".into(), None),
            ("Some".into(), Some(Box::new(TypeDescriptor::Dyn))),
        ])),
    )]));
    let caps_type = TypeDescriptor::Struct(BTreeMap::from([("input".into(), bool_type)]));
    let option_string = TypeDescriptor::Enum(BTreeMap::from([
        ("None".into(), None),
        ("Some".into(), Some(Box::new(TypeDescriptor::String))),
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
            "initialize",
            TypeDescriptor::Function {
                parameters: vec![main_type.clone()],
                result: Box::new(TypeDescriptor::Tuple(vec![
                    state_type.clone(),
                    reducer_type,
                ])),
            },
        ),
        (
            "prepare",
            TypeDescriptor::Function {
                parameters: vec![options_type],
                result: Box::new(caps_type),
            },
        ),
    ]);
    for (name, expected) in expected {
        let scheme = entry
            .analysis
            .module_interface
            .exports
            .get(name)
            .ok_or_else(|| ModuleError::new(format!("Entry interface omitted {name}")))?;
        if !scheme.parameters.is_empty()
            || !crate::types::assignable(&scheme.body, &expected)
            || !crate::types::assignable(&expected, &scheme.body)
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

fn pack_dyn(value: Value, descriptor: TypeDescriptor, origin: String) -> Value {
    let mut values = Vm::new();
    let descriptor_value = descriptor.to_value(&mut values);
    Value::Dyn(
        crate::value::DynValue::from_module_export(
            descriptor_value,
            value,
            crate::TypeScheme {
                parameters: Vec::new(),
                body: descriptor,
            },
            origin,
        )
        .into(),
    )
}

fn make_system_options(input: Option<&Value>) -> Result<Value, ModuleError> {
    let input = match input {
        Some(input) => Value::tagged(
            Atom::builtin(BuiltinAtom::Some),
            pack_dyn(
                input.clone(),
                crate::types::infer_value(input),
                "telora run --input".into(),
            ),
        ),
        None => Value::none(),
    };
    Vm::new()
        .make_dict(vec![("input".into(), input)])
        .map_err(|error| ModuleError::new(error.to_string()))
}

fn parse_system_caps(value: &Value) -> Result<bool, ModuleError> {
    let Value::Dict(caps) = value else {
        return Err(ModuleError::new("Entry.prepare must return SystemCaps"));
    };
    if caps.shape().fields() != ["input"] {
        return Err(ModuleError::new(
            "Entry.prepare returned an invalid SystemCaps field shape",
        ));
    }
    match caps.get("input") {
        Some(Value::Atom(value)) if value.name() == "True" => Ok(true),
        Some(Value::Atom(value)) if value.name() == "False" => Ok(false),
        _ => Err(ModuleError::new("SystemCaps.input must be Bool")),
    }
}

fn concrete_module_descriptor(module: &InstantiatedModule) -> Result<TypeDescriptor, ModuleError> {
    let mut fields = BTreeMap::new();
    for (name, scheme) in &module.module.analysis.module_interface.exports {
        if !scheme.parameters.is_empty() {
            return Err(ModuleError::new(format!(
                "Main export {name:?} is generic and has no concrete Entry boundary type"
            )));
        }
        fields.insert(name.clone(), scheme.body.clone());
    }
    Ok(TypeDescriptor::Struct(fields))
}

fn load_selected_entry(
    resolver: ModuleResolver,
    entry_id: ModuleId,
    source: &str,
    module_quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
    native_modules: &[RegisteredNativeModule],
) -> Result<LoadedModule, ModuleError> {
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
    let mut main = MainWorld::building();
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
        debug_sink,
        sources,
        semantic_inputs: BTreeMap::new(),
        source_policy: ModuleSourcePolicy::ExplicitExports,
    };
    loader.install_injected_modules(&main_path, injected_modules)?;
    loader.load_entry(main_path, entry_id, source, BTreeMap::new())
}

struct ModuleLoader {
    resolver: ModuleResolver,
    cache: HashMap<ModuleId, ModuleState>,
    core_modules: HashMap<String, (Value, PersistentValue, ModuleInterface)>,
    main: MainWorld,
    visiting: Vec<ModuleId>,
    dependencies: BTreeSet<PathBuf>,
    module_quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
    sources: SourceDatabase,
    semantic_inputs: BTreeMap<String, SemanticModuleInput>,
    source_policy: ModuleSourcePolicy,
}

#[derive(Clone)]
enum ModuleState {
    Ready {
        root: PersistentValue,
        sourced: SourcedValue,
        opaque: bool,
        interface: ModuleInterface,
    },
}

impl ModuleLoader {
    fn install_injected_values(
        &mut self,
        modules: BTreeMap<String, InjectedValueModule>,
    ) -> Result<(), ModuleError> {
        for (name, module) in modules {
            let root = publish_value(&mut self.main.heap, &module.value)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            self.core_modules
                .insert(name, (module.value, root, module.interface));
        }
        Ok(())
    }

    fn install_injected_modules(
        &mut self,
        context_path: &Path,
        modules: BTreeMap<String, String>,
    ) -> Result<(), ModuleError> {
        for (name, source) in modules {
            let module_id = ModuleId::builtin(&name);
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
            let value = arena
                .export(&self.main.heap)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            let root = arena
                .publish(&mut self.main.heap)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            self.core_modules
                .insert(name, (value, root, compiled.analysis.module_interface));
        }
        Ok(())
    }

    fn load_entry(
        &mut self,
        main_path: PathBuf,
        module_id: ModuleId,
        entry_source: &str,
        external_bindings: BTreeMap<String, Value>,
    ) -> Result<LoadedModule, ModuleError> {
        self.enter(&module_id)?;
        let mut account = QuotaAccount::new(self.module_quota);
        let source_name = module_id.to_string();
        let result = self.compile_telora(
            &module_id,
            ModuleAuthority::RuntimeSystem,
            TeloraModuleSource::Synthetic {
                name: &source_name,
                context_path: &main_path,
                source: entry_source,
            },
            external_bindings,
            true,
            &mut account,
        );
        self.leave(&module_id);
        let CompiledTeloraModule {
            analysis,
            function,
            externals,
            options,
        } = result?;
        let workspace = WorkspaceSnapshot::build(
            self.sources.clone(),
            self.semantic_inputs.values().cloned().collect(),
        );
        let main = std::mem::replace(&mut self.main, MainWorld::building()).seal();
        Ok(LoadedModule {
            path: main_path,
            dependencies: self.dependencies.iter().cloned().collect(),
            analysis,
            function,
            sources: self.sources.clone(),
            workspace,
            options,
            runtime: Arc::new(ModuleRuntime { main, externals }),
        })
    }

    fn load_root(
        &mut self,
        module: ResolvedModule,
        external_bindings: BTreeMap<String, Value>,
    ) -> Result<LoadedModule, ModuleError> {
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
        let CompiledTeloraModule {
            analysis,
            function,
            externals,
            options,
        } = result?;
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
            runtime: Arc::new(ModuleRuntime { main, externals }),
        })
    }

    #[cfg(test)]
    fn load_value(&mut self, path: &Path) -> Result<SourcedValue, ModuleError> {
        let module = self
            .resolver
            .resolve_root(path)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        self.load_resolved_value(module)
    }

    fn load_resolved_value(&mut self, module: ResolvedModule) -> Result<SourcedValue, ModuleError> {
        let format = module.format;
        let authority = module.authority;
        let path = module
            .path()
            .expect("source module has a physical path")
            .to_owned();
        let module_id = module.id;
        if let Some(ModuleState::Ready { root, sourced, .. }) = self.cache.get(&module_id) {
            let _persistent_root = root;
            return Ok(sourced.clone());
        }
        self.enter(&module_id)?;
        self.dependencies.insert(path.clone());
        let result: Result<(SourcedValue, PersistentValue, bool, ModuleInterface), ModuleError> =
            match format {
                ModuleFormat::Json | ModuleFormat::Toml | ModuleFormat::Yaml => {
                    let source = read(&path)?;
                    let source_id = self.sources.add(path.display().to_string(), source);
                    let StaticDataParse {
                        value,
                        diagnostics,
                        kind,
                    } = parse_static_data_registered(format, &self.sources, source_id)
                        .expect("static data format has a frontend");
                    value
                        .ok_or_else(|| {
                            ModuleError::new(
                                diagnostics
                                    .iter()
                                    .map(|diagnostic| self.sources.render(diagnostic))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            )
                        })
                        .and_then(|sourced| {
                            let mut local = Heap::work();
                            let local_root = local
                                .import_sourced_value(Some(&self.main.heap), &sourced)
                                .map_err(|error| ModuleError::new(error.to_string()))?;
                            let root = publish_root(&mut self.main.heap, &local, local_root)
                                .map_err(|error| ModuleError::new(error.to_string()))?;
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
                                    state: crate::semantic::WorkspaceModuleState::Known,
                                    imports: Vec::new(),
                                    diagnostics: Vec::new(),
                                },
                            );
                            Ok((sourced, root, false, ModuleInterface::default()))
                        })
                }
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
                            .execute_in_work(
                                &self.main.heap,
                                &externals,
                                &function,
                                &[],
                                &mut account,
                            )
                            .map_err(|error| {
                                ModuleError::new(error.with_sources(&self.sources).to_string())
                            })?;
                        let root = arena
                            .publish(&mut self.main.heap)
                            .map_err(|error| ModuleError::new(error.to_string()))?;
                        let interface = analysis.module_interface.clone();
                        let contains_up_link = self
                            .main
                            .heap
                            .persistent_contains_up_link(root)
                            .map_err(|error| ModuleError::new(error.to_string()))?;
                        let (value, opaque) = if contains_up_link && analysis.explicit_exports {
                            (
                                project_module_value(root, &interface, &self.main.heap)?,
                                false,
                            )
                        } else if contains_up_link {
                            (Value::none(), true)
                        } else {
                            (
                                self.main
                                    .heap
                                    .export_persistent(root)
                                    .map_err(|error| ModuleError::new(error.to_string()))?,
                                false,
                            )
                        };
                        Ok((
                            SourcedValue {
                                value,
                                provenance: Provenance::default(),
                            },
                            root,
                            opaque,
                            interface,
                        ))
                    })
                }
            };
        self.leave(&module_id);
        let (sourced, root, opaque, interface) = result?;
        self.cache.insert(
            module_id,
            ModuleState::Ready {
                root,
                sourced: sourced.clone(),
                opaque,
                interface,
            },
        );
        Ok(sourced)
    }

    fn compile_telora(
        &mut self,
        module_id: &ModuleId,
        authority: ModuleAuthority,
        module_source: TeloraModuleSource<'_>,
        mut external_bindings: BTreeMap<String, Value>,
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
        let mut option_vm = Vm::new();
        let options = parsed
            .options
            .iter()
            .map(|option| {
                immediate_value(&option.value, &mut option_vm)
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
            let root = publish_value(&mut self.main.heap, value)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            external_roots.insert(name.clone(), root);
        }
        let mut opaque_bindings = HashSet::new();
        let mut semantic_imports = Vec::new();
        let mut external_interfaces = BTreeMap::new();
        let mut open_candidates: BTreeMap<String, Vec<OpenImportCandidate>> = BTreeMap::new();

        for binding in &program.value.body.value.bindings {
            if !matches!(
                binding.value.kind,
                BindingKind::Import | BindingKind::OpenImport
            ) {
                continue;
            }
            if binding.value.kind == BindingKind::Import
                && external_bindings.contains_key(&binding.value.name.value)
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
            if imported.authority == ModuleAuthority::RuntimeSystem {
                let (value, root, interface) =
                    self.load_native_module(relative).map_err(|error| {
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
                        &value,
                        root,
                        &interface,
                        &self.main.heap,
                        None,
                        false,
                    )? {
                        open_candidates.entry(name).or_default().push(candidate);
                    }
                    continue;
                }
                let selected_root =
                    binding
                        .value
                        .imported_name
                        .as_ref()
                        .map_or(Ok(root), |name| {
                            root.dict_get(&self.main.heap, &name.value)
                                .map_err(|error| ModuleError::new(error.to_string()))?
                                .ok_or_else(|| {
                                    ModuleError::new(format!(
                                        "module has no export {:?}",
                                        name.value
                                    ))
                                })
                        })?;
                let (value, interface) = select_import_value(
                    value,
                    interface,
                    binding.value.imported_name.as_deref(),
                    &binding.value.name.value,
                )?;
                external_roots.insert(binding.value.name.value.clone(), selected_root);
                external_interfaces.insert(binding.value.name.value.clone(), interface);
                external_bindings.insert(binding.value.name.value.clone(), value);
                continue;
            }
            let imported_id = imported.id.clone();
            let sourced = self.load_resolved_value(imported)?;
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
            let ModuleState::Ready {
                root,
                opaque,
                interface,
                ..
            } = self
                .cache
                .get(&imported_id)
                .expect("loaded module has a ready cache entry");
            if binding.value.kind == BindingKind::OpenImport {
                let provenance =
                    (!sourced.provenance.values.is_empty()).then_some(&sourced.provenance);
                for (name, candidate) in open_import_exports(
                    &imported_id,
                    &sourced.value,
                    *root,
                    interface,
                    &self.main.heap,
                    provenance,
                    *opaque,
                )? {
                    open_candidates.entry(name).or_default().push(candidate);
                }
                continue;
            }
            let selected_root = binding
                .value
                .imported_name
                .as_ref()
                .map_or(Ok(*root), |name| {
                    root.dict_get(&self.main.heap, &name.value)
                        .map_err(|error| ModuleError::new(error.to_string()))?
                        .ok_or_else(|| {
                            ModuleError::new(format!("module has no export {:?}", name.value))
                        })
                })?;
            let (selected_value, selected_interface) = select_import_value(
                sourced.value.clone(),
                interface.clone(),
                binding.value.imported_name.as_deref(),
                &binding.value.name.value,
            )?;
            external_roots.insert(binding.value.name.value.clone(), selected_root);
            external_interfaces.insert(binding.value.name.value.clone(), selected_interface);
            if *opaque {
                opaque_bindings.insert(binding.value.name.value.clone());
            }
            if !sourced.provenance.values.is_empty() {
                external_provenance.insert(binding.value.name.value.clone(), sourced.provenance);
            }
            external_bindings.insert(binding.value.name.value.clone(), selected_value);
        }
        if module_id.to_string() != PRELUDE_MODULE
            && let Some((value, root, interface)) = self.core_modules.get(PRELUDE_MODULE)
        {
            let provider = ModuleId::Builtin(PRELUDE_MODULE.into());
            for (name, candidate) in open_import_exports(
                &provider,
                value,
                *root,
                interface,
                &self.main.heap,
                None,
                false,
            )? {
                open_candidates.entry(name).or_default().push(candidate);
            }
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
            if explicit_names.contains(name.as_str()) || external_bindings.contains_key(&name) {
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
            if candidate.opaque {
                opaque_bindings.insert(name.clone());
            }
            if let Some(provenance) = candidate.provenance
                && !provenance.values.is_empty()
            {
                external_provenance.insert(name.clone(), provenance);
            }
            external_bindings.insert(name, candidate.value);
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

        let mut dynamic_bindings = opaque_bindings;
        if is_root && external_bindings.contains_key("input") {
            dynamic_bindings.insert("input".to_owned());
        }
        let has_type_bindings = program
            .value
            .body
            .value
            .bindings
            .iter()
            .any(|binding| binding.value.kind == BindingKind::Type);
        let bootstrap_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
        let mut bootstrap_account;
        let analysis_account = if has_type_bindings {
            bootstrap_account = QuotaAccount::new(account.quota());
            &mut bootstrap_account
        } else {
            &mut *account
        };
        let mut analysis = analyze_program_with_bindings_observed(
            &source_name,
            &program,
            analysis_account,
            &external_bindings,
            &dynamic_bindings,
            &self.sources,
            &external_provenance,
            &external_interfaces,
            &bootstrap_sink,
        )
        .map_err(|error| {
            error.diagnostic.as_ref().map_or_else(
                || ModuleError::new(error.to_string()),
                |diagnostic| ModuleError::new(self.sources.render(diagnostic)),
            )
        })?;
        if let Some((value, _, interface)) = self.core_modules.get(PRELUDE_MODULE) {
            let exports = default_prelude_exports(value, interface)?;
            project_default_prelude(&mut analysis, &exports);
        }
        let source_file = self.sources.get(source_id);
        let mut promoted_types = HashSet::new();
        let mut promoted_type_roots = BTreeMap::new();
        let mut erased_metadata_bindings = HashSet::new();
        if let Some(metadata) = compile_metadata_initializer(source_file, &program, &analysis)
            .map_err(|error| ModuleError::new(error.to_string()))?
        {
            erased_metadata_bindings = metadata.erased_bindings;
            let arena = Vm::new()
                .with_debug_sink(Arc::clone(&self.debug_sink))
                .execute_in_work(
                    &self.main.heap,
                    &external_roots,
                    &metadata.function,
                    &[],
                    account,
                )
                .map_err(|error| ModuleError::new(error.with_sources(&self.sources).to_string()))?;
            let metadata_root = arena
                .publish(&mut self.main.heap)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            for name in metadata.type_names {
                let root = metadata_root
                    .dict_get(&self.main.heap, &name)
                    .map_err(|error| ModuleError::new(error.to_string()))?
                    .ok_or_else(|| {
                        ModuleError::new(format!("metadata initializer omitted type root {name:?}"))
                    })?;
                external_roots.insert(type_link_key(&name), root);
                promoted_type_roots.insert(name.clone(), root);
                promoted_types.insert(name);
            }
            analysis
                .install_promoted_types(&self.main.heap, &promoted_type_roots)
                .map_err(ModuleError::new)?;
        }
        let function = if promoted_types.is_empty() {
            compile_program_analyzed_in(source_file, &program, &analysis)
        } else {
            compile_program_with_promoted_types(
                source_file,
                &program,
                &analysis,
                &promoted_types,
                &erased_metadata_bindings,
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
                state: crate::semantic::WorkspaceModuleState::Known,
                imports: semantic_imports,
                diagnostics: Vec::new(),
            },
        );
        Ok(CompiledTeloraModule {
            analysis,
            function,
            externals: external_roots,
            options,
        })
    }

    fn load_native_module(
        &mut self,
        name: &str,
    ) -> Result<(Value, PersistentValue, ModuleInterface), ModuleError> {
        self.core_modules
            .get(name)
            .map(|(value, root, interface)| (value.clone(), *root, interface.clone()))
            .ok_or_else(|| ModuleError::new(format!("unknown built-in module {name:?}")))
    }

    fn enter(&mut self, module_id: &ModuleId) -> Result<(), ModuleError> {
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

    fn leave(&mut self, module_id: &ModuleId) {
        let popped = self.visiting.pop();
        debug_assert_eq!(popped.as_ref(), Some(module_id));
    }
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

#[cfg(test)]
fn canonicalize(path: &Path) -> Result<PathBuf, ModuleError> {
    fs::canonicalize(path).map_err(|error| {
        ModuleError::new(format!("cannot resolve module {}: {error}", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::evaluate_expression_module as load_module;
    use super::evaluate_expression_module_with_quota_and_debug_sink as load_module_with_quota_and_debug_sink;
    use super::*;
    use crate::parse_json;
    use std::sync::Mutex;

    fn named_output(value: Value) -> Value {
        let Value::Dict(exports) = value else {
            panic!("explicit module must return an export record")
        };
        exports.get("output").cloned().expect("output export")
    }
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_native_callback(
        _: &mut crate::CallContext<'_, '_>,
    ) -> Result<(), crate::NativeError> {
        Ok(())
    }

    fn fixture_answer_callback(
        context: &mut crate::CallContext<'_, '_>,
    ) -> Result<(), crate::NativeError> {
        context.set_int(context.result(), 42)
    }

    fn fixture_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("telora-module-test-{unique}"));
        fs::create_dir(&path).unwrap();
        path
    }

    fn recovery_engine() -> Engine {
        Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(1_000_000),
            session_quota: Quota::with_fuel(1_000_000),
        })
    }

    #[derive(Default)]
    struct CapturingDebugSink {
        events: Mutex<Vec<crate::DebugEvent>>,
    }

    impl crate::DebugSink for CapturingDebugSink {
        fn emit(&self, event: crate::DebugEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[test]
    fn core_native_module_ids_are_reserved_unique_and_order_independent() {
        let specs = module_specs();
        let identities = specs
            .iter()
            .map(|spec| (spec.name, spec.native_id))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(identities.len(), specs.len());
        assert!(
            identities
                .values()
                .all(|id| *id > 0 && *id <= crate::value::RESERVED_NATIVE_MODULE_MAX)
        );
        assert_eq!(
            identities.values().copied().collect::<HashSet<_>>().len(),
            specs.len()
        );
        assert_eq!(identities.get(crate::core::EXEC_MODULE), Some(&21));
        assert!(!identities.values().any(|id| *id == 12));
        let reordered = specs
            .iter()
            .rev()
            .map(|spec| (spec.name, spec.native_id))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(identities, reordered);
    }

    #[test]
    fn core_prelude_is_explicitly_importable_with_typed_exports() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "core/prelude" as prelude;
import "core/prelude" { struct as make_struct, validate as check };
import "std/result" as result;
type User = make_struct('None, {name: String});
let user: User = {name: result.unwrap(check(String, "telora"))};
(user, struct == prelude.struct, enum == prelude.enum, union == prelude.union, validate == prelude.validate, make_struct == prelude.struct, check == prelude.validate)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "({name: \"telora\"}, 'True, 'True, 'True, 'True, 'True, 'True)"
        );
        fs::write(
            directory.join("missing.telora"),
            "import \"core/prelude\" { missing }; missing",
        )
        .unwrap();
        let missing =
            load_module(directory.join("missing.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(missing.to_string().contains("has no export \"missing\""));

        fs::write(
            directory.join("duplicate.telora"),
            "import \"core/prelude\" { struct as item, enum as item }; item",
        )
        .unwrap();
        let duplicate =
            load_module(directory.join("duplicate.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(
            duplicate
                .to_string()
                .contains("duplicate module binding \"item\"")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn open_imports_resolve_lazily_and_combine_with_module_bindings() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/result" as result, *;
import "core/prelude" as prelude, { struct as make_struct };
type User = make_struct('None, {name: String});
let user = {name: unwrap('Ok("telora"))};
(user, result.unwrap == unwrap, prelude.struct == make_struct)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "({name: \"telora\"}, 'True, 'True)"
        );

        fs::write(
            directory.join("left.telora"),
            "def shared: Int = 1; export { shared };",
        )
        .unwrap();
        fs::write(
            directory.join("right.telora"),
            "def shared: Int = 2; export { shared };",
        )
        .unwrap();
        fs::write(
            directory.join("unused.telora"),
            "import \"./left.telora\" *; import \"./right.telora\" *; 0",
        )
        .unwrap();
        let unused =
            load_module(directory.join("unused.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(unused.execute(100_000).unwrap().to_string(), "0");

        fs::write(
            directory.join("shadowed.telora"),
            "import \"./left.telora\" *; import \"./right.telora\" *; let shared = 3; shared",
        )
        .unwrap();
        let shadowed =
            load_module(directory.join("shadowed.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(shadowed.execute(100_000).unwrap().to_string(), "3");

        fs::write(
            directory.join("ambiguous.telora"),
            "import \"./left.telora\" *; import \"./right.telora\" *; export { shared as output };",
        )
        .unwrap();
        let ambiguous =
            load_module(directory.join("ambiguous.telora"), BTreeMap::new(), 100_000).unwrap_err();
        let message = ambiguous.to_string();
        assert!(
            message.contains("open import name \"shared\" is ambiguous"),
            "{message}"
        );
        assert!(message.contains("left.telora"));
        assert!(message.contains("right.telora"));
        let recovered = recovery_engine()
            .recover_workspace(directory.join("ambiguous.telora"))
            .unwrap();
        assert!(recovered.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("open import name \"shared\" is ambiguous")
        }));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_exports_synthesize_typed_identity_preserving_module_records() {
        let directory = fixture_dir();
        fs::write(
            directory.join("library.telora"),
            r#"let private = "hidden";
export def identity: for(A) Fn(A) -> A = fn(value) { value };
export let answer = 42;
export { identity as map };"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./library.telora" as library, { identity as id, answer };
import "./library.telora" *;
(id(1), id("telora"), answer, map == library.map, library.identity == library.map)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(1, \"telora\", 42, 'True, 'True)"
        );
        let snapshot = recovery_engine()
            .recover_workspace(directory.join("main.telora"))
            .unwrap();
        let library = snapshot
            .module_by_path(&canonicalize(&directory.join("library.telora")).unwrap())
            .unwrap();
        let exports = snapshot
            .exports_of(library.id)
            .into_iter()
            .map(|export| export.name)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            exports,
            BTreeSet::from(["answer".into(), "identity".into(), "map".into()])
        );

        fs::write(
            directory.join("private.telora"),
            "import \"./library.telora\" { private }; private",
        )
        .unwrap();
        let private =
            load_module(directory.join("private.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(private.to_string().contains("has no export \"private\""));

        fs::write(
            directory.join("forward.telora"),
            "export { later }; let later = 1;",
        )
        .unwrap();
        let forward =
            load_module(directory.join("forward.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(
            forward
                .to_string()
                .contains("cannot export unknown or forward binding \"later\"")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn production_modules_require_explicit_exports() {
        let directory = fixture_dir();
        let module = directory.join("missing-export.telora");
        fs::write(&module, "let value = 42;").unwrap();

        let engine = recovery_engine();
        let error = engine.load_module(&module, BTreeMap::new()).unwrap_err();
        assert!(
            error
                .message()
                .contains("requires at least one explicit export"),
            "{}",
            error.message()
        );

        let snapshot = engine.recover_workspace(&module).unwrap();
        let module = snapshot
            .module_by_path(&canonicalize(&module).unwrap())
            .unwrap();
        assert_eq!(module.state, WorkspaceModuleState::Unavailable);
        assert_eq!(
            snapshot
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic
                    .message
                    .contains("requires at least one explicit export"))
                .count(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn native_type_slots_are_explicit_unique_and_order_independent() {
        fn declarations(
            source: &str,
        ) -> Result<BTreeMap<u32, (String, crate::NativeType)>, ModuleError> {
            let mut sources = SourceDatabase::default();
            let source_id = sources.add("<fixture>", source);
            let program = parse_registered(&sources, source_id).program.unwrap();
            declared_native_types(
                &program,
                crate::value::NativeModuleId(1024),
                "host:fixture",
                &sources,
            )
        }

        let forward = declarations("native type First @7; native type Second @2; First").unwrap();
        let reversed = declarations("native type Second @2; native type First @7; First").unwrap();
        assert_eq!(forward.get(&2).unwrap().1, reversed.get(&2).unwrap().1);
        assert_eq!(forward.get(&7).unwrap().1, reversed.get(&7).unwrap().1);

        let duplicate =
            declarations("native type First @7; native type Second @7; First").unwrap_err();
        assert!(
            duplicate
                .to_string()
                .contains("duplicate native type slot @7")
        );

        let overflow = declarations("native type Huge @4294967296; Huge").unwrap_err();
        assert!(overflow.to_string().contains("must fit the u32 range"));
    }

    #[test]
    fn engine_builder_allocates_and_freezes_host_native_modules() {
        fn spec(name: &str) -> NativeModuleSpec {
            NativeModuleSpec::new(
                name,
                "native type Token @7; native make: Fn() -> Token; {Token: Token, make: make}",
                vec![(
                    "make",
                    crate::NativeFunction::new_with_native_type(
                        "host.make",
                        0,
                        7,
                        fixture_native_callback,
                    ),
                )],
            )
        }

        let mut builder = Engine::builder(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
        });
        assert_eq!(
            builder
                .register_native_module(Some(2_000), spec("acme/stable"))
                .unwrap(),
            2_000
        );
        assert_eq!(
            builder
                .register_native_module(None, spec("acme/automatic"))
                .unwrap(),
            1_024
        );
        assert!(
            builder
                .register_native_module(Some(2_000), spec("acme/collision"))
                .unwrap_err()
                .to_string()
                .contains("already registered")
        );
        assert!(
            builder
                .register_native_module(Some(2_001), spec("acme/stable"))
                .unwrap_err()
                .to_string()
                .contains("name")
        );
        assert!(
            builder
                .register_native_module(Some(1_023), spec("acme/reserved"))
                .unwrap_err()
                .to_string()
                .contains("reserved range")
        );
        assert!(
            builder
                .register_native_module(None, spec("invalid"))
                .unwrap_err()
                .to_string()
                .contains("absolute module path")
        );
        assert!(
            builder
                .register_native_module(None, spec("std/hash"))
                .unwrap_err()
                .to_string()
                .contains("already registered by Telora")
        );
        assert_eq!(
            builder
                .register_native_module(None, spec("core/future"))
                .unwrap(),
            1_025
        );
        assert_eq!(
            builder
                .register_native_module(None, spec("acme/after-errors"))
                .unwrap(),
            1_026
        );

        let engine = builder.build();
        assert_eq!(
            engine
                .native_modules
                .iter()
                .map(|module| module.id)
                .collect::<Vec<_>>(),
            [1_024, 1_025, 1_026, 2_000]
        );
        let directory = fixture_dir();
        fs::write(directory.join("main.telora"), "export let output = 1;").unwrap();
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        assert_eq!(
            named_output(engine.execute(&module).unwrap()).to_string(),
            "1"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn registered_host_modules_flow_through_execution_and_workspace_recovery() {
        let config = EngineConfig {
            module_quota: Quota::with_fuel(500_000),
            session_quota: Quota::with_fuel(500_000),
        };
        let mut builder = Engine::builder(config);
        builder
            .register_native_module(
                Some(1_500),
                NativeModuleSpec::new(
                    "acme/runtime",
                    "native type Token @9; native answer: Fn() -> Int; export { Token, answer };",
                    vec![(
                        "answer",
                        crate::NativeFunction::new(
                            "acme/runtime.answer",
                            0,
                            fixture_answer_callback,
                        ),
                    )],
                ),
            )
            .unwrap();
        let engine = builder.build();
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "acme/runtime" as host;
import "std/type-desc" as desc;
export let output = {answer: host.answer(), name: desc.opaque_name(host.Token)};"#,
        )
        .unwrap();

        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let Value::Dict(output) = named_output(engine.execute(&module).unwrap()) else {
            panic!("Host native module test must return a Dict")
        };
        assert_eq!(output.get("answer").unwrap().to_string(), "42");
        assert_eq!(
            output.get("name").unwrap().to_string(),
            "'Some(\"acme/runtime#Token\")"
        );
        let host = module
            .workspace
            .modules()
            .iter()
            .find(|module| module.name == "acme/runtime")
            .unwrap();
        assert_eq!(host.kind, WorkspaceModuleKind::Core);
        assert_eq!(host.state, WorkspaceModuleState::Known);

        let snapshot = engine.recover_workspace(&main).unwrap();
        assert!(snapshot.diagnostics().is_empty());
        assert!(snapshot.modules().iter().any(|module| {
            module.name == "acme/runtime"
                && module.kind == WorkspaceModuleKind::Core
                && module.state == WorkspaceModuleState::Known
        }));
        let clock = crate::RevisionClock::default();
        let context = crate::QueryContext::current(clock);
        let source = snapshot
            .module_by_path(&fs::canonicalize(&main).unwrap())
            .unwrap()
            .source
            .unwrap();
        let source_text = snapshot.sources().get(source).text().to_string();
        let needle = "host.answer";
        let offset = source_text.find(needle).unwrap() + needle.len();
        let completion = block_on_recovery(snapshot.query_completion_at(
            &context,
            crate::Location::new(source, crate::TextRange::at(offset as u32)),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(completion.candidates.len(), 1);
        assert_eq!(completion.candidates[0].label, "answer");
        assert_eq!(
            completion.candidates[0].kind,
            crate::CompletionKind::ModuleExport
        );
        let async_snapshot =
            block_on_recovery(engine.recover_workspace_async(&main, &BTreeMap::new(), &context))
                .unwrap();
        assert!(async_snapshot.diagnostics().is_empty());

        let isolated = Engine::new(config)
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(isolated.to_string().contains("unknown dependency"));
        assert!(isolated.to_string().contains("main.telora:1:8"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn selected_entry_runs_graph_visible_registered_native_modules() {
        let config = EngineConfig {
            module_quota: Quota::with_fuel(500_000),
            session_quota: Quota::with_fuel(500_000),
        };
        let native_source = "native answer: Fn() -> Int; export { answer };";
        let mut builder = Engine::builder(config);
        builder
            .register_native_module(
                Some(1_500),
                NativeModuleSpec::new(
                    "dep/service.native.telora",
                    native_source,
                    vec![(
                        "answer",
                        crate::NativeFunction::new(
                            "dep/service.answer",
                            0,
                            fixture_answer_callback,
                        ),
                    )],
                ),
            )
            .unwrap();
        let engine = builder.build();
        let directory = fixture_dir();
        fs::create_dir_all(directory.join("src/bin")).unwrap();
        fs::create_dir_all(directory.join("dependency/src")).unwrap();
        fs::write(
            directory.join("telora-deps.json"),
            r#"{"dependencies":{"dep":{"path":"dependency"}}}"#,
        )
        .unwrap();
        fs::write(
            directory.join("dependency/src/service.native.telora"),
            native_source,
        )
        .unwrap();
        fs::write(
            directory.join("src/bin/main.telora"),
            "export let marker = 0;",
        )
        .unwrap();
        let entry = directory.join("entry.telora");
        fs::write(
            &entry,
            r#"import "std/rt.priv.telora" as rt;
import "dep/service.native.telora" as service;
@struct type Main = {marker: Int};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (service.answer(), fn(state, event) {
        match event {
            'Initialize => (state, ['Output("42"), 'Exit(0)]),
            _ => fail!("unexpected event", event),
        }
    })
};"#,
        )
        .unwrap();
        let pending = engine
            .prepare_module_id(&directory, "@bin/main.telora")
            .unwrap();
        let outcome = block_on_recovery(engine.run_pending(pending, None, Some(&entry))).unwrap();
        assert_eq!(outcome.output, "42");
        assert_eq!(outcome.termination, RunTermination::Exit(0));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unknown_host_modules_are_unavailable_without_blocking_independent_facts() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "acme/missing" as missing;
type Independent = String;
{Independent: Independent}"#,
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.message.contains("unknown dependency")
                && diagnostic.labels[0].location.start == 7
        }));
        let root = snapshot
            .module_by_path(&fs::canonicalize(&main).unwrap())
            .unwrap();
        let independent = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.module == root.id && definition.name == "Independent")
            .unwrap();
        assert_eq!(independent.ty.state, crate::FactState::Known);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn contextual_debug_observes_values_with_authored_context() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"let identity: Fn(Any) -> Any = fn(value) { value };
               let data = { text: "line\nnext", items: [1, 'Ok, (2,)] };
               let observed = dbg!(data, "loaded\nvalue");
               let seen_identity = dbg!(identity);
               let seen_value = dbg!(observed);
               let whole_float = dbg!(3.0);
               let negative_zero = dbg!(-0.0);
               export let output = if seen_identity == identity { seen_value } else { data };"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        let engine = Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
        })
        .with_debug_sink(sink.clone());
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        assert_eq!(
            named_output(engine.execute(&module).unwrap()).to_string(),
            "{items: [1, 'Ok, (2)], text: \"line\\nnext\"}"
        );
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].message.as_deref(), Some("loaded\nvalue"));
        assert_eq!(events[0].name, "data");
        assert!(events[0].module.ends_with("main.telora"));
        assert_eq!(events[0].line, 3);
        assert_eq!(
            events[0].repr,
            "{\"items\": [1, 'Ok, (2,)], \"text\": \"line\\nnext\"}"
        );
        assert_eq!(events[1].name, "identity");
        assert!(events[1].repr.starts_with("<fn "));
        assert_eq!(events[2].name, "observed");
        assert_eq!(events[2].repr, events[0].repr);
        assert_eq!(events[3].name, "3.0");
        assert_eq!(events[3].repr, "3.0");
        assert_eq!(events[4].name, "-0.0");
        assert_eq!(events[4].repr, "-0.0");
        drop(events);

        fs::write(
            directory.join("bad-message.telora"),
            r#"let message = "dynamic"; export let output = dbg!(42, message);"#,
        )
        .unwrap();
        let bad = engine
            .load_module(directory.join("bad-message.telora"), BTreeMap::new())
            .unwrap_err();
        assert!(bad.to_string().contains("String literal"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn runtime_debug_does_not_emit_during_bootstrap_analysis() {
        let directory = fixture_dir();
        let sink = Arc::new(CapturingDebugSink::default());
        let engine = Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
        })
        .with_debug_sink(sink.clone());

        for (name, type_binding) in [
            ("without-type.telora", ""),
            ("with-type.telora", "type Number = Int;"),
        ] {
            let path = directory.join(name);
            fs::write(
                &path,
                format!(
                    "{type_binding}\nlet value = 1;\nlet observed = dbg!(value);\nexport let output = \"ok\";"
                ),
            )
            .unwrap();
            let before = sink.events.lock().unwrap().len();
            let module = engine.load_module(path, BTreeMap::new()).unwrap();
            assert_eq!(sink.events.lock().unwrap().len(), before, "{name}");
            assert_eq!(
                named_output(engine.execute(&module).unwrap()).to_string(),
                "\"ok\""
            );
            assert_eq!(sink.events.lock().unwrap().len(), before + 1, "{name}");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn contextual_debug_is_outside_telora_fuel_and_allocation() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"export let output = dbg!(42, "answer");"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        let engine = Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
        })
        .with_debug_sink(sink.clone());
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        let initial_events = sink.events.lock().unwrap().len();
        let mut exact = QuotaAccount::new(Quota::new(0, 1_000, u64::MAX));
        let arena = Vm::new()
            .with_debug_sink(sink.clone())
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut exact,
            )
            .unwrap();
        assert_eq!(
            named_output(arena.export(&module.runtime.main.heap).unwrap()).to_string(),
            "42"
        );
        assert_eq!(sink.events.lock().unwrap().len(), initial_events + 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn metadata_only_helpers_are_erased_but_runtime_helpers_are_retained() {
        let directory = fixture_dir();
        fs::write(
            directory.join("erased.telora"),
            r#"def observe: Fn(Any) -> Any = fn(value) { dbg!(value, "metadata") };
               type Observed = observe(Int);
               0"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        let erased = load_module_with_quota_and_debug_sink(
            directory.join("erased.telora"),
            BTreeMap::new(),
            Quota::with_fuel(100_000),
            sink.clone(),
        )
        .unwrap();
        assert_eq!(sink.events.lock().unwrap().len(), 1);
        assert_eq!(
            erased
                .execute_with_quota(Quota::new(0, 1_000, 0))
                .unwrap()
                .to_string(),
            "0"
        );
        assert_eq!(sink.events.lock().unwrap().len(), 1);

        fs::write(
            directory.join("retained.telora"),
            r#"def observe: Fn(Any) -> Any = fn(value) { dbg!(value, "observed") };
               type Observed = observe(Int);
               observe(1)"#,
        )
        .unwrap();
        let retained = load_module_with_quota_and_debug_sink(
            directory.join("retained.telora"),
            BTreeMap::new(),
            Quota::with_fuel(100_000),
            sink.clone(),
        )
        .unwrap();
        assert_eq!(sink.events.lock().unwrap().len(), 2);
        retained
            .execute_with_quota_and_debug_sink(Quota::with_fuel(2), sink.clone())
            .unwrap();
        assert_eq!(sink.events.lock().unwrap().len(), 3);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bootstrap_shadow_does_not_consume_the_module_initialization_quota() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"type Observed = dbg!(Int);
               0"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        load_module_with_quota_and_debug_sink(
            directory.join("main.telora"),
            BTreeMap::new(),
            Quota::new(1, 1_000, u64::MAX),
            sink.clone(),
        )
        .unwrap();
        assert_eq!(
            sink.events.lock().unwrap().len(),
            1,
            "only authoritative MetadataInit is observable and charged"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn derived_codec_normalizes_options_and_pretty_prints_json() {
        let directory = fixture_dir();
        fs::write(
            directory.join("User.telora"),
            r#"import "std/codec" as codec;
               import "std/result" as result;
               @struct type User = {v: Option(String)};
               let decode = fn(value) { codec.decode(User, value) };
               let encode = fn(value) {
                   codec.encode(User, value) |> result.unwrap
               };
               {Type: User, decode: decode, encode: encode}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./abc.json" as data;
               import "./User.telora" as User;
               import "std/result" as result;
               import "std/json" as json;
               let user = data |> User.decode |> result.unwrap;
               user |> User.encode |> json.stringify_pretty(2)"#,
        )
        .unwrap();

        let expected = [
            (r#"{"v":"abc"}"#, "{\n  \"v\": \"abc\"\n}"),
            (r#"{"v":null}"#, "{\n  \"v\": null\n}"),
            (r#"{}"#, "{\n  \"v\": null\n}"),
        ];
        for (source, output) in expected {
            fs::write(directory.join("abc.json"), source).unwrap();
            let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000)
                .unwrap_or_else(|error| panic!("failed to load {source}: {error}"));
            assert_eq!(
                module.execute(100_000).unwrap().to_string(),
                format!("{output:?}")
            );
        }

        fs::write(directory.join("abc.json"), r#"{"v":1}"#).unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let failure = module.execute(100_000).unwrap_err();
        assert!(failure.message.contains("$.v"), "{}", failure.message);
        assert!(failure.message.contains("String"), "{}", failure.message);
        let data_location = failure
            .data_location()
            .expect("codec failure must retain the invalid JSON value location");
        assert_eq!(
            module.sources.get(data_location.source).name.as_ref(),
            directory.join("abc.json").display().to_string()
        );
        assert_eq!(
            module
                .sources
                .get(data_location.source)
                .slice(data_location)
                .as_deref(),
            Some("1")
        );
        let rendered = failure.to_string();
        assert!(rendered.contains("abc.json:1:6:"), "{rendered}");
        assert!(
            rendered.contains("contract rule declared here"),
            "{rendered}"
        );
        assert!(rendered.contains("User.telora:3:47:"), "{rendered}");

        fs::write(
            directory.join("inspect.telora"),
            r#"import "./abc.json" as data;
               import "./User.telora" as User;
               data |> User.decode"#,
        )
        .unwrap();
        let inspected = load_module(directory.join("inspect.telora"), BTreeMap::new(), 100_000)
            .unwrap()
            .execute(100_000)
            .unwrap();
        let Value::Tagged { tag, payload } = inspected else {
            panic!("codec must return a tagged Result")
        };
        assert_eq!(tag.name(), "Err");
        let Value::Dict(payload) = payload.as_ref() else {
            panic!("codec failure must be an ordinary diagnostic Dict")
        };
        assert!(payload.get("message").is_some());
        assert_eq!(payload.get("data").unwrap().to_string(), "1");
        assert_eq!(payload.get("rule").unwrap().to_string(), "{kind: 'String}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codec_accepts_user_computed_canonical_type_metadata() {
        let directory = fixture_dir();
        fs::write(directory.join("data.json"), r#"{"v":"plain"}"#).unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./data.json" as data;
               import "std/codec" as codec;
               import "std/result" as result;
               type StringRule = {kind: 'String};
               type UserRule = {kind: 'Struct, fields: {v: StringRule}};
               codec.decode(UserRule, data) |> result.unwrap"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{v: \"plain\"}"
        );

        fs::write(
            directory.join("legacy.telora"),
            r#"import "std/result" as result; result.unwrap('Err("legacy"))"#,
        )
        .unwrap();
        let legacy = load_module(directory.join("legacy.telora"), BTreeMap::new(), 100_000)
            .unwrap()
            .execute(100_000)
            .unwrap_err();
        assert_eq!(legacy.message, "legacy");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codec_rejects_struct_shape_errors_and_json_is_strict() {
        let directory = fixture_dir();
        let cases = [
            (
                r#"import "std/codec" as codec;
                   import "std/result" as result;
                   @struct type T = {name: String};
                   codec.decode(T, {}) |> result.unwrap"#,
                "$.name: missing required field",
            ),
            (
                r#"import "std/codec" as codec;
                   import "std/result" as result;
                   @struct type T = {name: String};
                   codec.decode(T, {name: "Ada", extra: 1}) |> result.unwrap"#,
                "$.extra: unknown field",
            ),
            (
                r#"import "std/json" as json; json.stringify((1, 2))"#,
                "JSON cannot encode Tuple",
            ),
            (
                r#"import "std/json" as json; json.stringify_pretty(17)"#,
                "indent must be between 0 and 16",
            ),
        ];
        for (index, (source, expected)) in cases.into_iter().enumerate() {
            let path = directory.join(format!("case-{index}.telora"));
            fs::write(&path, source).unwrap();
            let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
            let failure = module.execute(100_000).unwrap_err();
            assert!(failure.message.contains(expected), "{}", failure.message);
        }

        let path = directory.join("compact.telora");
        fs::write(
            &path,
            r#"import "std/json" as json;
               json.stringify({z: [1, 'True], a: "line\nnext"})"#,
        )
        .unwrap();
        let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            r#""{\"a\":\"line\\nnext\",\"z\":[1,true]}""#
        );
        assert_eq!(
            module
                .execute_with_quota(Quota::new(100_000, 1_000, 1))
                .expect_err("allocation must be exhausted")
                .kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loads_json_and_telora_modules_with_types() {
        let directory = fixture_dir();
        fs::write(directory.join("user.json"), r#"{"name":"Ada","age":36}"#).unwrap();
        fs::write(directory.join("answer.telora"), "40 + 2").unwrap();
        fs::write(
            directory.join("main.telora"),
            "import \"./user.json\" as user;\
             import \"./answer.telora\" as answer;\
             @struct type User = {name: String, age: Int};\
             let checked: User = user;\
             (checked.name, answer)",
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.dependencies.len(), 3);
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(\"Ada\", 42)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loads_toml_modules_with_temporal_tags_and_reuses_resolved_identity() {
        let directory = fixture_dir();
        fs::write(
            directory.join("config.toml"),
            r#"title = "Telora"
released = 2026-08-04
[environment]
PATH = "/bin"
[[tools]]
name = "telora"
[[tools]]
name = "rustc"
"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./config.toml" as config;
               import "./sub/../config.toml" as same;
               import "std/toml" as toml;
               type TomlDate = toml.DateTime;
               @struct type Tool = {name: String};
               @struct type Config = {
                   title: String,
                   released: TomlDate,
                   environment: Dict(String),
                   tools: Array(Tool),
               };
               let checked: Config = config;
               (checked.released, checked.tools, same.title)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.dependencies.len(), 2);
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "('LocalDate(\"2026-08-04\"), [{name: \"telora\"}, {name: \"rustc\"}], \"Telora\")"
        );
        let toml = module
            .workspace
            .module_by_path(&canonicalize(&directory.join("config.toml")).unwrap())
            .unwrap();
        assert_eq!(toml.kind, WorkspaceModuleKind::Toml);
        assert_eq!(toml.state, WorkspaceModuleState::Known);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn toml_annotation_errors_label_data_and_type_declaration() {
        let directory = fixture_dir();
        fs::write(
            directory.join("user.toml"),
            "name = \"Ada\"\nage = \"old\"\n",
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            "import \"./user.toml\" as user;\n\
             @struct type User = {name: String, age: Int};\n\
             let checked: User = user;\n\
             checked",
        )
        .unwrap();
        let error =
            load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap_err();
        let message = error.message();
        assert!(
            message.contains("user.toml:2:7: binding checked has type"),
            "{message}"
        );
        assert!(
            message.contains("main.telora:2:1: type requirement declared here"),
            "{message}"
        );

        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/result" as result;
               import "./user.toml" as user;
               @struct type User = {name: String, age: Int};
               codec.decode(User, user) |> result.unwrap"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        let rendered = error.with_sources(&module.sources).to_string();
        assert!(rendered.contains("user.toml:2:7:"), "{rendered}");
        assert!(rendered.contains("main.telora:4:"), "{rendered}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_retains_invalid_toml_source_and_diagnostics() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let config = directory.join("config.toml");
        fs::write(&config, "name = \"first\"\nname = \"second\"\n").unwrap();
        fs::write(&main, "import \"./config.toml\" as config; config").unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let config = snapshot
            .module_by_path(&canonicalize(&config).unwrap())
            .unwrap();
        assert_eq!(config.kind, WorkspaceModuleKind::Toml);
        assert_eq!(config.state, WorkspaceModuleState::Unavailable);
        let source = config.source.expect("invalid TOML source is retained");
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.message.contains("duplicate TOML key")
                && diagnostic
                    .labels
                    .first()
                    .is_some_and(|label| label.location.source == source)
        }));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loads_yaml_modules_and_retains_invalid_workspace_source() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let config = directory.join("config.yaml");
        fs::write(
            &config,
            "name: Telora\nfeatures:\n  - static data\n  - provenance\nlegacy: yes\n",
        )
        .unwrap();
        fs::write(
            &main,
            "import \"./config.yaml\" as config; (config.name, config.features, config.legacy)",
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(\"Telora\", [\"static data\", \"provenance\"], \"yes\")"
        );
        let yaml = module
            .workspace
            .module_by_path(&canonicalize(&config).unwrap())
            .unwrap();
        assert_eq!(yaml.kind, WorkspaceModuleKind::Yaml);
        assert_eq!(yaml.state, WorkspaceModuleState::Known);

        fs::write(&config, "name: first\nname: second\n").unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let yaml = snapshot
            .module_by_path(&canonicalize(&config).unwrap())
            .unwrap();
        assert_eq!(yaml.kind, WorkspaceModuleKind::Yaml);
        assert_eq!(yaml.state, WorkspaceModuleState::Unavailable);
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate YAML key"))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_module_cycles() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            "import \"./a.telora\" as a; a",
        )
        .unwrap();
        fs::write(directory.join("a.telora"), "import \"./b.telora\" as b; b").unwrap();
        fs::write(directory.join("b.telora"), "import \"./a.telora\" as a; a").unwrap();
        let error =
            load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.message().contains("cycle"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_unregistered_and_nested_native_declarations_with_locations() {
        let directory = fixture_dir();
        fs::write(
            directory.join("missing-native.telora"),
            "native missing: Fn(Int) -> Int; missing(1)",
        )
        .unwrap();
        let missing = load_module(
            directory.join("missing-native.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(missing.message().contains("only allowed"));
        assert!(missing.to_string().contains("missing-native.telora:1:1"));
        let recovered = recovery_engine()
            .recover_workspace(directory.join("missing-native.telora"))
            .unwrap();
        assert!(recovered.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("only allowed in built-in or *.native.telora modules")
        }));

        fs::write(
            directory.join("missing-native-type.telora"),
            "native type State @1; State",
        )
        .unwrap();
        let missing_type = load_module(
            directory.join("missing-native-type.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(missing_type.message().contains("only allowed"));
        assert!(
            missing_type
                .to_string()
                .contains("missing-native-type.telora:1:1")
        );

        fs::write(
            directory.join("system.native.telora"),
            "native missing: Fn(Int) -> Int; missing(1)",
        )
        .unwrap();
        fs::write(
            directory.join("system-user.telora"),
            "import \"./system.native.telora\" as system; system",
        )
        .unwrap();
        let system = load_module(
            directory.join("system-user.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(
            system
                .message()
                .contains("not registered for this system module")
        );
        assert!(!system.message().contains("only allowed"));

        fs::write(
            directory.join("nested-native.telora"),
            "let value = { native hidden: Fn(Int) -> Int; 1 }; value",
        )
        .unwrap();
        let nested = load_module(
            directory.join("nested-native.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(
            nested
                .message()
                .contains("only allowed at module top level")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imports_recursive_function_roots_from_the_persistent_world() {
        let directory = fixture_dir();
        fs::write(
            directory.join("countdown.telora"),
            "def countdown: Fn(Int) -> Int = fn(n) { if n < 1 { 0 } else { countdown(n - 1) } }; countdown",
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            "import \"./countdown.telora\" as countdown; countdown(4)",
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.execute(100_000).unwrap().to_string(), "0");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn external_input_is_any_and_available_at_runtime() {
        let directory = fixture_dir();
        fs::write(directory.join("main.telora"), "input").unwrap();
        let input = parse_json("input", r#"{"value":42}"#).unwrap();
        let module = load_module(
            directory.join("main.telora"),
            BTreeMap::from([("input".into(), input)]),
            100_000,
        )
        .unwrap();
        assert_eq!(
            module
                .analysis
                .types
                .node(module.analysis.binding_types["input"]),
            &crate::TypeNode::Any
        );
        assert_eq!(module.execute(100_000).unwrap().to_string(), "{value: 42}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn annotation_error_labels_json_data_and_telora_type_declaration() {
        let directory = fixture_dir();
        fs::write(directory.join("user.json"), r#"{"name":"Ada","age":"old"}"#).unwrap();
        fs::write(
            directory.join("main.telora"),
            "import \"./user.json\" as user;\n\
             @struct type User = {name: String, age: Int};\n\
             let checked: User = user;\n\
             checked",
        )
        .unwrap();
        let error =
            load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap_err();
        let message = error.message();
        assert!(
            message.contains("user.json:1:21: binding checked has type"),
            "{message}"
        );
        assert!(
            message.contains("main.telora:2:1: type requirement declared here"),
            "{message}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn module_execution_uses_evaluation_fuel_semantics() {
        let directory = fixture_dir();
        fs::write(directory.join("straight.telora"), "40 + 2").unwrap();
        let straight = load_module(directory.join("straight.telora"), BTreeMap::new(), 0).unwrap();
        assert_eq!(straight.execute(0).unwrap().to_string(), "42");

        fs::write(
            directory.join("call.telora"),
            "let identity = fn(value) { value }; identity(42)",
        )
        .unwrap();
        let call = load_module(directory.join("call.telora"), BTreeMap::new(), 0).unwrap();
        assert_eq!(
            call.execute(0).unwrap_err().kind,
            crate::RuntimeErrorKind::FuelExhausted
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn engine_applies_module_and_session_quotas_at_separate_boundaries() {
        let directory = fixture_dir();
        fs::write(
            directory.join("typed.telora"),
            "type First = Array(Int); type Second = Array(Int); export let output = 0;",
        )
        .unwrap();
        let module_limited = Engine::new(EngineConfig {
            module_quota: Quota::new(1, 1_000, u64::MAX),
            session_quota: Quota::new(100, 1_000, u64::MAX),
        });
        let error = module_limited
            .load_module(directory.join("typed.telora"), BTreeMap::new())
            .unwrap_err();
        assert!(error.message().contains("fuel"));

        fs::write(directory.join("value.telora"), "export let output = [1];").unwrap();
        let session_limited = Engine::new(EngineConfig {
            module_quota: Quota::new(100, 1_000, u64::MAX),
            session_quota: Quota::new(100, 1_000, 0),
        });
        let module = session_limited
            .load_module(directory.join("value.telora"), BTreeMap::new())
            .unwrap();
        assert_eq!(
            session_limited.execute(&module).unwrap_err().kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );
        assert_eq!(
            session_limited.execute(&module).unwrap_err().kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ready_module_root_is_promoted_once_into_the_shared_world() {
        let directory = fixture_dir();
        let data = directory.join("data.json");
        fs::write(&data, r#"{"name":"Ada","items":[1,2,3]}"#).unwrap();
        let mut loader = ModuleLoader {
            resolver: ModuleResolver::for_root(&data).unwrap(),
            cache: HashMap::new(),
            core_modules: HashMap::new(),
            main: MainWorld::building(),
            visiting: Vec::new(),
            dependencies: BTreeSet::new(),
            module_quota: Quota::with_fuel(100_000),
            debug_sink: Arc::new(DiscardDebugSink),
            sources: SourceDatabase::default(),
            semantic_inputs: BTreeMap::new(),
            source_policy: ModuleSourcePolicy::ExpressionHarness,
        };

        let first = loader.load_value(&data).unwrap();
        let counts = loader.main.heap.counts();
        let data_id = loader.resolver.resolve_root(&data).unwrap().id;
        let first_root = match loader.cache.get(&data_id).unwrap() {
            ModuleState::Ready { root, .. } => *root,
        };
        let second = loader.load_value(&data).unwrap();
        let second_root = match loader.cache.get(&data_id).unwrap() {
            ModuleState::Ready { root, .. } => *root,
        };

        assert_eq!(first.value.to_string(), second.value.to_string());
        assert_eq!(first_root, second_root);
        assert_eq!(counts, loader.main.heap.counts());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sessions_use_fresh_work_worlds_and_leave_frozen_main_unchanged() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays; arrays.map([1, 2], fn(x) { x + 1 })"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let main_counts = module.runtime.main.heap.counts();
        assert!(main_counts.0 > 0, "core modules must be installed in Main");

        assert_eq!(module.execute(100_000).unwrap().to_string(), "[2, 3]");
        assert_eq!(module.execute(100_000).unwrap().to_string(), "[2, 3]");
        assert_eq!(module.runtime.main.heap.counts(), main_counts);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_module_runs_higher_order_operations_and_nested_callbacks() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               let values = [1, 2, 3];
               let empty: Array(Int) = [];
               let empty_strings: Array(String) = [];
               {
                   length: arrays.length(values),
                   first: arrays.get(values, 0),
                   last: arrays.get(values, 2),
                   negative: arrays.get(values, -1),
                   out_of_range: arrays.get(values, 3),
                   empty_get: arrays.get(empty, 0),
                   enumerated: arrays.enumerate(values),
                   enumerated_empty: arrays.enumerate(empty),
                   pushed: arrays.push(values, 4),
                   pushed_empty: arrays.push(empty, 1),
                   original: values,
                   zipped: arrays.zip(values, ["a", "b", "c"]),
                   zip_mismatch: arrays.zip(values, ["a"]),
                   zip_empty: arrays.zip(empty, empty_strings),
                   mapped: arrays.map(values, fn(value) { value + 10 }),
                   filtered: arrays.filter(values, fn(value) { 1 < value }),
                   flattened: arrays.flat_map(values, fn(value) { [value, value] }),
                   folded: arrays.fold(values, 0, fn(total, value) { total + value }),
                   controlled: arrays.fold_control@[Int, Int, String](
                       values,
                       0,
                       fn(total, value) { 'Continue(total + value) },
                   ),
                   controlled_break: arrays.fold_control@[Int, Int, String](
                       [1, 0],
                       0,
                       fn(total, value) {
                           if 0 < value { 'Break("done") } else { 'Continue(total + 1 / value) }
                       },
                   ),
                   controlled_empty: arrays.fold_control@[Int, Int, String](
                       empty,
                       42,
                       fn(total, value) { 'Continue(total + 1 / value) },
                   ),
                   empty_map: arrays.map(empty, fn(value) { value / 0 }),
                   empty_filter: arrays.filter(empty, fn(unused) { 'True }),
                   empty_flat_map: arrays.flat_map(empty, fn(value) { [value] }),
                   empty_fold: arrays.fold(empty, 42, fn(total, value) { total + value }),
                   nested: arrays.map(values, fn(value) {
                       arrays.fold([value, value], 0, fn(total, item) { total + item })
                   }),
                   pipelined: values |> arrays.map\(_, fn(value) { value + 20 }),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let value = module.execute(100_000).unwrap();
        let Value::Dict(result) = value else {
            panic!("expected Dict result")
        };
        assert_eq!(result.get("length").unwrap().to_string(), "3");
        assert_eq!(result.get("first").unwrap().to_string(), "'Some(1)");
        assert_eq!(result.get("last").unwrap().to_string(), "'Some(3)");
        assert_eq!(result.get("negative").unwrap().to_string(), "'None");
        assert_eq!(result.get("out_of_range").unwrap().to_string(), "'None");
        assert_eq!(result.get("empty_get").unwrap().to_string(), "'None");
        assert_eq!(
            result.get("enumerated").unwrap().to_string(),
            "[(0, 1), (1, 2), (2, 3)]"
        );
        assert_eq!(result.get("enumerated_empty").unwrap().to_string(), "[]");
        assert_eq!(result.get("pushed").unwrap().to_string(), "[1, 2, 3, 4]");
        assert_eq!(result.get("pushed_empty").unwrap().to_string(), "[1]");
        assert_eq!(result.get("original").unwrap().to_string(), "[1, 2, 3]");
        assert_eq!(
            result.get("zipped").unwrap().to_string(),
            "'Some([(1, \"a\"), (2, \"b\"), (3, \"c\")])"
        );
        assert_eq!(result.get("zip_mismatch").unwrap().to_string(), "'None");
        assert_eq!(result.get("zip_empty").unwrap().to_string(), "'Some([])");
        assert_eq!(result.get("mapped").unwrap().to_string(), "[11, 12, 13]");
        assert_eq!(result.get("filtered").unwrap().to_string(), "[2, 3]");
        assert_eq!(
            result.get("flattened").unwrap().to_string(),
            "[1, 1, 2, 2, 3, 3]"
        );
        assert_eq!(result.get("folded").unwrap().to_string(), "6");
        assert_eq!(
            result.get("controlled").unwrap().to_string(),
            "'Continue(6)"
        );
        assert_eq!(
            result.get("controlled_break").unwrap().to_string(),
            "'Break(\"done\")"
        );
        assert_eq!(
            result.get("controlled_empty").unwrap().to_string(),
            "'Continue(42)"
        );
        assert_eq!(result.get("empty_map").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_filter").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_flat_map").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_fold").unwrap().to_string(), "42");
        assert_eq!(result.get("nested").unwrap().to_string(), "[2, 4, 6]");
        assert_eq!(result.get("pipelined").unwrap().to_string(), "[21, 22, 23]");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_fold_widens_singleton_enum_fields_in_callback_results() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               @enum type Kind = {
                   Missing: 'None,
                   Unauthorized: 'None,
               };
               @struct type Rejection = {kind: Kind};
               let initial: Array(Rejection) = [];
               array.fold([1, 2], initial, fn(rejections, value) {
                   let rejection = if value == 1 {
                       {kind: 'Missing}
                   } else {
                       {kind: 'Unauthorized}
                   };
                   array.push(rejections, rejection)
               })"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "Array<{kind: enum {Missing, Unauthorized}}>"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "[{kind: 'Missing}, {kind: 'Unauthorized}]"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_fold_defers_singleton_evidence_until_declared_result_widens_it() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               def collect: Fn(Array(Int)) -> Array(Option(Int)) =
                   fn(values) {
                       let options = array.fold(values, [], fn(options, value) {
                           if value == 1 {
                               array.push(options, 'None)
                           } else {
                               array.push(options, 'Some(value))
                           }
                       });
                       options
                   };
               collect([1, 2])"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let inferred = module.analysis.display(module.analysis.result_type);
        assert_eq!(inferred, "Array<enum {None, Some(Int)}>");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "['None, 'Some(2)]"
        );

        fs::write(
            directory.join("types.telora"),
            r#"@enum type Kind = {
                   Missing: 'None,
                   Unauthorized: 'None,
               };
               export { Kind };"#,
        )
        .unwrap();
        fs::write(
            directory.join("records.telora"),
            r#"import "std/array" as array;
               import "./types.telora" { Kind };
               @struct type Rejection(Subject) = {kind: Kind, subject: Subject};
               def reject_all: for(Subject)
                   Fn(Array(Int), Subject) -> Array(Rejection(Subject)) =
                   fn(values, subject) {
                       let rejections = array.fold(values, [], fn(rejections, value) {
                           if value == 1 {
                               array.push(rejections, {kind: 'Missing, subject: subject})
                           } else {
                               array.push(rejections, {kind: 'Unauthorized, subject: subject})
                           }
                       });
                       rejections
                   };
               reject_all([2, 1], "subject")"#,
        )
        .unwrap();
        let records =
            load_module(directory.join("records.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            records.analysis.display(records.analysis.result_type),
            "Array<{kind: enum {Missing, Unauthorized}, subject: String}>"
        );
        assert_eq!(
            records.execute(100_000).unwrap().to_string(),
            "[{kind: 'Unauthorized, subject: \"subject\"}, {kind: 'Missing, subject: \"subject\"}]"
        );

        fs::write(
            directory.join("invalid.telora"),
            r#"import "std/array" as array;
               def collect: Fn(Array(Int)) -> Array(Option(Int)) =
                   fn(values) {
                       let options = array.fold(values, [], fn(options, value) {
                           if value == 1 {
                               array.push(options, 'None)
                           } else {
                               array.push(options, 'Missing)
                           }
                       });
                       options
                   };
               collect([1, 2])"#,
        )
        .unwrap();
        let error =
            load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("'Missing"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_fold_infers_anonymous_state_from_computed_array_fields() {
        let directory = fixture_dir();
        let source = r#"import "std/array" as array;
               @struct type Report(A) = {value: A, accepted: Bool};
               @struct type CollectResult(A) = {
                   reports: Array(Report(A)),
                   diagnostics: Array(String),
               };
               def collect: for(A)
                   Fn(Array(A), Array(String)) -> CollectResult(A) =
                   fn(values, prior) {
                       let initial = {reports: [], diagnostics: prior};
                       array.fold(values, initial, fn(acc, value) {
                           if value == value {
                               {reports: array.push(acc.reports, {value, accepted: 'True}),
                                diagnostics: acc.diagnostics}
                           } else {
                               {reports: array.push(acc.reports, {value, accepted: 'False}),
                                diagnostics: array.push(acc.diagnostics, "rejected")}
                           }
                       })
                   };
               collect([1, 2], [])"#;
        fs::write(directory.join("main.telora"), source).unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let expected = module.analysis.display(module.analysis.result_type);
        assert_eq!(
            expected,
            "{diagnostics: Array<String>, reports: Array<{accepted: enum {False, True}, value: Int}>}"
        );

        let reversed = source
            .replace("if value == value", "if value != value")
            .replace(
                "{reports: array.push(acc.reports, {value, accepted: 'True}),\n                                diagnostics: acc.diagnostics}",
                "{diagnostics: acc.diagnostics,\n                                reports: array.push(acc.reports, {accepted: 'True, value})}",
            )
            .replace(
                "{reports: array.push(acc.reports, {value, accepted: 'False}),\n                                diagnostics: array.push(acc.diagnostics, \"rejected\")}",
                "{diagnostics: array.push(acc.diagnostics, \"rejected\"),\n                                reports: array.push(acc.reports, {accepted: 'False, value})}",
            );
        fs::write(directory.join("reversed.telora"), reversed).unwrap();
        let reversed =
            load_module(directory.join("reversed.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            reversed.analysis.display(reversed.analysis.result_type),
            expected
        );

        let incompatible =
            source.replace("Fn(Array(A), Array(String))", "Fn(Array(A), Array(Int))");
        fs::write(directory.join("incompatible.telora"), incompatible).unwrap();
        let error = load_module(
            directory.join("incompatible.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(error.message.contains("String"), "{}", error.message);
        assert!(!error.message.contains(" T"), "{}", error.message);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn function_contracts_resolve_qualified_imported_type_paths() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"@struct type Input = {value: Int};
               @struct type Item = {name: String};
               @struct type Output = {count: Int};
               export { Input, Item, Output };"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./types.telora" as types;
               def consume:
                   Fn(types.Input, Array(types.Item)) -> types.Output =
                   fn(input, items) { {count: input.value} };
               consume({value: 2}, [{name: "first"}])"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "{count: Int}"
        );
        assert_eq!(module.execute(100_000).unwrap().to_string(), "{count: 2}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_fold_widens_atom_fields_from_callback_results() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               let computed = array.fold(
                   [1, 2, 3],
                   {flag: 'False, items: []},
                   fn(state, item) {
                       {flag: item > 1 || state.flag,
                        items: array.push(state.items, item)}
                   },
               );
               let branched = array.fold(
                   [1, 2],
                   {flag: 'False, items: []},
                   fn(state, item) {
                       if item > 1 {
                           {flag: 'True, items: array.push(state.items, item)}
                       } else {
                           {flag: 'False, items: array.push(state.items, item)}
                       }
                   },
               );
               (computed, branched)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "({flag: enum {False, True}, items: Array<Int>}, {flag: 'False, items: Array<Int>} | {flag: 'True, items: Array<Int>})"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "({flag: 'True, items: [1, 2, 3]}, {flag: 'True, items: [1, 2]})"
        );

        fs::write(
            directory.join("invalid.telora"),
            r#"import "std/array" as array;
               array.fold([1], {flag: 'False}, fn(state, item) {
                   {flag: 'Foreign}
               })"#,
        )
        .unwrap();
        let error =
            load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("Foreign"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_map_widens_singleton_option_arm_in_generic_callback() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               def lower_all: for(Id, Input, Output, Capability)
                   Fn(
                       Array(Id),
                       Fn(Id) -> Option(Capability),
                       Fn(Capability) -> Fn(Id, Input) -> Option(Output),
                       Input,
                   ) -> Array(Option(Output)) =
                   fn(ids, find, lower, input) {
                       array.map(ids, fn(id) {
                           match find(id) {
                               'Some(capability) => lower(capability)(id, input),
                               'None => 'None,
                           }
                       })
                   };
               lower_all"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "Fn(Array<Any>, Fn(Any) -> enum {None, Some(Any)}, Fn(Any) -> Fn(Any, Any) -> enum {None, Some(Any)}, Any) -> Array<enum {None, Some(Any)}>"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_map_widens_option_fields_across_nested_generic_matches() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               @struct type Capability(Id, Output) = {
                   id: Id,
                   lower: Fn(Id) -> Option(Output),
               };
               def collect: for(Id, Output)
                   Fn(Array(Capability(Id, Output)), Array(Id)) -> Array(Option(Output)) =
                   fn(catalog, requests) {
                       let steps = array.map(requests, fn(requested) {
                           match array.find(catalog, fn(capability) {
                               capability.id == requested
                           }) {
                               'Some(capability) => match capability.lower(requested) {
                                   'Some(value) => {
                                       evidence: 'Some(value),
                                       error: 'None,
                                   },
                                   'None => {
                                       evidence: 'None,
                                       error: 'Some("lowering failed"),
                                   },
                               },
                               'None => {
                                   evidence: 'None,
                                   error: 'Some("missing"),
                               },
                           }
                       });
                       array.map(steps, fn(step) { step.evidence })
                   };
               @enum type Id = { A: 'None, B: 'None };
               def lower_a: Fn(Id) -> Option(Int) = fn(id) {
                   if id == 'A { 'Some(1) } else { 'None }
               };
               let catalog: Array(Capability(Id, Int)) = [{id: 'A, lower: lower_a}];
               collect(catalog, ['A, 'B])"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "Array<enum {None, Some(Int)}>"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "['Some(1), 'None]"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deterministic_array_string_and_path_modules_cover_plan_composition() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               import "std/path" as paths;
               import "std/string" as strings;
               {
                   concat: arrays.concat([[1, 2], [], [3]]),
                   any: arrays.any([1, 0], fn(value) {
                       if 0 < value { 'True } else { value / 0 < 1 }
                   }),
                   all: arrays.all([0, 1], fn(value) {
                       if value < 1 { 'False } else { value / 0 < 1 }
                   }),
                   found: arrays.find([1, 2, 3], fn(value) { 1 < value }),
                   missing: arrays.find([1], fn(value) { value < 0 }),
                   empty_any: arrays.any([], fn(value) { value / 0 < 1 }),
                   empty_all: arrays.all([], fn(value) { value / 0 < 1 }),
                   chars: strings.length("形态a"),
                   joined: strings.join(["a", "形", "c"], ":"),
                   split: strings.split("a::形", ":"),
                   scalar_split: strings.split("a形", ""),
                   starts: strings.starts_with("形态", "形"),
                   ends: strings.ends_with("telora", "ra"),
                   contains: strings.contains("telora", "lor"),
                   replaced: strings.replace("a-b-a", "a", "xy"),
                   lines: strings.lines("a\r\nb\n"),
                   joined_lines: strings.join_lines(["a", "形", "c"]),
                   indented: strings.indent("a\n\nb", 2),
                   trailing: strings.ensure_trailing_newline("a"),
                   margin: strings.trim_margin(r"  |a
    |b
unchanged", "|"),
                   normalized: paths.normalize("/a/./b/../../../../c"),
                   relative: paths.normalize("../../a/../b"),
                   joined_path: paths.join(["/tool", "bin", "../lib", "gcc"]),
                   restarted: paths.join(["ignored", "/absolute", "file"]),
                   parent: paths.parent("/a/b/../c"),
                   root_parent: paths.parent("/"),
                   file_name: paths.file_name("a/b/../c"),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected Dict result")
        };
        assert_eq!(result.get("concat").unwrap().to_string(), "[1, 2, 3]");
        assert_eq!(result.get("any").unwrap().to_string(), "'True");
        assert_eq!(result.get("all").unwrap().to_string(), "'False");
        assert_eq!(result.get("found").unwrap().to_string(), "'Some(2)");
        assert_eq!(result.get("missing").unwrap().to_string(), "'None");
        assert_eq!(result.get("empty_any").unwrap().to_string(), "'False");
        assert_eq!(result.get("empty_all").unwrap().to_string(), "'True");
        assert_eq!(result.get("chars").unwrap().to_string(), "3");
        assert_eq!(result.get("joined").unwrap().to_string(), r#""a:形:c""#);
        assert_eq!(
            result.get("split").unwrap().to_string(),
            r#"["a", "", "形"]"#
        );
        assert_eq!(
            result.get("scalar_split").unwrap().to_string(),
            r#"["", "a", "形", ""]"#
        );
        assert_eq!(result.get("starts").unwrap().to_string(), "'True");
        assert_eq!(result.get("ends").unwrap().to_string(), "'True");
        assert_eq!(result.get("contains").unwrap().to_string(), "'True");
        assert_eq!(result.get("replaced").unwrap().to_string(), r#""xy-b-xy""#);
        assert_eq!(
            result.get("lines").unwrap().to_string(),
            r#"["a", "b", ""]"#
        );
        assert_eq!(
            result.get("joined_lines").unwrap().to_string(),
            "\"a\\n形\\nc\""
        );
        assert_eq!(
            result.get("indented").unwrap().to_string(),
            "\"  a\\n\\n  b\""
        );
        assert_eq!(result.get("trailing").unwrap().to_string(), "\"a\\n\"");
        assert_eq!(
            result.get("margin").unwrap().to_string(),
            "\"a\\nb\\nunchanged\""
        );
        assert_eq!(result.get("normalized").unwrap().to_string(), r#""/c""#);
        assert_eq!(result.get("relative").unwrap().to_string(), r#""../../b""#);
        assert_eq!(
            result.get("joined_path").unwrap().to_string(),
            r#""/tool/lib/gcc""#
        );
        assert_eq!(
            result.get("restarted").unwrap().to_string(),
            r#""/absolute/file""#
        );
        assert_eq!(result.get("parent").unwrap().to_string(), r#"'Some("/a")"#);
        assert_eq!(result.get("root_parent").unwrap().to_string(), "'None");
        assert_eq!(
            result.get("file_name").unwrap().to_string(),
            r#"'Some("c")"#
        );

        for (source, expected) in [
            (
                "import \"std/string\" as strings; strings.indent(\"x\", -1)",
                "indentation width must be non-negative",
            ),
            (
                "import \"std/string\" as strings; strings.trim_margin(\"x\", \"\")",
                "margin marker must not be empty",
            ),
        ] {
            fs::write(directory.join("invalid.telora"), source).unwrap();
            let module =
                load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000).unwrap();
            let error = module.execute(100_000).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn homogeneous_dict_metadata_preserves_types_through_core_codecs_and_schema() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/codec" as codec;
               import "std/dict" as dicts;
               import "std/json" as json;
               import "std/result" as result;
               type Env = Dict(String);
               let env: Env = {PATH: "/bin", HOME: "/tmp"};
               let decoded = codec.decode(Env, {SHELL: "/bin/sh"}) |> result.unwrap;
               {
                   env: env,
                   decoded: decoded,
                   values: dicts.values(env),
                   built: dicts.from_pairs([("A", "one"), ("B", "two")]),
                   encoded: codec.encode(Env, decoded) |> result.unwrap,
                   schema: json.schema(Env),
               }"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "{built: Any, decoded: Dict<String>, encoded: Any, env: Dict<String>, schema: Any, values: Array<Any>}"
        );
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("expected Dict output")
        };
        assert_eq!(
            output.get("values").unwrap().to_string(),
            "[\"/tmp\", \"/bin\"]"
        );
        assert_eq!(
            output.get("built").unwrap().to_string(),
            "{A: \"one\", B: \"two\"}"
        );
        assert_eq!(
            output.get("encoded").unwrap().to_string(),
            "{SHELL: \"/bin/sh\"}"
        );
        let Value::Dict(schema) = output.get("schema").unwrap() else {
            panic!("expected schema Dict")
        };
        assert_eq!(schema.get("type").unwrap().to_string(), "\"object\"");
        assert_eq!(
            schema.get("additionalProperties").unwrap().to_string(),
            "{type: \"string\"}"
        );

        fs::write(
            &main,
            r#"type Env = Dict(String);
               let env: Env = {GOOD: "yes", BAD: 1};
               env"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("BAD"), "{error}");
        assert!(error.to_string().contains("Int"), "{error}");
        assert!(error.to_string().contains("String"), "{error}");

        fs::write(
            &main,
            r#"@struct type Fixed = {a: String};
               let dynamic: Dict(String) = {a: "value"};
               let fixed: Fixed = dynamic;
               fixed"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("not assignable"), "{error}");
        assert!(error.to_string().contains("Dict<String>"), "{error}");

        fs::write(
            &main,
            r#"@struct type Fixed = {a: String};
               let read: Fn(Fixed) -> String = fn(value) { value.a };
               let dynamic: Dict(String) = {a: "value"};
               read(dynamic)"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(
            error.to_string().contains("cannot unify Dict<String>"),
            "{error}"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recursive_dict_metadata_reuses_existing_schema_links() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/json" as json;
               @struct type Node = {children: Dict(Node)};
               json.schema(Node)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let schema = module.execute(100_000).unwrap().to_string();
        assert!(schema.contains("additionalProperties"), "{schema}");
        assert!(schema.contains("$defs"), "{schema}");
        assert!(schema.contains("$ref"), "{schema}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_core_exports_instantiate_per_member_access_but_not_per_local_use() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as arrays;
               {
                   ints: arrays.map([1, 2], fn(value) { value + 1 }),
                   strings: arrays.map(["a"], fn(value) { value }),
               }"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "{ints: Array<Int>, strings: Array<String>}"
        );

        fs::write(
            &main,
            r#"import "std/array" as arrays;
               let map = arrays.map;
               (map([1], fn(value) { value }), map(["a"], fn(value) { value }))"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("cannot unify String with Int"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_definition_exports_instantiate_per_member_access() {
        let directory = fixture_dir();
        fs::write(
            directory.join("identity.telora"),
            r#"decl identity: for(A) Fn(A) -> A;
               def identity = fn(value) { value };
               {identity: identity}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./identity.telora" as generic;
               (generic.identity(1), generic.identity("x"), generic.identity@[_](2))"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "(Int, String, Int)"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(1, \"x\", 2)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_import_forms_do_not_leak_bound_identities_into_definition_checks() {
        let directory = fixture_dir();
        fs::write(
            directory.join("pipeline.telora"),
            r#"import "std/array" as array;
               export def relay:
                   for(Item, Context, Output, Result)
                   Fn(
                       Array(Item),
                       Context,
                       Fn(Item, Context) -> Output,
                       Fn(Array(Output)) -> Result
                   ) -> Result =
                   fn(items, context, transform, finish) {
                       finish(array.map(items, fn(item) {
                           transform(item, context)
                       }))
                   };"#,
        )
        .unwrap();

        for (name, import, relay) in [
            (
                "namespace.telora",
                r#"import "./pipeline.telora" as pipeline;"#,
                "pipeline.relay",
            ),
            (
                "selective.telora",
                r#"import "./pipeline.telora" {relay};"#,
                "relay",
            ),
            (
                "aliased.telora",
                r#"import "./pipeline.telora" {relay as forward};"#,
                "forward",
            ),
            ("open.telora", r#"import "./pipeline.telora" *;"#, "relay"),
        ] {
            fs::write(
                directory.join(name),
                format!(
                    r#"{import}
                       import "std/array" as array;
                       export def execute:
                           for(Prefix, Item, Context, Output, Result)
                           Fn(
                               Prefix,
                               Array(Item),
                               Context,
                               Fn(Prefix, Item, Context) -> Output,
                               Fn(Array(Output)) -> Result
                           ) -> Result =
                           fn(prefix, items, context, transform, finish) {{
                               {relay}(
                                   items,
                                   context,
                                   fn(item, current) {{
                                       transform(prefix, item, current)
                                   }},
                                   finish
                               )
                           }};
                       export let output = execute(
                           1,
                           [2, 3],
                           4,
                           fn(prefix, item, context) {{ prefix + item + context }},
                           fn(values) {{ array.length(values) }}
                       );"#,
                ),
            )
            .unwrap();

            let module = load_module(directory.join(name), BTreeMap::new(), 100_000).unwrap();
            let execute = module
                .analysis
                .hir
                .definitions()
                .iter()
                .find(|definition| definition.name == "execute")
                .expect("execute definition");
            assert_eq!(
                module.analysis.definition_schemes[&execute.id].display_name(),
                "for(Prefix, Item, Context, Output, Result) Fn(Prefix, Array<Item>, Context, Fn(Prefix, Item, Context) -> Output, Fn(Array<Output>) -> Result) -> Result",
                "{name}"
            );
            let Value::Dict(result) = module.execute(100_000).unwrap() else {
                panic!("{name}: expected exported module Dict")
            };
            assert!(
                matches!(result.get("execute"), Some(Value::Func(_))),
                "{name}"
            );
            assert!(
                matches!(result.get("output"), Some(Value::Int(2))),
                "{name}"
            );
        }

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn inferred_generic_let_exports_instantiate_per_member_access() {
        let directory = fixture_dir();
        fs::write(
            directory.join("identity.telora"),
            r#"let identity = fn(value) { value };
               {identity: identity}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./identity.telora" as generic;
               (generic.identity(1), generic.identity("x"), generic.identity@[_](2))"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "(Int, String, Int)"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(1, \"x\", 2)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn acyclic_generic_def_exports_instantiate_per_member_access() {
        let directory = fixture_dir();
        fs::write(
            directory.join("identity.telora"),
            r#"def identity = fn(value) { value };
               {identity: identity}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./identity.telora" as generic;
               (generic.identity(1), generic.identity("x"))"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "(Int, String)"
        );
        assert_eq!(module.execute(100_000).unwrap().to_string(), "(1, \"x\")");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typed_metadata_constructors_cross_module_interfaces() {
        let directory = fixture_dir();
        fs::write(
            directory.join("constructors.telora"),
            r#"def Maybe: for(A) Fn(TypeOf(A)) -> TypeOf(Option(A)) = fn(Item) { Option(Item) };
               {Maybe: Maybe}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./constructors.telora" as constructors;
               type MaybeInt = constructors.Maybe(Int);
               let value: MaybeInt = 'None;
               value"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "enum {None, Some(Int)}"
        );
        assert_eq!(module.execute(100_000).unwrap().to_string(), "'None");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parameterized_type_families_preserve_schemes_across_import_forms() {
        let directory = fixture_dir();
        fs::write(
            directory.join("families.telora"),
            r#"@enum type Status = {Ready: 'None};
               @struct type Box(A) = {status: Status, value: A};
               {Box: Box}"#,
        )
        .unwrap();

        for (name, import, family) in [
            (
                "whole.telora",
                r#"import "./families.telora" as families;"#,
                "families.Box",
            ),
            (
                "selective.telora",
                r#"import "./families.telora" {Box};"#,
                "Box",
            ),
            ("open.telora", r#"import "./families.telora" *;"#, "Box"),
            (
                "aliased.telora",
                r#"import "./families.telora" {Box as Container};"#,
                "Container",
            ),
        ] {
            fs::write(
                directory.join(name),
                format!(
                    r#"{import}
                       type IntBox = {family}(Int);
                       let value: IntBox = {{status: 'Ready, value: 42}};
                       value"#
                ),
            )
            .unwrap();
            let module = load_module(directory.join(name), BTreeMap::new(), 100_000).unwrap();
            assert_eq!(
                module.analysis.display(module.analysis.result_type),
                "{status: enum {Ready}, value: Int}",
                "{name}"
            );
            assert_eq!(
                module.execute(100_000).unwrap().to_string(),
                "{status: 'Ready, value: 42}",
                "{name}"
            );
        }

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parameterized_type_families_construct_local_concrete_types() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"@struct type Box(A) = {value: A};
               type StringBox = Box(String);
               let value: StringBox = {value: "ready"};
               value"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "{value: String}"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{value: \"ready\"}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_generic_struct_families_construct_nested_array_tuple_fields() {
        let directory = fixture_dir();
        fs::write(
            directory.join("families.telora"),
            r#"@struct type Box(A) = {value: Array(Tuple([A, Int]))};
               def make: for(A) Fn(Array(Tuple([A, Int]))) -> Box(A) =
                   fn(value) { {value} };
               {Box, make}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./families.telora" as families;
               families.make([("ready", 1)]).value"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "Array<(String, Int)>"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "[(\"ready\", 1)]"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_families_preserve_recursive_arguments_in_top_level_aliases() {
        let directory = fixture_dir();
        fs::write(
            directory.join("families.telora"),
            r#"@struct type Box(A) = {value: A}; export { Box };"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./families.telora" {Box};
               @struct type Branch = {children: Array(Tree)};
               @enum type Tree = {Leaf: Int, Branch: Branch};
               type TreeBox = Box(Tree);
               def identity: Fn(TreeBox) -> TreeBox = fn(value) { value };
               identity({value: 'Branch({children: ['Leaf(1)]})})"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let alias = module
            .analysis
            .display(module.analysis.declared_types["TreeBox"]);
        assert!(alias.contains("Array<Tree>"), "{alias}");
        assert!(!alias.contains("Any"), "{alias}");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{value: 'Branch({children: ['Leaf(1)]})}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_families_preserve_provider_recursive_fields_regardless_of_declaration_order() {
        let recursive_orders = [
            r#"@struct type CallExpr = {name: String, args: Array(Expr)};
               @enum type Expr = {Literal: Literal, Column: ColumnRef, Call: CallExpr};"#,
            r#"@enum type Expr = {Literal: Literal, Column: ColumnRef, Call: CallExpr};
               @struct type CallExpr = {name: String, args: Array(Expr)};"#,
        ];
        let imports = [
            (
                r#"import "./types.telora" {Expr, Definition};"#,
                "Expr",
                "Definition",
            ),
            (
                r#"import "./types.telora" as types;"#,
                "types.Expr",
                "types.Definition",
            ),
            (r#"import "./types.telora" *;"#, "Expr", "Definition"),
        ];

        for recursive_types in recursive_orders {
            for (import, expr, definition) in imports {
                let directory = fixture_dir();
                let provider = r#"@struct type Literal = {value: Int};
                   @struct type ColumnRef = {alias: String, column: String};
                   $RECURSIVE_TYPES
                   @struct type Definition(Id, Output, Input) = {
                       id: Id,
                       expr: Expr,
                       lower: Fn(Id, Input) -> Output,
                   };
                   export {Expr, CallExpr, Definition};"#
                    .replace("$RECURSIVE_TYPES", recursive_types);
                fs::write(directory.join("types.telora"), provider).unwrap();
                let consumer = r#"$IMPORT
                   @enum type Id = {Name: 'None};
                   @struct type Output = {value: String};
                   @enum type Input = {All: 'None};
                   def column: Fn(String, String) -> $EXPR = fn(alias, name) {
                       'Column({alias: alias, column: name})
                   };
                   def lower: Fn(Id, Input) -> Output = fn(id, input) {
                       {value: "ready"}
                   };
                   type Concrete = $DEFINITION(Id, Output, Input);
                   let definitions: Array(Concrete) = [{
                       id: 'Name,
                       expr: column("t", "name"),
                       lower: lower,
                   }];
                   export let output = definitions;"#
                    .replace("$IMPORT", import)
                    .replace("$EXPR", expr)
                    .replace("$DEFINITION", definition);
                fs::write(directory.join("main.telora"), consumer).unwrap();

                let module =
                    load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
                for name in ["Concrete", "definitions", "output"] {
                    let ty = module
                        .analysis
                        .declared_types
                        .get(name)
                        .or_else(|| module.analysis.binding_types.get(name))
                        .copied()
                        .expect("tested binding has an analyzed type");
                    let ty = module.analysis.display(ty);
                    assert!(!ty.contains("Any"), "{name}: {ty}");
                }
                let output = module.execute(100_000).unwrap().to_string();
                assert!(output.contains("expr: 'Column"), "{output}");
                fs::remove_dir_all(directory).unwrap();
            }
        }
    }

    #[test]
    fn reexported_families_preserve_provider_recursive_fields() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"@struct type Call = {args: Array(Expr)};
               @enum type Expr = {Literal: Int, Call: Call};
               @struct type Family(A) = {expr: Expr, value: A};
               export {Expr, Family};"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./types.telora" {Expr, Family}; export {Expr, Family};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade.telora" {Family};
               type Concrete = Family(String);
               let value: Concrete = {expr: 'Literal(1), value: "ready"};
               export {value};"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        for name in ["Concrete", "value"] {
            let ty = module
                .analysis
                .declared_types
                .get(name)
                .or_else(|| module.analysis.binding_types.get(name))
                .copied()
                .expect("tested binding has an analyzed type");
            let ty = module.analysis.display(ty);
            assert!(!ty.contains("Any"), "{name}: {ty}");
        }
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{value: {expr: 'Literal(1), value: \"ready\"}}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reexported_recursive_generic_calls_accept_equivalent_result_annotations() {
        let directory = fixture_dir();
        fs::write(
            directory.join("expr.telora"),
            r#"@struct type CallExpr = {name: String, args: Array(Expr)};
               @enum type Expr = {Literal: Int, Call: CallExpr};
               export {Expr};"#,
        )
        .unwrap();
        fs::write(
            directory.join("plan.telora"),
            r#"import "./expr.telora" {Expr};
               @struct type Plan(A) = {value: A, expr: Expr};
               @struct type Output = {text: String};
               def render: Fn(Expr) -> String = fn(expr) {
                   match expr {
                       'Literal(value) => `\{value}`,
                       'Call(call) => render(call.args[0]),
                   }
               };
               export def transform: for(A) Fn(Plan(A)) -> Output = fn(plan) {
                   {text: render(plan.expr)}
               };
               export {Plan, Output};"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./plan.telora" {Plan, Output, transform};
               export {Plan, Output, transform};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade.telora" as api;
               @struct type Item = {id: Int};
               type ItemPlan = api.Plan(Item);
               type OutputAlias = api.Output;
               let plan: ItemPlan = {
                   value: {id: 1},
                   expr: 'Call({name: "f", args: ['Literal(1)]}),
               };
               let direct: api.Output = api.transform(plan);
               let alias: OutputAlias = api.transform(plan);
               export {direct, alias};"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{alias: {text: \"1\"}, direct: {text: \"1\"}}"
        );
        for name in ["direct", "alias"] {
            assert_eq!(
                module.analysis.display(module.analysis.binding_types[name]),
                "{text: String}"
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_family_aliases_preserve_provider_local_concrete_arguments() {
        let directory = fixture_dir();
        fs::write(
            directory.join("provider.telora"),
            r#"@struct type Box(A) = {value: A}; export {Box};"#,
        )
        .unwrap();
        fs::write(
            directory.join("alias.telora"),
            r#"import "./provider.telora" {Box};
               @enum type Local = {A: 'None};
               type LocalBox = Box(Local);
               export {LocalBox, Local};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./provider.telora" {Box};
               import "./alias.telora" {LocalBox, Local};
               def identity: Fn(LocalBox) -> LocalBox = fn(value) { value };
               let via_alias: LocalBox = identity({value: 'A});
               let direct: Box(Local) = {value: 'A};
               export let output = (via_alias, direct);"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["via_alias"]),
            "{value: enum {A}}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["direct"]),
            "{value: enum {A}}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["identity"]),
            "Fn({value: enum {A}}) -> {value: enum {A}}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["output"]),
            "({value: enum {A}}, {value: enum {A}})"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_instantiated_higher_order_creators_with_recursive_results() {
        let directory = fixture_dir();
        fs::write(
            directory.join("factory.telora"),
            r#"@struct type Model(Subject, Output) = {subject: Subject, output: Output};
               def apply: for(Input, Output) Fn(Input, Fn(Input) -> Output) -> Output =
                   fn(input, callback) { callback(input) };
               export def make_creator:
                   for(Subject, Output)
                   Fn(Model(Subject, Output)) -> Fn(Subject) -> Output =
                   fn(model) { fn(subject) { model.output } };
               export def make_composed_creator:
                   for(Subject, Output)
                   Fn(Model(Subject, Output)) -> Fn(Subject) -> Output =
                   fn(model) {
                       fn(subject) { apply(subject, fn(current) { model.output }) }
                   };
               export { Model };"#,
        )
        .unwrap();
        fs::write(
            directory.join("domain.telora"),
            r#"import "./factory.telora" {Model, make_creator, make_composed_creator};
               @enum type Subject = {Order: 'None};
               @struct type CallExpr = {name: String, args: Array(Expr)};
               @enum type Expr = {Subject: Subject, Call: CallExpr};
               let model: Model(Subject, Expr) = {
                   subject: 'Order,
                   output: 'Call({name: "root", args: ['Subject('Order)]}),
               };
               export let creator = make_creator(model);
               export let composed_creator = make_composed_creator(model);"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./domain.telora" {creator, composed_creator};
               (creator('Order), composed_creator('Order))"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let result_type = module.analysis.display(module.analysis.result_type);
        assert!(result_type.contains("Array<Expr>"), "{result_type}");
        assert!(!result_type.contains("Any"), "{result_type}");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "('Call({args: ['Subject('Order)], name: \"root\"}), 'Call({args: ['Subject('Order)], name: \"root\"}))"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_imported_local_bindings_without_creating_local_aliases() {
        let directory = fixture_dir();
        fs::write(
            directory.join("origin.telora"),
            r#"@struct type Box(A) = {value: A};
               @struct type Branch = {children: Array(Tree)};
               @enum type Tree = {Leaf: Int, Branch: Branch};
               export def identity: for(A) Fn(A) -> A = fn(value) { value };
               export {Box, Tree};"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./origin.telora" {Box as Container, Tree, identity};
               export {Container as Box, Tree, identity};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade.telora" {Box, Tree, identity};
               type TreeBox = Box(Tree);
               export let output: TreeBox = identity({value: 'Leaf(1)});"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let alias = module
            .analysis
            .display(module.analysis.declared_types["TreeBox"]);
        assert!(
            alias.contains("Array<Tree>") || alias.contains("recursive"),
            "{alias}"
        );
        assert!(!alias.contains("Any"), "{alias}");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{output: {value: 'Leaf(1)}}"
        );

        fs::write(
            directory.join("invalid-local.telora"),
            r#"let a = 1; export {a as b}; export let output = b;"#,
        )
        .unwrap();
        let error = load_module(
            directory.join("invalid-local.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unknown binding \"b\""), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_imported_namespace_as_a_semantic_module() {
        let directory = fixture_dir();
        fs::write(
            directory.join("origin.telora"),
            r#"@struct type Box(A) = {value: A};
               export def identity: for(A) Fn(A) -> A = fn(value) { value };
               export {Box};"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./origin.telora" as model; export {model};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade.telora" {model};
               type IntBox = model.Box(Int);
               export let output: IntBox = model.identity({value: 1});
               export let polymorphic = (model.identity(1), model.identity("x"));"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{output: {value: 1}, polymorphic: (1, \"x\")}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_open_imported_locals_through_multihop_facades() {
        let directory = fixture_dir();
        fs::write(
            directory.join("origin.telora"),
            r#"export let value = 7;
               export def identity: for(A) Fn(A) -> A = fn(item) { item };"#,
        )
        .unwrap();
        fs::write(
            directory.join("first.telora"),
            r#"import "./origin.telora" *;
               export {value as answer, identity};"#,
        )
        .unwrap();
        fs::write(
            directory.join("second.telora"),
            r#"import "./first.telora" {answer, identity as relay};
               export {answer, relay as identity};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./origin.telora" {identity as direct};
               import "./second.telora" {answer, identity};
               export let output = {
                   answer,
                   same: direct == identity,
                   values: (identity(1), identity("x")),
               };"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{output: {answer: 7, same: 'True, values: (1, \"x\")}}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_imported_opaque_types_with_provider_identity() {
        let directory = fixture_dir();
        fs::write(
            directory.join("facade.telora"),
            r#"import "std/hash" {HashState}; export {HashState as State};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade.telora" {State};
               import "std/type-desc" as desc;
               export let output = {
                   kind: desc.kind(State),
                   name: desc.opaque_name(State),
               };"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{output: {kind: 'Opaque, name: 'Some(\"std/hash#HashState\")}}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_generic_apis_widen_singleton_fields_in_anonymous_records() {
        let directory = fixture_dir();
        fs::write(
            directory.join("api.telora"),
            r#"def use: for(Req, Node) Fn(Array(Req), Fn(Req) -> Node) -> Node =
                   fn(requirements, selector) { selector(requirements[0]) };
               {use}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./api.telora" as api;
               @enum type Node = {A: 'None, B: 'None};
               @struct type Requirement = {target: Node};
               def target_of: Fn(Requirement) -> Node = fn(req) { req.target };
               api.use([{target: 'B}], target_of)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "enum {A, B}"
        );
        assert_eq!(module.execute(100_000).unwrap().to_string(), "'B");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_generic_apis_refine_option_results_of_let_bound_callbacks() {
        let directory = fixture_dir();
        fs::write(
            directory.join("api.telora"),
            r#"def apply: for(A, B) Fn(A, Fn(A) -> Option(B)) -> Option(B) =
                   fn(value, callback) { callback(value) };
               {apply}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./api.telora" as api;
               let build = fn(value) {
                   if value > 0 { 'Some("ok") } else { 'None }
               };
               api.apply(1, build)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "enum {None, Some(String)}"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "'Some(\"ok\")"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parameterized_type_families_preserve_attributes_and_codec_rules() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/attributes" as attributes;
               type Box(Item) = attributes.add(
                   struct('None, {
                       value: attributes.add(
                           Item,
                           { "std/json.rename": "payload" },
                       ),
                   }),
                   { "std/json.rename_all": 'CamelCase },
               );
               {
                   metadata: Box(String),
                   decoded: codec.decode(Box(String), {payload: "ready"}),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected Dict result")
        };
        assert!(
            result
                .get("decoded")
                .unwrap()
                .to_string()
                .starts_with("'Ok("),
            "{}",
            result.get("decoded").unwrap()
        );
        let Value::Dict(metadata) = result.get("metadata").unwrap() else {
            panic!("expected attributed family metadata")
        };
        assert_eq!(metadata.get("kind").unwrap().to_string(), "'WithAttributes");
        let Value::Dict(model_attributes) = metadata.get("attributes").unwrap() else {
            panic!("expected family model attributes")
        };
        assert_eq!(
            model_attributes
                .get("std/json.rename_all")
                .unwrap()
                .to_string(),
            "'CamelCase"
        );
        let Value::Dict(struct_metadata) = metadata.get("inner").unwrap() else {
            panic!("expected family Struct metadata")
        };
        let Value::Dict(fields) = struct_metadata.get("fields").unwrap() else {
            panic!("expected family Struct fields")
        };
        let Value::Dict(field) = fields.get("value").unwrap() else {
            panic!("expected attributed family field")
        };
        let Value::Dict(field_attributes) = field.get("attributes").unwrap() else {
            panic!("expected family field attributes")
        };
        assert_eq!(
            field_attributes.get("std/json.rename").unwrap().to_string(),
            "\"payload\""
        );

        fs::write(
            directory.join("family.telora"),
            r#"import "std/attributes" as attributes;
               type Box(Item) = attributes.add(
                   struct('None, {
                       value: attributes.add(
                           Item,
                           { "std/json.rename": "payload" },
                       ),
                   }),
                   { "std/json.rename_all": 'CamelCase },
               );
               export {Box};"#,
        )
        .unwrap();
        fs::write(
            directory.join("imported.telora"),
            r#"import "std/codec" as codec;
               import "./family.telora" {Box};
               type StringBox = Box(String);
               codec.decode(StringBox, {payload: "imported"})"#,
        )
        .unwrap();
        let imported =
            load_module(directory.join("imported.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            imported.execute(100_000).unwrap().to_string(),
            "'Ok({value: \"imported\"})"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn local_concrete_family_dependencies_preserve_metadata_and_match_imports() {
        let directory = fixture_dir();
        fs::write(
            directory.join("attributes.telora"),
            r#"import "std/attributes" as attributes;
               type Local = attributes.add(Int, {marker: "local"});
               type Pair(A) = Tuple([Local, A]);
               {direct: Local, captured: Pair(String)}"#,
        )
        .unwrap();
        let module = load_module(
            directory.join("attributes.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected metadata result")
        };
        let Value::Dict(captured) = result.get("captured").unwrap() else {
            panic!("expected Tuple metadata")
        };
        let Value::Array(items) = captured.get("items").unwrap() else {
            panic!("expected Tuple items")
        };
        assert_eq!(
            items[0].to_string(),
            result.get("direct").unwrap().to_string()
        );

        fs::write(
            directory.join("base.telora"),
            "@enum type Status = {Ready: 'None}; {Status: Status}",
        )
        .unwrap();
        fs::write(
            directory.join("local.telora"),
            "@enum type Status = {Ready: 'None};\
             @struct type Box(A) = {status: Status, value: A};\
             {Box: Box}",
        )
        .unwrap();
        fs::write(
            directory.join("imported.telora"),
            "import \"./base.telora\" {Status};\
             @struct type Box(A) = {status: Status, value: A};\
             {Box: Box}",
        )
        .unwrap();
        fs::write(
            directory.join("compare.telora"),
            "import \"./local.telora\" as local;\
             import \"./imported.telora\" as imported;\
             (local.Box(Int), imported.Box(Int))",
        )
        .unwrap();
        let module =
            load_module(directory.join("compare.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Tuple(items) = module.execute(100_000).unwrap() else {
            panic!("expected metadata pair")
        };
        assert_eq!(items[0].to_string(), items[1].to_string());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parameterized_type_family_applications_preserve_authored_rule_provenance() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let data = directory.join("data.json");
        fs::write(&data, r#"{"value":42}"#).unwrap();
        fs::write(
            &main,
            r#"import "./data.json" as data;
               import "std/codec" as codec;
               import "std/result" as result;
               @struct type Box(Item) = {value: Item};
               codec.decode(Box(String), data) |> result.unwrap"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module.execute(100_000).unwrap_err();
        assert!(failure.message.contains("$.value"), "{}", failure.message);
        let data_location = failure.data_location().expect("codec data location");
        assert_eq!(
            module.sources.get(data_location.source).name.as_ref(),
            data.display().to_string()
        );
        let rule_location = failure.rule_location().expect("codec rule location");
        assert_eq!(
            module.sources.get(rule_location.source).name.as_ref(),
            "@standalone/main.telora"
        );
        assert!(
            module
                .sources
                .get(rule_location.source)
                .slice(rule_location)
                .is_some_and(|rule| rule.contains("String")),
            "rule location: {rule_location:?}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_callbacks_share_fuel_allocation_and_tool_stage_execution() {
        let directory = fixture_dir();
        let item_count = 1_500usize;
        let data = format!("[{}]", vec!["1"; item_count].join(","));
        fs::write(directory.join("values.json"), data).unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               import "./values.json" as values;
               arrays.map(values, fn(value) { value + 1 })"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();

        let mut exact = QuotaAccount::new(Quota::new(1_501, 1_000, u64::MAX));
        let arena = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut exact,
            )
            .unwrap();
        assert_eq!(
            exact.requested_allocation_bytes(),
            item_count as u64 * std::mem::size_of::<Value>() as u64
        );
        let Value::Array(mapped) = arena.export(&module.runtime.main.heap).unwrap() else {
            panic!("expected mapped Array")
        };
        assert_eq!(mapped.len(), item_count);

        let mut fuel_short = QuotaAccount::new(Quota::new(1_500, 1_000, u64::MAX));
        assert_eq!(
            Vm::new()
                .execute_in_work(
                    &module.runtime.main.heap,
                    &module.runtime.externals,
                    &module.function,
                    &[],
                    &mut fuel_short,
                )
                .err()
                .expect("fuel must be exhausted")
                .kind,
            crate::RuntimeErrorKind::FuelExhausted
        );

        let requested = item_count as u64 * std::mem::size_of::<Value>() as u64;
        let mut allocation_short = QuotaAccount::new(Quota::new(1_501, 1_000, requested - 1));
        assert_eq!(
            Vm::new()
                .execute_in_work(
                    &module.runtime.main.heap,
                    &module.runtime.externals,
                    &module.function,
                    &[],
                    &mut allocation_short,
                )
                .err()
                .expect("allocation must be exhausted")
                .kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        fs::write(
            directory.join("types.telora"),
            r#"import "std/array" as arrays;
               type Pair = Tuple(arrays.map([Int, String], fn(item) { item }));
               let pair: Pair = (1, "one");
               pair"#,
        )
        .unwrap();
        let types = load_module(directory.join("types.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(types.execute(100_000).unwrap().to_string(), "(1, \"one\")");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_reports_boundary_and_callback_result_errors() {
        let directory = fixture_dir();
        let analysis_error = |name: &str, expression: &str| {
            let path = directory.join(name);
            fs::write(
                &path,
                format!("import \"std/array\" as arrays; {expression}"),
            )
            .unwrap();
            load_module(path, BTreeMap::new(), 100_000).unwrap_err()
        };
        let run_error = |name: &str, expression: &str| {
            let path = directory.join(name);
            fs::write(
                &path,
                format!("import \"std/array\" as arrays; {expression}"),
            )
            .unwrap();
            let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
            module.execute(100_000).unwrap_err()
        };

        assert!(
            analysis_error("length.telora", "arrays.length(1)")
                .to_string()
                .contains("cannot unify Int with Array")
        );
        assert!(
            analysis_error("get-index.telora", "arrays.get([1], \"first\")")
                .to_string()
                .contains("cannot unify String with Int")
        );
        assert!(
            analysis_error("enumerate.telora", "arrays.enumerate(1)")
                .to_string()
                .contains("cannot unify Int with Array")
        );
        assert!(
            analysis_error("push.telora", "arrays.push([1], \"wrong\")")
                .to_string()
                .contains("cannot unify")
        );
        assert!(
            analysis_error("arity.telora", "arrays.map([1], fn(a, b) { a + b })")
                .to_string()
                .contains("cannot unify")
        );
        assert!(
            analysis_error("filter.telora", "arrays.filter([1], fn(value) { value })")
                .to_string()
                .contains("cannot unify Int with enum {False, True}")
        );
        assert!(
            analysis_error(
                "flat-map.telora",
                "arrays.flat_map([1], fn(value) { value })"
            )
            .to_string()
            .contains("cannot unify Int with Array")
        );
        let callback = run_error(
            "callback.telora",
            "arrays.map([1], fn(value) { value / 0 })",
        );
        assert!(callback.to_string().contains("callback.telora:1:"));
        assert!(
            callback
                .trace
                .iter()
                .any(|frame| frame.function == "std/array.map")
        );
        let dynamic_get = run_error(
            "dynamic-get.telora",
            "let index: Any = \"first\"; arrays.get([1], index)",
        );
        assert_eq!(dynamic_get.kind, crate::RuntimeErrorKind::TypeMismatch);
        assert!(dynamic_get.message.contains("Int"));

        let nested_depth = run_error(
            "nested-depth.telora",
            "decl nest: Fn(Int) -> Int;
             def nest = fn(n) {
                 if n < 1 { 0 } else {
                     arrays.fold([n], 0, fn(total, value) { nest(value - 1) })
                 }
             };
             nest(1100)",
        );
        assert_eq!(
            nested_depth.kind,
            crate::RuntimeErrorKind::CallDepthExceeded
        );

        let unknown_path = directory.join("unknown-core.telora");
        fs::write(&unknown_path, "import \"std/unknown\" as unknown; unknown").unwrap();
        assert!(
            load_module(unknown_path, BTreeMap::new(), 100_000)
                .unwrap_err()
                .to_string()
                .contains("unknown dependency")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn array_push_preserves_existing_and_appended_value_provenance() {
        let directory = fixture_dir();
        let data = directory.join("data.json");
        let main = directory.join("main.telora");
        let source = r#"import "std/array" as arrays;
                        import "std/result" as result;
                        import "./data.json" as data;
                        let values = arrays.push(data, APPENDED);
                        arrays.map(values, fn(value) {
                            if value == TARGET {
                                fail!("selected value", value)
                            } else { value }
                        })"#;

        fs::write(&data, "[1]").unwrap();
        fs::write(
            &main,
            source.replace("APPENDED", "2").replace("TARGET", "1"),
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let existing = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(existing.contains("data.json:1:2:"), "{existing}");

        fs::write(
            &main,
            source.replace("APPENDED", "2").replace("TARGET", "2"),
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let appended = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(appended.contains("main.telora:4:"), "{appended}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn array_push_charges_the_complete_logical_result() {
        let directory = fixture_dir();
        fs::write(directory.join("values.json"), "[1, 2]").unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               import "./values.json" as values;
               arrays.push(values, 3)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let requested = 3 * std::mem::size_of::<Value>() as u64;
        let mut exact = QuotaAccount::new(Quota::new(1, 1_000, requested));
        let result = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut exact,
            )
            .unwrap()
            .export(&module.runtime.main.heap)
            .unwrap();
        assert_eq!(result.to_string(), "[1, 2, 3]");
        assert_eq!(exact.requested_allocation_bytes(), requested);

        let mut short = QuotaAccount::new(Quota::new(1, 1_000, requested - 1));
        let failure = match Vm::new().execute_in_work(
            &module.runtime.main.heap,
            &module.runtime.externals,
            &module.function,
            &[],
            &mut short,
        ) {
            Ok(_) => panic!("allocation must be exhausted"),
            Err(error) => error,
        };
        assert_eq!(
            failure.kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn array_get_and_enumerate_obey_exact_allocation_and_tool_stage_contracts() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(directory.join("values.json"), "[10, 20]").unwrap();
        let load = |expression: &str| {
            fs::write(
                &main,
                format!(
                    "import \"std/array\" as arrays;\nimport \"./values.json\" as values;\n{expression}"
                ),
            )
            .unwrap();
            load_module(&main, BTreeMap::new(), 100_000).unwrap()
        };
        let value_bytes = std::mem::size_of::<Value>() as u64;

        let some = load("arrays.get(values, 1)");
        let mut exact_some = QuotaAccount::new(Quota::new(1, 1_000, 2 * value_bytes));
        let result = Vm::new()
            .execute_in_work(
                &some.runtime.main.heap,
                &some.runtime.externals,
                &some.function,
                &[],
                &mut exact_some,
            )
            .unwrap()
            .export(&some.runtime.main.heap)
            .unwrap();
        assert_eq!(result.to_string(), "'Some(20)");
        assert_eq!(exact_some.requested_allocation_bytes(), 2 * value_bytes);
        let mut short_some = QuotaAccount::new(Quota::new(1, 1_000, 2 * value_bytes - 1));
        let failure = match Vm::new().execute_in_work(
            &some.runtime.main.heap,
            &some.runtime.externals,
            &some.function,
            &[],
            &mut short_some,
        ) {
            Ok(_) => panic!("allocation must be exhausted"),
            Err(error) => error,
        };
        assert_eq!(
            failure.kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        let none = load("arrays.get(values, -1)");
        let mut no_allocation = QuotaAccount::new(Quota::new(1, 1_000, 0));
        let result = Vm::new()
            .execute_in_work(
                &none.runtime.main.heap,
                &none.runtime.externals,
                &none.function,
                &[],
                &mut no_allocation,
            )
            .unwrap()
            .export(&none.runtime.main.heap)
            .unwrap();
        assert_eq!(result.to_string(), "'None");
        assert_eq!(no_allocation.requested_allocation_bytes(), 0);

        let enumerate = load("arrays.enumerate(values)");
        let requested = 6 * value_bytes;
        let mut exact_enumerate = QuotaAccount::new(Quota::new(1, 1_000, requested));
        let result = Vm::new()
            .execute_in_work(
                &enumerate.runtime.main.heap,
                &enumerate.runtime.externals,
                &enumerate.function,
                &[],
                &mut exact_enumerate,
            )
            .unwrap()
            .export(&enumerate.runtime.main.heap)
            .unwrap();
        assert_eq!(result.to_string(), "[(0, 10), (1, 20)]");
        assert_eq!(exact_enumerate.requested_allocation_bytes(), requested);
        let mut short_enumerate = QuotaAccount::new(Quota::new(1, 1_000, requested - 1));
        let failure = match Vm::new().execute_in_work(
            &enumerate.runtime.main.heap,
            &enumerate.runtime.externals,
            &enumerate.function,
            &[],
            &mut short_enumerate,
        ) {
            Ok(_) => panic!("allocation must be exhausted"),
            Err(error) => error,
        };
        assert_eq!(
            failure.kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        fs::write(
            directory.join("types.telora"),
            r#"import "std/array" as arrays;
               type Pair = Tuple(arrays.map(
                   arrays.enumerate([String, Int]),
                   fn(entry) { let (index, item) = entry; item },
               ));
               let pair: Pair = ("ten", 10);
               pair"#,
        )
        .unwrap();
        let types = load_module(directory.join("types.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            types.analysis.display(types.analysis.result_type),
            "(String, Int)"
        );
        assert_eq!(types.execute(100_000).unwrap().to_string(), "(\"ten\", 10)");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn array_get_and_enumerate_preserve_element_and_call_provenance() {
        let directory = fixture_dir();
        let data = directory.join("data.json");
        let main = directory.join("main.telora");
        fs::write(&data, "[10]").unwrap();

        fs::write(
            &main,
            r#"import "std/array" as arrays;
               import "std/result" as result;
               import "./data.json" as data;
               match arrays.get(data, 0) {
                   'Some(value) => fail!("selected", value),
                   'None => 0,
               }"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(failure.contains("data.json:1:2:"), "{failure}");

        fs::write(
            &main,
            r#"import "std/array" as arrays;
               import "std/result" as result;
               import "./data.json" as data;
               let indexed = arrays.enumerate(data);
               arrays.map(indexed, fn(entry) {
                   let (index, value) = entry;
                   if value == 10 {
                       fail!("selected", value)
                   } else { index }
               })"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(failure.contains("data.json:1:2:"), "{failure}");

        fs::write(
            &main,
            r#"import "std/array" as arrays;
               import "std/result" as result;
               import "./data.json" as data;
               let indexed = arrays.enumerate(data);
               let first = arrays.get(indexed, 0);
               match first {
                   'Some((index, value)) => fail!("index", index),
                   'None => 0,
               }"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(failure.contains("main.telora:4:"), "{failure}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_option_and_result_combinators_are_generic_telora_definitions() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/option" as options;
               import "std/result" as results;
               let ok: Result(Int, String) = 'Ok(3);
               let err: Result(Int, String) = 'Err("bad");
               {
                   option_map: options.map('Some(2), fn(value) { value + 1 }),
                   option_map_none: options.map('None, fn(value) { value / 0 }),
                   option_flat_map: options.flat_map('Some(2), fn(value) { 'Some(value + 2) }),
                   option_flat_none: options.flat_map('None, fn(value) { 'Some(value / 0) }),
                   option_some_or: options.unwrap_or('Some(4), 9),
                   option_none_or: options.unwrap_or('None, 9),
                   option_is_some: options.is_some('Some("x")),
                   option_is_none: options.is_some('None),
                   result_map: results.map(ok, fn(value) { value + 1 }),
                   result_map_err: results.map(err, fn(value) { value / 0 }),
                   result_err_map: results.map_err(err, fn(error) { error }),
                   result_err_map_ok: results.map_err(ok, fn(error) { error }),
                   result_ok_or: results.unwrap_or(ok, 9),
                   result_err_or: results.unwrap_or(err, 9),
                   result_is_ok: results.is_ok(ok),
                   result_is_err: results.is_ok(err),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected combinator results")
        };
        let expected = [
            ("option_map", "'Some(3)"),
            ("option_map_none", "'None"),
            ("option_flat_map", "'Some(4)"),
            ("option_flat_none", "'None"),
            ("option_some_or", "4"),
            ("option_none_or", "9"),
            ("option_is_some", "'True"),
            ("option_is_none", "'False"),
            ("result_map", "'Ok(4)"),
            ("result_map_err", "'Err(\"bad\")"),
            ("result_err_map", "'Err(\"bad\")"),
            ("result_err_map_ok", "'Ok(3)"),
            ("result_ok_or", "3"),
            ("result_err_or", "9"),
            ("result_is_ok", "'True"),
            ("result_is_err", "'False"),
        ];
        for (name, expected) in expected {
            assert_eq!(result.get(name).unwrap().to_string(), expected, "{name}");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typed_metadata_witnesses_flow_through_codec_and_validation_boundaries() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/result" as result;
               @struct type User = {name: String};
               let decoded = codec.decode(User, {name: "Ada"});
               let encoded = codec.encode(User, {name: "Lin"});
               let checked = validate(User, {name: "Grace"});
               let invalid = validate(User, {name: 1});
               let formatted = result.map_err(
                   codec.decode(User, {name: 1}),
                   codec.format_error,
               );
               let chained = result.flat_map(
                   codec.decode(User, {name: "Mira"}),
                   fn(user) { validate(User, user) },
               );
               let name = result.unwrap(result.map(
                   codec.decode(User, {name: "Kai"}),
                   fn(user) { user.name },
               ));
               {
                   decoded: decoded,
                   encoded: encoded,
                   checked: checked,
                   invalid: invalid,
                   formatted: formatted,
                   chained: chained,
                   name: name,
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["decoded"]),
            "enum {Err({data: Any, message: String, rule: Any}), Ok({name: String})}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["checked"]),
            "enum {Err({data: Any, message: String, rule: Any}), Ok({name: String})}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["encoded"]),
            "enum {Err({data: Any, message: String, rule: Any}), Ok(Any)}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["formatted"]),
            "enum {Err(String), Ok({name: String})}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["chained"]),
            "enum {Err({data: Any, message: String, rule: Any}), Ok({name: String})}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["name"]),
            "String"
        );
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("typed boundary test must return a Dict")
        };
        assert_eq!(output.get("name").unwrap().to_string(), "\"Kai\"");
        assert!(
            output
                .get("formatted")
                .unwrap()
                .to_string()
                .contains("expected String")
        );
        let Value::Tagged { tag, payload } = output.get("invalid").unwrap() else {
            panic!("invalid validation must return a Result")
        };
        assert_eq!(tag.name(), "Err");
        let Value::Dict(error) = payload.as_ref() else {
            panic!("validation failure must be a structured error")
        };
        assert!(
            error
                .get("message")
                .unwrap()
                .to_string()
                .contains("must be String")
        );
        assert_eq!(error.get("data").unwrap().to_string(), "{name: 1}");
        assert!(error.get("rule").unwrap().to_string().contains("'Struct"));

        fs::write(
            directory.join("wrong-encode.telora"),
            r#"import "std/codec" as codec;
               @struct type User = {name: String};
               codec.encode(User, {name: 1})"#,
        )
        .unwrap();
        let error = load_module(
            directory.join("wrong-encode.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot unify Int with String"));

        fs::write(
            directory.join("erased.telora"),
            "let metadata: Type = Int; validate(metadata, 1)",
        )
        .unwrap();
        let error =
            load_module(directory.join("erased.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("TypeOf"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_dict_enumerates_constructs_and_merges_in_canonical_order() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dict" as dicts;
               let source = { z: 3, a: 1, middle: 2 };
               {
                   keys: dicts.keys(source),
                   values: dicts.values(source),
                   pairs: dicts.pairs(source),
                   round_trip: dicts.from_pairs(dicts.pairs(source)),
                   merged: dicts.merge(
                       { a: 1, nested: { left: 1 } },
                       { b: 2, nested: { right: 2 } },
                   ),
                   empty_keys: dicts.keys({}),
                   empty_pairs: dicts.pairs({}),
                   empty_from_pairs: dicts.from_pairs([]),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected Dict result")
        };
        assert_eq!(
            result.get("keys").unwrap().to_string(),
            "[\"a\", \"middle\", \"z\"]"
        );
        assert_eq!(result.get("values").unwrap().to_string(), "[1, 2, 3]");
        assert_eq!(
            result.get("pairs").unwrap().to_string(),
            "[(\"a\", 1), (\"middle\", 2), (\"z\", 3)]"
        );
        assert_eq!(
            result.get("round_trip").unwrap().to_string(),
            "{a: 1, middle: 2, z: 3}"
        );
        assert_eq!(
            result.get("merged").unwrap().to_string(),
            "{a: 1, b: 2, nested: {right: 2}}"
        );
        assert_eq!(result.get("empty_keys").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_pairs").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_from_pairs").unwrap().to_string(), "{}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typed_dict_lookup_and_argv_rewrites_compose_in_user_code() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dict" as dict;
               import "std/argv" as argv;
               let environment: Dict(String) = {TARGET: "aarch64-linux-gnu"};
               let target: Option(String) = dict.get(environment, "TARGET");
               let missing: Option(String) = dict.get(environment, "MISSING");
               def rewrite:
                   Fn(Array(String)) -> Result(Array(String), String) = fn(arguments) {
                       let arguments = argv.reject_option(arguments, "--sysroot")?;
                       let arguments = argv.reject_option(arguments, "-fdebug-prefix-map")?;
                       'Ok(argv.prepend(
                           ["--sysroot=/sdk", "-fdebug-prefix-map=/work=."],
                           arguments,
                       ))
                   };
               export let output = {
                   target,
                   missing,
                   exact: [
                       argv.matches_option("--sysroot", "--sysroot"),
                       argv.matches_option("--sysroot=/x", "--sysroot"),
                       argv.matches_option("--sysrooted", "--sysroot"),
                   ],
                   contains: argv.contains_option(["-c", "--sysroot=/x"], "--sysroot"),
                   rewritten: rewrite(["-c", "main.c"]),
                   rejected: rewrite(["-c", "--sysroot=/other"]),
               };"#,
        )
        .unwrap();

        let engine = recovery_engine();
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        let output = named_output(engine.execute(&module).unwrap());
        let Value::Dict(output) = output else {
            panic!("expected output Dict")
        };
        assert_eq!(
            output.get("target").unwrap().to_string(),
            "'Some(\"aarch64-linux-gnu\")"
        );
        assert_eq!(output.get("missing").unwrap().to_string(), "'None");
        assert_eq!(
            output.get("exact").unwrap().to_string(),
            "['True, 'True, 'False]"
        );
        assert_eq!(output.get("contains").unwrap().to_string(), "'True");
        assert_eq!(
            output.get("rewritten").unwrap().to_string(),
            "'Ok([\"--sysroot=/sdk\", \"-fdebug-prefix-map=/work=.\", \"-c\", \"main.c\"])",
        );
        let rejected = output.get("rejected").unwrap().to_string();
        assert!(rejected.contains("'Err("), "{rejected}");
        assert!(
            rejected.contains("conflicting argument: --sysroot"),
            "{rejected}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_type_desc_exposes_erased_kinds_and_structured_ref_errors() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/type-desc" as desc;
               import "std/attributes" as attributes;
               import "std/array" as arrays;
               @struct type Node = {children: Array(Node)};
               let struct_nodes = desc.children(Node);
               let field_nodes = arrays.flat_map(struct_nodes, desc.children);
               let array_nodes = arrays.flat_map(field_nodes, desc.children);
               let refs = arrays.flat_map(array_nodes, desc.children);
               {
                   int_kind: desc.kind(Int),
                   func_kind: desc.kind(Func([Int], String)),
                   attributed_kind: desc.kind(attributes.add(Int, {doc: "number"})),
                   resolve_int: desc.resolve(Int),
                   ref_kinds: arrays.map(refs, desc.kind),
                   resolved_kinds: arrays.map(refs, fn(reference) {
                       match desc.resolve(reference) {
                           'Ok(target) => desc.kind(target),
                           'Err(_) => 'Never,
                       }
                   }),
                   TypeDesc: desc.TypeDesc,
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("type descriptor test must return a Dict")
        };
        assert_eq!(output.get("int_kind").unwrap().to_string(), "'Int");
        assert_eq!(output.get("func_kind").unwrap().to_string(), "'Func");
        assert_eq!(
            output.get("attributed_kind").unwrap().to_string(),
            "'WithAttributes"
        );
        assert_eq!(output.get("TypeDesc").unwrap().to_string(), "{kind: 'Type}");
        assert_eq!(output.get("ref_kinds").unwrap().to_string(), "['Ref]");
        assert_eq!(
            output.get("resolved_kinds").unwrap().to_string(),
            "['WithAttributes]"
        );
        let Value::Tagged { tag, payload } = output.get("resolve_int").unwrap() else {
            panic!("resolve must return a Result")
        };
        assert_eq!(tag.name(), "Err");
        let Value::Dict(error) = payload.as_ref() else {
            panic!("resolve error must be structured")
        };
        assert_eq!(
            error.get("message").unwrap().to_string(),
            "\"type descriptor is not a recursive reference\""
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn opaque_type_is_nominal_reflectable_and_not_codec_data() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/type-desc" as desc;
               import "std/hash" as hash;
               {
                   kind: desc.kind(hash.HashState),
                   children: desc.children(hash.HashState),
                   name: desc.opaque_name(hash.HashState),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("opaque type test must return a Dict")
        };
        assert_eq!(output.get("kind").unwrap().to_string(), "'Opaque");
        assert_eq!(output.get("children").unwrap().to_string(), "[]");
        assert_eq!(
            output.get("name").unwrap().to_string(),
            "'Some(\"std/hash#HashState\")"
        );
        fs::write(
            directory.join("main.telora"),
            r#"import "std/json" as json;
               import "std/hash" as hash;
               json.decode(hash.HashState, "1")"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Tagged { tag, payload } = module.execute(100_000).unwrap() else {
            panic!("opaque JSON decode must return a Result")
        };
        assert_eq!(tag.name(), "Err");
        let Value::Dict(error) = payload.as_ref() else {
            panic!("opaque JSON decode error must be structured")
        };
        assert_eq!(
            error.get("message").unwrap().to_string(),
            "\"$: Opaque has no JSON codec\""
        );
        fs::write(
            directory.join("main.telora"),
            r#"import "std/json" as json;
               import "std/hash" as hash;
               json.schema(hash.HashState)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Opaque has no JSON Schema mapping")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_hash_state_is_persistent_and_follows_the_versioned_protocol() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/hash" as hash;
               let empty = hash.new();
               let string = hash.update_string(empty, "abc");
               let bytes = hash.update_bytes(empty, b"abc");
               let integer = hash.update_int(empty, -1);
               {
                   empty: hash.finish(empty),
                   empty_again: hash.finish(empty),
                   string: hash.finish(string),
                   bytes: hash.finish(bytes),
                   integer: hash.finish(integer),
                   empty_unchanged: empty == hash.new(),
                   kinds_differ: string == bytes,
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("hash state test must return a Dict")
        };
        let digest = |name: &str| {
            let Value::Bytes(bytes) = output.get(name).unwrap() else {
                panic!("{name} must be Bytes")
            };
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert_eq!(
            digest("empty"),
            "60ec81853a8626c6656e854de787935ca1d364e6b35721c911bb52f5ab6848e0"
        );
        assert_eq!(digest("empty_again"), digest("empty"));
        assert_eq!(
            digest("string"),
            "74365801acb81e0b715feefcc7f61d2ae2b69ca2db302ac8da1d4905903a2357"
        );
        assert_eq!(
            digest("bytes"),
            "8153fe52a36d2948a281e929455aca2f565b95c43b7b1731ac471a49d23ec1cd"
        );
        assert_eq!(
            digest("integer"),
            "322aacdbd881f3ea5904156d8e4030a2936e085ff92cd272ae99378207eb7d34"
        );
        assert_eq!(output.get("empty_unchanged").unwrap().to_string(), "'True");
        assert_eq!(output.get("kinds_differ").unwrap().to_string(), "'False");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_dyn_packs_projects_and_publishes_opaque_values() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dyn" as dyn;
               let int_value = dyn.pack(Int, 41);
               let string_value = dyn.pack(String, "text");
               let float_value = dyn.pack(Float, 1.5);
               let bytes_value = dyn.pack(Bytes, b"ab");
               type Unary = Func([Int], Int);
               let identity: Fn(Int) -> Int = fn(value) { value };
               let func_value = dyn.pack(Unary, identity);
               let captured = fn() { int_value };
               {
                   int_type: dyn.desc(int_value),
                   int_kind: dyn.kind(int_value),
                   func_kind: dyn.kind(func_value),
                   int_value: dyn.check_int(int_value),
                   wrong_value: dyn.check_string(int_value),
                   string_value: dyn.check_string(string_value),
                   float_value: dyn.check_float(float_value),
                   bytes_value: dyn.check_bytes(bytes_value),
                   same_identity: int_value == int_value,
                   different_identity: int_value == dyn.pack(Int, 41),
                   values: [captured(), string_value],
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["int_value"]),
            "Dyn"
        );
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("Dyn test must return a Dict")
        };
        assert_eq!(output.get("int_type").unwrap().to_string(), "{kind: 'Int}");
        assert_eq!(output.get("int_kind").unwrap().to_string(), "'Int");
        assert_eq!(output.get("func_kind").unwrap().to_string(), "'Func");
        assert_eq!(output.get("int_value").unwrap().to_string(), "'Some(41)");
        assert_eq!(output.get("wrong_value").unwrap().to_string(), "'None");
        assert_eq!(
            output.get("string_value").unwrap().to_string(),
            "'Some(\"text\")"
        );
        assert_eq!(output.get("float_value").unwrap().to_string(), "'Some(1.5)");
        assert_eq!(
            output.get("bytes_value").unwrap().to_string(),
            "'Some(b\"\\x61\\x62\")"
        );
        assert_eq!(output.get("same_identity").unwrap().to_string(), "'True");
        assert_eq!(
            output.get("different_identity").unwrap().to_string(),
            "'False"
        );
        assert_eq!(output.get("values").unwrap().to_string(), "[<dyn>, <dyn>]");

        fs::write(
            directory.join("invalid.telora"),
            r#"import "std/dyn" as dyn;
               dyn.pack@[Int](Int, "wrong")"#,
        )
        .unwrap();
        let error =
            load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("cannot unify String with Int"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_dyn_structural_observers_preserve_child_descriptors() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dyn" as dyn;
               import "std/result" as result;
               import "std/array" as arrays;
               @struct type User = {name: String, pair: Tuple([Int, String])};
               @struct type Node = {value: Int, children: Array(Node)};
               @enum type Maybe = {None: 'None, Some: Int};
               let user = dyn.pack(User, {name: "Ada", pair: (1, "one")});
               let name = result.unwrap(dyn.field(user, "name"));
               let pair = result.unwrap(dyn.field(user, "pair"));
               let dict = dyn.pack(Dict(Int), {a: 7});
               let numbers = dyn.pack(Array(Int), [1, 2]);
               let root = dyn.pack(Node, {
                   value: 1,
                   children: [{value: 2, children: []}],
               });
               let children = result.unwrap(dyn.field(root, "children"));
               let child_nodes = result.unwrap(dyn.array_items(children));
               {
                   name: dyn.check_string(name),
                   user_fields: arrays.map(result.unwrap(dyn.fields(user)), fn(pair) {
                       match pair { (name, value) => (name, dyn.kind(value)) }
                   }),
                   dict_fields: arrays.map(result.unwrap(dyn.fields(dict)), fn(pair) {
                       match pair { (name, value) => (name, dyn.kind(value)) }
                   }),
                   dict_value: dyn.check_int(result.unwrap(dyn.field(dict, "a"))),
                   array_values: arrays.map(
                       result.unwrap(dyn.array_items(numbers)),
                       dyn.check_int,
                   ),
                   tuple_values: arrays.map(
                       result.unwrap(dyn.tuple_items(pair)),
                       dyn.kind,
                   ),
                   enum_tag: result.unwrap(dyn.tag(dyn.pack(Maybe, 'Some(3)))),
                   enum_payload: match result.unwrap(dyn.payload(dyn.pack(Maybe, 'Some(3)))) {
                       'Some(value) => dyn.check_int(value),
                       'None => 'None,
                   },
                   unit_payload: result.unwrap(dyn.payload(dyn.pack(Maybe, 'None))),
                   recursive_values: arrays.map(child_nodes, fn(child) {
                       dyn.check_int(result.unwrap(dyn.field(child, "value")))
                   }),
                   missing: dyn.field(user, "missing"),
                   wrong_shape: dyn.array_items(user),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 200_000).unwrap();
        let Value::Dict(output) = module.execute(200_000).unwrap() else {
            panic!("structural Dyn test must return a Dict")
        };
        assert_eq!(output.get("name").unwrap().to_string(), "'Some(\"Ada\")");
        assert_eq!(
            output.get("user_fields").unwrap().to_string(),
            "[(\"name\", 'String), (\"pair\", 'Tuple)]"
        );
        assert_eq!(
            output.get("dict_fields").unwrap().to_string(),
            "[(\"a\", 'Int)]"
        );
        assert_eq!(output.get("dict_value").unwrap().to_string(), "'Some(7)");
        assert_eq!(
            output.get("array_values").unwrap().to_string(),
            "['Some(1), 'Some(2)]"
        );
        assert_eq!(
            output.get("tuple_values").unwrap().to_string(),
            "['Int, 'String]"
        );
        assert_eq!(output.get("enum_tag").unwrap().to_string(), "\"Some\"");
        assert_eq!(output.get("enum_payload").unwrap().to_string(), "'Some(3)");
        assert_eq!(output.get("unit_payload").unwrap().to_string(), "'None");
        assert_eq!(
            output.get("recursive_values").unwrap().to_string(),
            "['Some(2)]"
        );
        for field in ["missing", "wrong_shape"] {
            let Value::Tagged { tag, payload } = output.get(field).unwrap() else {
                panic!("observer failure must return Result")
            };
            assert_eq!(tag.name(), "Err");
            let Value::Dict(blame) = payload.as_ref() else {
                panic!("observer failure must contain BlameError")
            };
            assert!(blame.get("message").unwrap().to_string().len() > 4);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interpreter_keyword_lifts_erased_binary_consumers() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dyn" as dyn;
               def int_eq_i: Fn(Dyn, Dyn) -> Bool = fn(left, right) {
                   match dyn.check_int(left) {
                       'Some(a) => match dyn.check_int(right) {
                           'Some(b) => a == b,
                           'None => 'False,
                       },
                       'None => 'False,
                   }
               };
               def eq_fn: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool =
                   interpreter!(int_eq_i);
               {
                   equal: eq_fn@[Int](Int)(1, 1),
                   different: eq_fn@[Int](Int)(1, 2),
                   inferred: eq_fn(Int)(2, 2),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 200_000).unwrap();
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["eq_fn"]),
            "Fn(TypeOf(Any)) -> Fn(Any, Any) -> enum {False, True}"
        );
        let Value::Dict(output) = module.execute(200_000).unwrap() else {
            panic!("interpreter test must return a Dict")
        };
        assert_eq!(output.get("equal").unwrap().to_string(), "'True");
        assert_eq!(output.get("different").unwrap().to_string(), "'False");
        assert_eq!(output.get("inferred").unwrap().to_string(), "'True");

        fs::write(
            directory.join("invalid.telora"),
            r#"def bad_i: Fn(Dyn) -> Bool = fn(value) { 'True };
               def bad: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool =
                   interpreter!(bad_i);
               0"#,
        )
        .unwrap();
        let error =
            load_module(directory.join("invalid.telora"), BTreeMap::new(), 200_000).unwrap_err();
        assert!(error.to_string().contains("expects 1 arguments, found 2"));

        for (source, expected) in [
            (
                "def bad = interpreter!(eq_i); 0",
                "interpreter requires an explicit",
            ),
            (
                "def bad: for(A) Fn(A) -> Fn(A, A) -> Bool = interpreter!(eq_i); 0",
                "witness parameter 1",
            ),
            (
                "def bad: for(A) Fn(TypeOf(A)) -> Fn(Array(A)) -> Bool = interpreter!(eq_i); 0",
                "inner parameter 1 contains type parameter A",
            ),
            (
                "def bad: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> A = interpreter!(eq_i); 0",
                "result contains type parameter A",
            ),
            (
                "def bad: for(A, B) Fn(TypeOf(A), TypeOf(A)) -> Fn(A) -> Bool = interpreter!(eq_i); 0",
                "type parameter A has more than one TypeOf witness",
            ),
            (
                "def bad: for(A, B) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter!(eq_i); 0",
                "type parameter B has no TypeOf witness",
            ),
            (
                "def bad: for(A) Fn(TypeOf(A)) -> Fn(Fn(A) -> Bool) -> Bool = interpreter!(eq_i); 0",
                "inner parameter 1 contains type parameter A",
            ),
            (
                "def bad: for(A) Fn(TypeOf(A)) -> Fn(A) -> Option(A) = interpreter!(eq_i); 0",
                "result contains type parameter A",
            ),
            (
                "let bad = interpreter!(eq_i); 0",
                "interpreter requires an explicit",
            ),
        ] {
            fs::write(directory.join("invalid-shape.telora"), source).unwrap();
            let error = load_module(
                directory.join("invalid-shape.telora"),
                BTreeMap::new(),
                200_000,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error}"
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn telora_equality_interpreter_matches_native_structural_equality() {
        let directory = fixture_dir();
        fs::write(
            directory.join("reference-equality.telora"),
            include_str!("../../../examples/reference-equality.telora"),
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./reference-equality.telora" as equality;
               import "std/eq" as eq;
               @struct type Node = {value: Int, children: Array(Node)};
               @enum type Choice = {None: 'None, Some: String};
               type Pair = Tuple([Int, String]);
               type Unary = Fn(Int) -> Int;
               let left: Node = {value: 1, children: [{value: 2, children: []}]};
               let same: Node = {value: 1, children: [{value: 2, children: []}]};
               let different: Node = {value: 1, children: [{value: 3, children: []}]};
               let none: Choice = 'None;
               let some: Choice = 'Some("x");
               {
                   int_equal: (equality.equal(Int)(1, 1), eq.equal(1, 1), 1 == 1),
                   int_different: (equality.equal(Int)(1, 2), eq.equal(1, 2), 1 == 2),
                   array_equal: (equality.equal(Array(Int))([1, 2], [1, 2]), eq.equal([1, 2], [1, 2]), [1, 2] == [1, 2]),
                   array_length: (equality.equal(Array(Int))([1], [1, 2]), eq.equal([1], [1, 2]), [1] == [1, 2]),
                   tuple_equal: (equality.equal(Pair)((1, "a"), (1, "a")), eq.equal((1, "a"), (1, "a")), (1, "a") == (1, "a")),
                   dict_different: (equality.equal(Dict(Int))({a: 1}, {a: 2}), eq.equal({a: 1}, {a: 2}), {a: 1} == {a: 2}),
                   enum_equal: (equality.equal(Choice)(some, some), eq.equal(some, some), some == some),
                   enum_tag: (equality.equal(Choice)(none, some), eq.equal(none, some), none == some),
                   recursive_equal: (equality.equal(Node)(left, same), eq.equal(left, same), left == same),
                   recursive_different: (equality.equal(Node)(left, different), eq.equal(left, different), left == different),
                   function_error: equality.equal(Unary)(fn(x) { x }, fn(x) { x }),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 500_000).unwrap();
        let Value::Dict(output) = module.execute(500_000).unwrap() else {
            panic!("equality interpreter test must return a Dict")
        };
        for field in [
            "int_equal",
            "array_equal",
            "tuple_equal",
            "enum_equal",
            "recursive_equal",
        ] {
            assert_eq!(
                output.get(field).unwrap().to_string(),
                "('Ok('True), 'True, 'True)"
            );
        }
        for field in [
            "int_different",
            "array_length",
            "dict_different",
            "enum_tag",
            "recursive_different",
        ] {
            assert_eq!(
                output.get(field).unwrap().to_string(),
                "('Ok('False), 'False, 'False)"
            );
        }
        assert!(
            output
                .get("function_error")
                .unwrap()
                .to_string()
                .starts_with("'Err(")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_eq_is_the_function_form_of_the_equality_operator() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               import "std/eq" as eq;
               let function = fn(value) { value };
               let pairs = arrays.zip([1, 2, 3], [1, 2, 3]);
               def accepts_int_eq: Fn(Fn(Int, Int) -> Bool) -> Bool = fn(compare) {
                   compare(1, 1)
               };
               {
                   scalar: (eq.equal(1, 1), 1 == 1),
                   heterogeneous: (eq.equal(1, "1"), 1 == "1"),
                   nested: (eq.equal([{a: 1}], [{a: 1}]), [{a: 1}] == [{a: 1}]),
                   same_function: (eq.equal(function, function), function == function),
                   other_function: (eq.equal(function, fn(value) { value }), function == fn(value) { value }),
                   higher_order: match pairs {
                       'None => 'False,
                       'Some(values) => arrays.all(values, fn(pair) {
                           match pair { (left, right) => eq.equal(left, right) }
                       }),
                   },
                   direct_callback: accepts_int_eq(eq.equal),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 200_000).unwrap();
        let Value::Dict(output) = module.execute(200_000).unwrap() else {
            panic!("eq test must return a Dict")
        };
        for field in ["scalar", "nested", "same_function"] {
            assert_eq!(output.get(field).unwrap().to_string(), "('True, 'True)");
        }
        assert_eq!(
            output.get("other_function").unwrap().to_string(),
            "('False, 'False)"
        );
        assert_eq!(
            output.get("heterogeneous").unwrap().to_string(),
            "('False, 'False)"
        );
        assert_eq!(output.get("higher_order").unwrap().to_string(), "'True");
        assert_eq!(output.get("direct_callback").unwrap().to_string(), "'True");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn homogeneous_dict_combinators_preserve_types_and_canonical_order() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dict" as dicts;
               let source: Dict(Int) = {z: 3, a: 1, middle: 2};
               let empty: Dict(Int) = {};
               {
                   mapped: dicts.map_values(source, fn(value) { `v\{value}` }),
                   filtered: dicts.filter(source, fn(value) { 1 < value }),
                   folded: dicts.fold(source, "", fn(total, key, value) {
                       `\{total}\{key}=\{value};`
                   }),
                   empty_mapped: dicts.map_values(empty, fn(value) { `v\{value}` }),
                   empty_filtered: dicts.filter(empty, fn(value) { 0 < value }),
                   empty_folded: dicts.fold(empty, 42, fn(total, key, value) {
                       total + value
                   }),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "{empty_filtered: Dict<Int>, empty_folded: Int, empty_mapped: Dict<String>, filtered: Dict<Int>, folded: String, mapped: Dict<String>}"
        );
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected Dict result")
        };
        assert_eq!(
            result.get("mapped").unwrap().to_string(),
            r#"{a: "v1", middle: "v2", z: "v3"}"#
        );
        assert_eq!(
            result.get("filtered").unwrap().to_string(),
            "{middle: 2, z: 3}"
        );
        assert_eq!(
            result.get("folded").unwrap().to_string(),
            r#""a=1;middle=2;z=3;""#
        );
        assert_eq!(result.get("empty_mapped").unwrap().to_string(), "{}");
        assert_eq!(result.get("empty_filtered").unwrap().to_string(), "{}");
        assert_eq!(result.get("empty_folded").unwrap().to_string(), "42");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn homogeneous_dict_combinators_reject_invalid_contracts_and_trace_callbacks() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/dict" as dicts;
               dicts.filter({a: 1}, fn(value) { value })"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot unify Int with enum {False, True}")
        );

        fs::write(
            &main,
            r#"import "std/dict" as dicts;
               let mixed = {number: 1, text: "two"};
               dicts.map_values(mixed, fn(value) { value })"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("cannot unify"));

        fs::write(
            &main,
            r#"import "std/dict" as dicts;
               let source: Dict(Int) = {a: 1};
               dicts.map_values(source, fn(value) { value / 0 })"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(error.to_string().contains("main.telora:3:"));
        assert!(
            error
                .trace
                .iter()
                .any(|frame| frame.function == "std/dict.map_values")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_dict_supports_tool_stage_and_exact_output_quota() {
        let directory = fixture_dir();
        fs::write(directory.join("data.json"), r#"{"a":1,"long":2}"#).unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dict" as dicts;
               import "./data.json" as data;
               dicts.keys(data)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let requested = 2 * std::mem::size_of::<Value>() as u64 + 5;
        let mut exact = QuotaAccount::new(Quota::new(1, 1_000, requested));
        let arena = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut exact,
            )
            .unwrap();
        assert_eq!(exact.requested_allocation_bytes(), requested);
        assert_eq!(
            arena.export(&module.runtime.main.heap).unwrap().to_string(),
            "[\"a\", \"long\"]"
        );

        let mut short = QuotaAccount::new(Quota::new(1, 1_000, requested - 1));
        assert_eq!(
            Vm::new()
                .execute_in_work(
                    &module.runtime.main.heap,
                    &module.runtime.externals,
                    &module.function,
                    &[],
                    &mut short,
                )
                .err()
                .expect("allocation must be exhausted")
                .kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        fs::write(
            directory.join("types.telora"),
            r#"import "std/dict" as dicts;
               type Pair = Tuple(dicts.values({ first: Int, second: String }));
               let pair: Pair = (1, "one");
               pair"#,
        )
        .unwrap();
        let types = load_module(directory.join("types.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(types.execute(100_000).unwrap().to_string(), "(1, \"one\")");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_attributes_normalizes_flattens_and_inspects_arbitrary_values() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/attributes" as attributes;
               let nested = {
                   kind: 'WithAttributes,
                   inner: {
                       kind: 'WithAttributes,
                       inner: 42,
                       attributes: { shared: "inner", only_inner: 1 },
                   },
                   attributes: { shared: "outer", only_outer: 2 },
               };
               let augmented = attributes.add(
                   nested,
                   { shared: "addition", "vendor:acme.flag": 'True },
               );
               {
                   normalized: attributes.normalize(nested),
                   all: attributes.all(augmented),
                   shared: attributes.get(augmented, "shared"),
                   missing: attributes.get(augmented, "missing"),
                   has: attributes.has(augmented, "vendor:acme.flag"),
                   lacks: attributes.has(augmented, "missing"),
                   stripped: attributes.strip(augmented),
                   plain: attributes.normalize("plain"),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected Dict result")
        };
        assert_eq!(
            result.get("all").unwrap().to_string(),
            "{only_inner: 1, only_outer: 2, shared: \"addition\", vendor:acme.flag: 'True}"
        );
        assert_eq!(
            result.get("shared").unwrap().to_string(),
            "'Some(\"addition\")"
        );
        assert_eq!(result.get("missing").unwrap().to_string(), "'None");
        assert_eq!(result.get("has").unwrap().to_string(), "'True");
        assert_eq!(result.get("lacks").unwrap().to_string(), "'False");
        assert_eq!(result.get("stripped").unwrap().to_string(), "42");

        let Value::Dict(normalized) = result.get("normalized").unwrap() else {
            panic!("expected normalized wrapper")
        };
        assert_eq!(
            normalized.get("attributes").unwrap().to_string(),
            "{only_inner: 1, only_outer: 2, shared: \"outer\"}"
        );
        assert_eq!(normalized.get("inner").unwrap().to_string(), "42");
        let Value::Dict(plain) = result.get("plain").unwrap() else {
            panic!("expected plain wrapper")
        };
        assert_eq!(plain.get("attributes").unwrap().to_string(), "{}");
        assert_eq!(plain.get("inner").unwrap().to_string(), "\"plain\"");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn attributed_type_metadata_is_transparent_and_preserved() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/attributes" as attributes;
               import "std/codec" as codec;
               let rename = fn(name) {
                   let decorate: Fn(Any, Any) -> Any = fn(ctx, value) {
                       attributes.add(value, { "std/json.rename": name })
                   }; decorate
               };
               let model: Fn(Any, Any) -> Any = fn(ctx, value) {
                   attributes.add(struct(ctx, value), { "vendor:acme.model": ctx.name })
               };
               @model
               type User = {
                   @rename("type")
                   ty: String,
               };
               let user: User = { ty: "admin" };
               {
                   metadata: User,
                   checked: validate(User, user),
                   decoded: codec.decode(User, { "type": "member" }),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected Dict result")
        };
        assert!(
            result
                .get("checked")
                .unwrap()
                .to_string()
                .starts_with("'Ok(")
        );
        assert!(
            result
                .get("decoded")
                .unwrap()
                .to_string()
                .starts_with("'Ok(")
        );

        let Value::Dict(metadata) = result.get("metadata").unwrap() else {
            panic!("expected attributed type metadata")
        };
        assert_eq!(metadata.get("kind").unwrap().to_string(), "'WithAttributes");
        let Value::Dict(model_attributes) = metadata.get("attributes").unwrap() else {
            panic!("expected model attributes")
        };
        assert_eq!(
            model_attributes
                .get("vendor:acme.model")
                .unwrap()
                .to_string(),
            "\"User\""
        );
        let Value::Dict(struct_metadata) = metadata.get("inner").unwrap() else {
            panic!("expected Struct metadata")
        };
        let Value::Dict(fields) = struct_metadata.get("fields").unwrap() else {
            panic!("expected Struct fields")
        };
        let Value::Dict(field) = fields.get("ty").unwrap() else {
            panic!("expected attributed field metadata")
        };
        assert_eq!(field.get("kind").unwrap().to_string(), "'WithAttributes");
        let Value::Dict(field_attributes) = field.get("attributes").unwrap() else {
            panic!("expected field attributes")
        };
        assert_eq!(
            field_attributes.get("std/json.rename").unwrap().to_string(),
            "\"type\""
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn normalized_struct_and_enum_models_preserve_uniform_member_attributes() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/attributes" as attributes;
               let annotate = fn(key, payload) {
                   let decorate: Fn(Any, Any) -> Any = fn(ctx, value) { attributes.add(value, { marker: (key, payload) }) };
                   decorate
               };

               @annotate("model", 1)
               @struct
               type User = {
                   name: String,
                   @annotate("field", 2)
                   role: String,
               };

               @annotate("enum", 3)
               @enum
               type Choice = {
                   None: 'None,
                   User: User,
               };

               @union
               type Scalar = [
                   attributes.add(Int, { marker: ("union", 4) }),
                   String,
               ];

               let explicit = struct('None, { value: Int });
               let explicit_union = union('None, [Int, String]);
               let unit: Choice = 'None;
               let payload: Choice = 'User({ name: "Ada", role: "admin" });
               let scalar_value: Scalar = 42;
               {
                   user: User,
                   choice: Choice,
                   explicit: explicit,
                   explicit_union: explicit_union,
                   scalar: Scalar,
                   scalar_value: scalar_value,
                   unit: validate(Choice, unit),
                   payload: validate(Choice, payload),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected model result")
        };
        assert!(result.get("unit").unwrap().to_string().starts_with("'Ok("));
        assert!(
            result
                .get("payload")
                .unwrap()
                .to_string()
                .starts_with("'Ok(")
        );

        fn assert_wrapper(value: &Value) -> &crate::Dict {
            let Value::Dict(wrapper) = value else {
                panic!("expected WithAttributes wrapper")
            };
            assert_eq!(wrapper.get("kind").unwrap().to_string(), "'WithAttributes");
            assert!(matches!(wrapper.get("attributes"), Some(Value::Dict(_))));
            wrapper
        }
        let user = assert_wrapper(result.get("user").unwrap());
        let Value::Dict(user_metadata) = user.get("inner").unwrap() else {
            panic!("expected Struct metadata")
        };
        assert_eq!(user_metadata.get("kind").unwrap().to_string(), "'Struct");
        let Value::Dict(fields) = user_metadata.get("fields").unwrap() else {
            panic!("expected normalized fields")
        };
        let name = assert_wrapper(fields.get("name").unwrap());
        assert_eq!(name.get("attributes").unwrap().to_string(), "{}");
        let role = assert_wrapper(fields.get("role").unwrap());
        assert_eq!(
            role.get("attributes").unwrap().to_string(),
            "{marker: (\"field\", 2)}"
        );
        assert_eq!(
            user.get("attributes").unwrap().to_string(),
            "{marker: (\"model\", 1)}"
        );

        let choice = assert_wrapper(result.get("choice").unwrap());
        let Value::Dict(enum_metadata) = choice.get("inner").unwrap() else {
            panic!("expected Enum metadata")
        };
        assert_eq!(enum_metadata.get("kind").unwrap().to_string(), "'Enum");
        let Value::Dict(variants) = enum_metadata.get("variants").unwrap() else {
            panic!("expected normalized variants")
        };
        for variant in variants.values() {
            assert_wrapper(variant);
        }
        let none = assert_wrapper(variants.get("None").unwrap());
        assert_eq!(none.get("inner").unwrap().to_string(), "'None");
        assert_eq!(
            choice.get("attributes").unwrap().to_string(),
            "{marker: (\"enum\", 3)}"
        );

        let scalar = assert_wrapper(result.get("scalar").unwrap());
        let Value::Dict(union_metadata) = scalar.get("inner").unwrap() else {
            panic!("expected Union metadata")
        };
        assert_eq!(union_metadata.get("kind").unwrap().to_string(), "'Union");
        let Value::Array(union_variants) = union_metadata.get("variants").unwrap() else {
            panic!("expected normalized Union variants")
        };
        assert_eq!(union_variants.len(), 2);
        let first = assert_wrapper(&union_variants[0]);
        assert_eq!(
            first.get("attributes").unwrap().to_string(),
            "{marker: (\"union\", 4)}"
        );
        let second = assert_wrapper(&union_variants[1]);
        assert_eq!(second.get("attributes").unwrap().to_string(), "{}");

        let explicit = assert_wrapper(result.get("explicit").unwrap());
        let Value::Dict(explicit_metadata) = explicit.get("inner").unwrap() else {
            panic!("expected explicit Struct metadata")
        };
        let Value::Dict(explicit_fields) = explicit_metadata.get("fields").unwrap() else {
            panic!("expected explicit fields")
        };
        assert_wrapper(explicit_fields.get("value").unwrap());
        let explicit_union = assert_wrapper(result.get("explicit_union").unwrap());
        let Value::Dict(explicit_union_metadata) = explicit_union.get("inner").unwrap() else {
            panic!("expected explicit Union metadata")
        };
        let Value::Array(explicit_variants) = explicit_union_metadata.get("variants").unwrap()
        else {
            panic!("expected explicit Union variants")
        };
        for variant in explicit_variants.iter() {
            assert_wrapper(variant);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enum_validation_rejects_unknown_tags_and_payload_shape_mismatches() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               @enum
               type Choice = { None: 'None, Number: Int };
               {
                   unknown: validate(Choice, 'Other),
                   missing: validate(Choice, 'Number),
                   unexpected: validate(Choice, 'None(1)),
                   wrong: validate(Choice, 'Number("one")),
                   codec: codec.decode(Choice, "None"),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected validation results")
        };
        for field in ["unknown", "missing", "unexpected", "wrong"] {
            assert!(result.get(field).unwrap().to_string().starts_with("'Err("));
        }
        assert_eq!(result.get("codec").unwrap().to_string(), "'Ok('None)");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enum_json_codecs_round_trip_external_and_untagged_representations() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               @struct type User = {name: String};
               @json.rename_all('CamelCase)
               @enum type Event = {
                   Idle: 'None,
                   UserJoined: User,
                   @json.rename("fatal") FatalError: String,
               };
               @json.untagged
               @enum type Scalar = {Text: String, Count: Int};
               @struct type Envelope = {event: Event};
               {
                   idle: codec.decode(Event, "idle") |> result.unwrap,
                   joined: codec.decode(Event, {userJoined: {name: "Ada"}}) |> result.unwrap,
                   fatal: codec.encode(Event, 'FatalError("boom")) |> result.unwrap,
                   nested: codec.encode(Envelope, {event: 'UserJoined({name: "Lin"})}) |> result.unwrap,
                   text: codec.decode(Scalar, "hello") |> result.unwrap,
                   count: codec.encode(Scalar, 'Count(3)) |> result.unwrap,
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("expected Enum codec results")
        };
        assert_eq!(output.get("idle").unwrap().to_string(), "'Idle");
        assert_eq!(
            output.get("joined").unwrap().to_string(),
            "'UserJoined({name: \"Ada\"})"
        );
        assert_eq!(
            output.get("fatal").unwrap().to_string(),
            "{fatal: \"boom\"}"
        );
        assert_eq!(
            output.get("nested").unwrap().to_string(),
            "{event: {userJoined: {name: \"Lin\"}}}"
        );
        assert_eq!(output.get("text").unwrap().to_string(), "'Text(\"hello\")");
        assert_eq!(output.get("count").unwrap().to_string(), "3");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn untagged_enum_json_codec_rejects_no_match_and_ambiguity() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               @json.untagged @enum type Scalar = {Text: String, Count: Int};
               @json.untagged @enum type Ambiguous = {Anything: Any, Text: String};
               {
                   no_match: codec.decode(Scalar, []),
                   ambiguous: codec.decode(Ambiguous, "text"),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("expected failures")
        };
        assert!(
            output
                .get("no_match")
                .unwrap()
                .to_string()
                .contains("matches no untagged")
        );
        assert!(
            output
                .get("ambiguous")
                .unwrap()
                .to_string()
                .contains("ambiguously matches")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn json_schema_and_codecs_share_one_vertical_model_plan() {
        let directory = fixture_dir();
        fs::write(
            directory.join("data.json"),
            r#"{"userId":7,"city_name":"London","event":{"userJoined":{"name":"Ada"}},"scalar":"active","notes":""}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./data.json" as data;
               import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               @struct type User = {name: String};
               @struct type Details = {city_name: String};
               @json.rename_all('CamelCase)
               @enum type Event = {Idle: 'None, UserJoined: User};
               @json.untagged @enum type Scalar = {Text: String, Count: Int};
               @json.rename_all('CamelCase)
               @struct type Model = {
                   user_id: Int,
                   @json.flatten details: Details,
                   @json.default('None) nickname: Option(String),
                   event: Event,
                   scalar: Scalar,
                   @json.skip_serializing_if('Empty) notes: String,
               };
               let decoded = codec.decode(Model, data) |> result.unwrap;
               let schema = json.schema(Model);
               {
                   decoded: decoded,
                   encoded: codec.encode(Model, decoded) |> result.unwrap,
                   schema: schema,
                   schema_text: json.stringify(schema),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("expected vertical model output")
        };
        let Value::Dict(schema) = output.get("schema").unwrap() else {
            panic!("expected schema Dict")
        };
        assert_eq!(schema.get("type").unwrap().to_string(), "\"object\"");
        assert_eq!(
            schema.get("additionalProperties").unwrap().to_string(),
            "'False"
        );
        let Value::Dict(properties) = schema.get("properties").unwrap() else {
            panic!("expected properties")
        };
        for key in [
            "userId",
            "city_name",
            "nickname",
            "event",
            "scalar",
            "notes",
        ] {
            assert!(
                properties.get(key).is_some(),
                "missing schema property {key}"
            );
        }
        assert!(
            schema
                .get("required")
                .unwrap()
                .to_string()
                .contains("userId")
        );
        assert!(
            !schema
                .get("required")
                .unwrap()
                .to_string()
                .contains("nickname")
        );
        assert!(
            output
                .get("schema_text")
                .unwrap()
                .to_string()
                .contains("$schema")
        );
        assert!(!output.get("encoded").unwrap().to_string().contains("notes"));
        assert!(
            output
                .get("encoded")
                .unwrap()
                .to_string()
                .contains("userId")
        );

        fs::write(
            directory.join("data.json"),
            r#"{"userId":"wrong","city_name":"London","event":"idle","scalar":1,"notes":""}"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let failure = module.execute(100_000).unwrap_err();
        assert!(failure.message.contains("$.userId"));
        assert!(failure.data_location().is_some());
        assert!(failure.rule_location().is_some());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn json_schema_maps_composites_and_obeys_allocation_quota() {
        let directory = fixture_dir();
        let path = directory.join("main.telora");
        fs::write(
            &path,
            r#"import "std/json" as json;
               json.schema(union('None, [Int, Array(String), {kind: 'Tuple, items: [Int, String]}]))"#,
        )
        .unwrap();
        let module = load_module(&path, BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("anyOf"));
        assert!(output.contains("prefixItems"));
        assert!(output.contains("items"));

        let mut account = QuotaAccount::new(Quota::new(10, 1_000, 1));
        let error = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            )
            .err()
            .expect("schema generation must exhaust allocation quota");
        assert_eq!(error.kind, crate::RuntimeErrorKind::AllocationQuotaExceeded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn builtin_bool_and_option_keep_natural_json_codec_and_schema_forms() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               {
                   boolean: codec.decode(Bool, 'True) |> result.unwrap,
                   none: codec.decode(Option(Int), 'None) |> result.unwrap,
                   some: codec.decode(Option(Int), 3) |> result.unwrap,
                   encoded: codec.encode(Option(Int), 'Some(4)) |> result.unwrap,
                   bool_schema: json.schema(Bool),
                   option_schema: json.schema(Option(Int)),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("boolean: 'True"), "{output}");
        assert!(output.contains("none: 'None"), "{output}");
        assert!(output.contains("some: 'Some(3)"), "{output}");
        assert!(output.contains("encoded: 4"), "{output}");
        assert!(output.contains("type: \"boolean\""), "{output}");
        assert!(output.contains("type: \"null\""), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recursive_type_metadata_publishes_and_drives_codecs_and_schema_refs() {
        let directory = fixture_dir();
        fs::write(
            directory.join("Types.telora"),
            r#"import "std/json" as json;
               @struct type Node = {
                   value: Int,
                   children: Array(Node),
               };
               @struct type Left = {@json.rename("rightValue") right: Option(Right)};
               @struct type Right = {left: Option(Left)};
               {Node: Node, Left: Left, Right: Right}"#,
        )
        .unwrap();
        let types_module =
            load_module(directory.join("Types.telora"), BTreeMap::new(), 100_000).unwrap();
        let node = types_module.analysis.declared_types["Node"];
        let crate::TypeNode::Struct(fields) = types_module.analysis.types.node(node) else {
            panic!("Node must be a Struct in the authoritative type graph");
        };
        let crate::TypeNode::Array(children) = types_module.analysis.types.node(fields["children"])
        else {
            panic!("Node.children must be an Array");
        };
        assert_eq!(
            *children, node,
            "the recursive edge must retain TypeId identity"
        );
        assert_eq!(
            types_module.analysis.display(node),
            "{children: Array<Node>, value: Int}"
        );
        assert!(types_module.analysis.types.is_assignable(node, node));

        fs::write(
            directory.join("main.telora"),
            r#"import "./Types.telora" as Types;
               import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               let node = codec.decode(Types.Node, {
                   value: 1,
                   children: [{value: 2, children: []}],
               }) |> result.unwrap;
               let pair = codec.decode(Types.Left, {
                   rightValue: {left: 'None},
               }) |> result.unwrap;
               {
                   node: node,
                   encoded: codec.encode(Types.Node, node) |> result.unwrap,
                   pair: pair,
                   schema: json.schema(Types.Node),
                   mutual_schema: json.schema(Types.Left),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(
            output.contains("children: [{children: [], value: 2}]"),
            "{output}"
        );
        assert!(
            output.contains("pair: {right: 'Some({left: 'None})}"),
            "{output}"
        );
        assert!(output.contains("$defs"), "{output}");
        assert!(output.contains("#/$defs/Type0"), "{output}");
        assert!(output.contains("#/$defs/Type1"), "{output}");

        fs::write(
            directory.join("bad.json"),
            r#"{"value":1,"children":[{"value":"wrong","children":[]}]}"#,
        )
        .unwrap();
        fs::write(
            directory.join("bad.telora"),
            r#"import "./bad.json" as data;
               import "./Types.telora" as Types;
               import "std/codec" as codec;
               import "std/result" as result;
               codec.decode(Types.Node, data) |> result.unwrap"#,
        )
        .unwrap();
        let bad = load_module(directory.join("bad.telora"), BTreeMap::new(), 100_000).unwrap();
        let failure = bad.execute(100_000).unwrap_err();
        assert!(failure.message.contains("$.children[0].value"));
        assert!(failure.data_location().is_some());
        assert!(failure.rule_location().is_some());

        fs::write(
            directory.join("leak.telora"),
            r#"import "./Types.telora" as Types;
               import "std/json" as json;
               json.stringify(Types.Node)"#,
        )
        .unwrap();
        let leak = load_module(directory.join("leak.telora"), BTreeMap::new(), 100_000).unwrap();
        assert!(
            leak.execute(100_000)
                .unwrap_err()
                .message
                .contains("internal up-link")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_nested_type_families_preserve_recursive_codec_metadata() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"@struct type IntValue = {value: Int};
               @struct type StringValue = {value: String};
               @enum type Val = {Int: IntValue, Str: StringValue};
               @struct type BinaryNode = {left: Expr, right: Expr};
               @struct type ColumnRef = {alias: String, column: String};
               @enum type Expr = {
                   Value: Val,
                   Add: BinaryNode,
                   Column: ColumnRef,
               };
               @struct type Mapping = {predicate: Expr};
               @struct type Relation(M) = {mapping: M};
               @struct type RelationUse(Entity) = {
                   entity: Entity,
                   relation: Relation(Mapping),
               };
               export {Expr, Val, Mapping, Relation, RelationUse};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./types.telora" as types;
               import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               @enum type Entity = {Order: 'None};
               type Use = types.RelationUse(Entity);
               let relation: Use = {
                   entity: 'Order,
                   relation: {mapping: {predicate: 'Add({
                       left: 'Value('Int({value: 1})),
                       right: 'Column({alias: "t", column: "id"}),
                   })}},
               };
               {
                   encoded: codec.encode(Use, relation)
                       |> result.unwrap
                       |> json.stringify,
                   schema: json.schema(Use),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("\\\"left\\\""), "{output}");
        assert!(output.contains("\\\"column\\\":\\\"id\\\""), "{output}");
        assert!(output.contains("$defs"), "{output}");
        assert!(output.contains("#/$defs/Type"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exported_codec_boundary_owns_complex_family_witness() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"import "std/codec" as codec;
               @struct type Binary = {left: Expr, right: Expr};
               @enum type Expr = {Lit: Int, Add: Binary};
               @struct type Payload(A, B, C, D, E, F, G) = {
                   a: A, b: B, c: C, d: D, e: E, f: F, g: G,
               };
               type Rejection = Payload(
                   Int, String, Bool, Float, Expr, Array(Int), Option(String)
               );
               def encode_rejection = fn(value: Rejection) {
                   codec.encode(Rejection, value)
               };
               export {Expr, Rejection, encode_rejection};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./types.telora" as types;
               import "std/json" as json;
               import "std/result" as result;
               let rejection: types.Rejection = {
                   a: 1,
                   b: "two",
                   c: 'True,
                   d: 4.0,
                   e: 'Add({left: 'Lit(5), right: 'Lit(6)}),
                   f: [7],
                   g: 'Some("eight"),
               };
               types.encode_rejection(rejection)
                   |> result.unwrap
                   |> json.stringify"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("\\\"left\\\""), "{output}");
        assert!(output.contains("\\\"g\\\":\\\"eight\\\""), "{output}");

        fs::write(
            directory.join("invalid.telora"),
            r#"import "./types.telora" as types;
               types.encode_rejection({
                   a: "wrong",
                   b: "two",
                   c: 'True,
                   d: 4.0,
                   e: 'Lit(5),
                   f: [7],
                   g: 'None,
               })"#,
        )
        .unwrap();
        let error = load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000)
            .unwrap_err()
            .to_string();
        assert!(error.contains("String") && error.contains("Int"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn check_keeps_recursive_metadata_before_the_legacy_value_boundary() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"@struct type CallExpr = { args: Array(Expr) };
@enum type Expr = { Call: CallExpr, Text: String };
export { CallExpr, Expr };"#,
        )
        .unwrap();

        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        engine.check(&module).unwrap();
        let legacy = engine.execute(&module).unwrap_err();
        assert!(
            legacy
                .message
                .contains("cyclic heap values cannot cross the legacy Value boundary")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recursive_type_metadata_keeps_typed_module_import_surfaces() {
        let directory = fixture_dir();
        fs::write(
            directory.join("expr.telora"),
            r#"@struct type Binary = {left: Expr, right: Expr};
               @enum type Expr = {Lit: Int, Add: Binary};
               def lit: Fn(Int) -> Expr = fn(value) { 'Lit(value) };
               def add: Fn(Expr, Expr) -> Expr = fn(left, right) {
                   'Add({left, right})
               };
               def depth: Fn(Expr) -> Int = fn(expr) {
                   match expr {
                       'Lit(_) => 1,
                       'Add({left, right}) => 1 + depth(left) + depth(right),
                   }
               };
               export {Binary, Expr, lit, add, depth};"#,
        )
        .unwrap();

        fs::write(
            directory.join("whole.telora"),
            r#"import "./expr.telora" as expr;
               import "std/array" as array;
               import "std/type-desc" as desc;
               def has_ref = fn(ty, fuel) {
                   if fuel < 1 {
                       'False
                   } else {
                       if desc.kind(ty) == 'Ref {
                           'True
                       } else {
                           array.any(desc.children(ty), fn(child) {
                               has_ref(child, fuel - 1)
                           })
                       }
                   }
               };
               let value: expr.Expr = expr.add(
                   expr.lit(1),
                   expr.add(expr.lit(2), expr.lit(3)),
               );
               export let output = (expr.depth(value), has_ref(expr.Expr, 8));"#,
        )
        .unwrap();
        let whole = load_module(directory.join("whole.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            whole.execute(100_000).unwrap().to_string(),
            "{output: (5, 'True)}"
        );

        for (name, import) in [
            (
                "selective.telora",
                r#"import "./expr.telora" {Expr, lit, add, depth};"#,
            ),
            ("open.telora", r#"import "./expr.telora" *;"#),
        ] {
            fs::write(
                directory.join(name),
                format!(
                    r#"{import}
                       let value: Expr = add(lit(1), lit(2));
                       export let output = depth(value);"#
                ),
            )
            .unwrap();
            let module = load_module(directory.join(name), BTreeMap::new(), 100_000).unwrap();
            assert_eq!(module.execute(100_000).unwrap().to_string(), "{output: 3}");
        }

        fs::write(
            directory.join("invalid.telora"),
            r#"import "./expr.telora" {Expr, depth};
               export let output = depth("bad");"#,
        )
        .unwrap();
        let invalid = load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000)
            .unwrap_err()
            .to_string();
        assert!(
            invalid.contains("String") && invalid.contains("Expr"),
            "{invalid}"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn final_program_observes_only_presealed_recursive_type_roots() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               @struct type Forward = {next: Later};
               let premature = codec.decode(Forward, {next: 1});
               type Later = Int;
               premature"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "'Ok({next: 1})"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn builtin_bool_option_and_result_are_normalized_enum_metadata() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/attributes" as attributes;
               type Maybe = Option(attributes.add(Int, { marker: "payload" }));
               type Outcome = Result(String, Int);
               let compared: Bool = 1 < 2;
               let none: Maybe = 'None;
               let some: Maybe = 'Some(42);
               let ok: Outcome = 'Ok("done");
               let err: Outcome = 'Err(7);
               {
                   bool: Bool,
                   maybe: Maybe,
                   outcome: Outcome,
                   compared: validate(Bool, compared),
                   none: validate(Maybe, none),
                   some: validate(Maybe, some),
                   ok: validate(Outcome, ok),
                   err: validate(Outcome, err),
                   wrong_bool: validate(Bool, 'Other),
                   wrong_some: validate(Maybe, 'Some("forty-two")),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(result) = module.execute(100_000).unwrap() else {
            panic!("expected built-in type results")
        };
        for field in ["compared", "none", "some", "ok", "err"] {
            assert!(result.get(field).unwrap().to_string().starts_with("'Ok("));
        }
        for field in ["wrong_bool", "wrong_some"] {
            assert!(result.get(field).unwrap().to_string().starts_with("'Err("));
        }

        fn wrapper(value: &Value) -> &crate::Dict {
            let Value::Dict(wrapper) = value else {
                panic!("expected WithAttributes wrapper")
            };
            assert_eq!(wrapper.get("kind").unwrap().to_string(), "'WithAttributes");
            assert!(matches!(wrapper.get("attributes"), Some(Value::Dict(_))));
            wrapper
        }
        for field in ["bool", "maybe", "outcome"] {
            let root = wrapper(result.get(field).unwrap());
            let Value::Dict(metadata) = root.get("inner").unwrap() else {
                panic!("expected Enum metadata")
            };
            assert_eq!(metadata.get("kind").unwrap().to_string(), "'Enum");
            let Value::Dict(variants) = metadata.get("variants").unwrap() else {
                panic!("expected Enum variants")
            };
            for variant in variants.values() {
                wrapper(variant);
            }
        }
        let maybe = wrapper(result.get("maybe").unwrap());
        let Value::Dict(metadata) = maybe.get("inner").unwrap() else {
            panic!("expected Option metadata")
        };
        let Value::Dict(variants) = metadata.get("variants").unwrap() else {
            panic!("expected Option variants")
        };
        let some = wrapper(variants.get("Some").unwrap());
        assert_eq!(
            some.get("attributes").unwrap().to_string(),
            "{marker: \"payload\"}"
        );
        let none = wrapper(variants.get("None").unwrap());
        assert_eq!(none.get("inner").unwrap().to_string(), "'None");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn builtin_enum_type_constructors_validate_inputs_and_charge_quota() {
        let directory = fixture_dir();
        let invalid_path = directory.join("invalid.telora");
        fs::write(&invalid_path, "Option(1)").unwrap();
        let invalid = load_module(&invalid_path, BTreeMap::new(), 100_000).unwrap_err();
        assert!(invalid.message.contains("cannot unify Int with Type"));

        let quota_path = directory.join("quota.telora");
        fs::write(&quota_path, "Result(String, Int)").unwrap();
        let module = load_module(&quota_path, BTreeMap::new(), 100_000).unwrap();
        let mut account = QuotaAccount::new(Quota::new(10, 1_000, 0));
        let error = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            )
            .err()
            .expect("Result construction must exhaust allocation quota");
        assert_eq!(error.kind, crate::RuntimeErrorKind::AllocationQuotaExceeded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn normalized_model_constructors_reject_invalid_inputs_and_charge_quota() {
        let directory = fixture_dir();
        let run_error = |name: &str, expression: &str| {
            let path = directory.join(name);
            fs::write(&path, expression).unwrap();
            let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
            module.execute(100_000).unwrap_err()
        };
        assert!(
            run_error("context.telora", "struct('Bad, {x: Int})")
                .message
                .contains("model context")
        );
        assert!(
            run_error("empty.telora", "enum('None, {})")
                .message
                .contains("at least one variant")
        );
        assert!(
            run_error("field.telora", "struct('None, {x: 1})")
                .message
                .contains("Type metadata")
        );
        assert!(
            run_error("variant.telora", "enum('None, {Bad: 1})")
                .message
                .contains("Type metadata")
        );
        assert!(
            run_error("empty-union.telora", "union('None, [])")
                .message
                .contains("at least one variant")
        );
        assert!(
            run_error("union-variant.telora", "union('None, [1])")
                .message
                .contains("Type metadata")
        );
        assert!(
            run_error(
                "union-wrapper.telora",
                "union('None, [{kind: 'WithAttributes, inner: Int, attributes: []}])",
            )
            .message
            .contains("attributes must be a Dict")
        );

        for (name, source) in [
            ("uppercase-struct.telora", "Struct({x: Int})"),
            ("uppercase-union.telora", "Union([Int, String])"),
        ] {
            let path = directory.join(name);
            fs::write(&path, source).unwrap();
            let error = match load_module(path, BTreeMap::new(), 100_000) {
                Ok(_) => panic!("uppercase constructor must be absent"),
                Err(error) => error,
            };
            assert!(error.message.contains("unknown binding"));
        }

        let path = directory.join("quota.telora");
        fs::write(&path, "union('None, [Int, String])").unwrap();
        let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
        let mut account = QuotaAccount::new(Quota::new(10, 1_000, 0));
        let error = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            )
            .err()
            .expect("model normalization must exhaust allocation quota");
        assert_eq!(error.kind, crate::RuntimeErrorKind::AllocationQuotaExceeded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_attributes_rejects_malformed_wrappers_and_obeys_allocation_quota() {
        let directory = fixture_dir();
        let path = directory.join("main.telora");
        fs::write(
            &path,
            r#"import "std/attributes" as attributes;
               attributes.normalize({kind: 'WithAttributes, inner: 1, attributes: []})"#,
        )
        .unwrap();
        let module = load_module(&path, BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(error.message.contains("attributes must be a Dict"));

        fs::write(
            &path,
            r#"import "std/attributes" as attributes;
               attributes.normalize(1)"#,
        )
        .unwrap();
        let module = load_module(&path, BTreeMap::new(), 100_000).unwrap();
        let mut account = QuotaAccount::new(Quota::new(10, 1_000, 0));
        let error = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            )
            .err()
            .expect("normalization must exhaust allocation quota");
        assert_eq!(error.kind, crate::RuntimeErrorKind::AllocationQuotaExceeded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_json_decorators_build_flat_standard_attribute_metadata() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/json" as json;
               @json.rename_all('CamelCase)
               @struct
               type Model = {
                   @json.rename("outerName")
                   @json.rename("innerName")
                   @json.default(7)
                   @json.skip_serializing_if('None)
                   value_name: Option(Int),

                   @json.flatten
                   nested: struct('None, { child_value: String }),
               };
               Model"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(root) = module.execute(100_000).unwrap() else {
            panic!("expected attributed model")
        };
        let Value::Dict(root_attributes) = root.get("attributes").unwrap() else {
            panic!("expected root attributes")
        };
        assert_eq!(
            root_attributes
                .get("std/json.rename_all")
                .unwrap()
                .to_string(),
            "'CamelCase"
        );
        let Value::Dict(metadata) = root.get("inner").unwrap() else {
            panic!("expected Struct metadata")
        };
        let Value::Dict(fields) = metadata.get("fields").unwrap() else {
            panic!("expected fields")
        };
        let Value::Dict(value) = fields.get("value_name").unwrap() else {
            panic!("expected normalized field wrapper")
        };
        assert!(
            !matches!(value.get("inner"), Some(Value::Dict(inner)) if matches!(inner.get("kind"), Some(Value::Atom(kind)) if kind.name() == "WithAttributes"))
        );
        let Value::Dict(attributes) = value.get("attributes").unwrap() else {
            panic!("expected field attributes")
        };
        assert_eq!(
            attributes.get("std/json.rename").unwrap().to_string(),
            "\"outerName\""
        );
        assert_eq!(attributes.get("std/json.default").unwrap().to_string(), "7");
        assert_eq!(
            attributes
                .get("std/json.skip_serializing_if")
                .unwrap()
                .to_string(),
            "'None"
        );
        let Value::Dict(nested) = fields.get("nested").unwrap() else {
            panic!("expected nested field wrapper")
        };
        let Value::Dict(nested_attributes) = nested.get("attributes").unwrap() else {
            panic!("expected nested attributes")
        };
        assert_eq!(
            nested_attributes
                .get("std/json.flatten")
                .unwrap()
                .to_string(),
            "'True"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn struct_json_codecs_apply_serde_style_attributes_bidirectionally() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;

               @struct type Coordinates = {
                   latitude: Int,
               };
               @struct type Address = {
                   city_name: String,
                   @json.flatten coordinates: Coordinates,
               };
               @json.rename_all('CamelCase)
               @struct type User = {
                   user_id: Int,
                   @json.rename("display") display_name: String,
                   @json.flatten address: Address,
                   @json.default('None)
                   @json.skip_serializing_if('None)
                   nickname: Option(String),
                   @json.skip_serializing_if('False) hidden: Any,
                   @json.skip_serializing_if('Empty) notes: String,
                   @json.skip_serializing_if('Empty) tags: Array(String),
                   @json.skip_serializing_if('Empty) extras: Any,
               };
               let decoded = codec.decode(User, {
                   userId: 7,
                   display: "Ada",
                   city_name: "London",
                   latitude: 51,
                   hidden: 'False,
                   notes: "",
                   tags: [],
                   extras: {},
               }) |> result.unwrap;
               { decoded: decoded, encoded: codec.encode(User, decoded) |> result.unwrap }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let Value::Dict(output) = module.execute(100_000).unwrap() else {
            panic!("expected codec results")
        };
        assert_eq!(
            output.get("decoded").unwrap().to_string(),
            "{address: {city_name: \"London\", coordinates: {latitude: 51}}, display_name: \"Ada\", extras: {}, hidden: 'False, nickname: 'None, notes: \"\", tags: [], user_id: 7}"
        );
        assert_eq!(
            output.get("encoded").unwrap().to_string(),
            "{city_name: \"London\", display: \"Ada\", latitude: 51, userId: 7}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn json_skip_serializing_if_calls_promoted_bytecode_and_builtin_predicates() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               let zero = 0;
               def is_zero: Fn(Int) -> Bool = fn(value) { value == zero };
               @struct type Model = {
                   @json.skip_serializing_if(is_zero) omitted: Int,
                   @json.skip_serializing_if(is_zero) retained: Int,
                   @json.skip_serializing_if('False) native_omitted: Bool,
               };
               codec.encode(Model, {
                   omitted: 0,
                   retained: 7,
                   native_omitted: 'False,
               })"#,
        )
        .unwrap();
        let module = load_module_with_quota_and_debug_sink(
            directory.join("main.telora"),
            BTreeMap::new(),
            Quota::with_fuel(100_000),
            Arc::new(crate::DiscardDebugSink),
        )
        .unwrap();
        let failure = module.execute_with_quota(Quota::with_fuel(2)).unwrap_err();
        assert_eq!(failure.kind, crate::RuntimeErrorKind::FuelExhausted);
        let value = module.execute_with_quota(Quota::with_fuel(3)).unwrap();
        assert_eq!(value.to_string(), "'Ok({retained: 7})");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn json_skip_serializing_if_rejects_invalid_function_contracts() {
        let directory = fixture_dir();
        fs::write(
            directory.join("arity.telora"),
            r#"import "std/json" as json;
               def wrong: Fn(Any, Any) -> Bool = fn(left, right) { 'False };
               @struct type Model = {
                   @json.skip_serializing_if(wrong) value: Int,
               };
               0"#,
        )
        .unwrap();
        let arity =
            load_module(directory.join("arity.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(arity.message().contains("unary Func"), "{arity}");

        fs::write(
            directory.join("result.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               def identity: Fn(Any) -> Any = fn(value) { value };
               @struct type Model = {
                   @json.skip_serializing_if(identity) value: Int,
               };
               codec.encode(Model, {value: 1})"#,
        )
        .unwrap();
        let module =
            load_module(directory.join("result.telora"), BTreeMap::new(), 100_000).unwrap();
        let result = module.execute(100_000).unwrap_err();
        assert_eq!(result.kind, crate::RuntimeErrorKind::TypeMismatch);
        assert!(result.message.contains("must return 'True or 'False"));
        assert!(
            result
                .trace
                .iter()
                .any(|frame| frame.function == "std/codec.encode")
        );

        fs::write(
            directory.join("callback.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               def fails: Fn(Any) -> Int = fn(value) { 1 / 0 };
               @struct type Model = {
                   @json.skip_serializing_if(fails) value: Int,
               };
               codec.encode(Model, {value: 1})"#,
        )
        .unwrap();
        let callback =
            load_module(directory.join("callback.telora"), BTreeMap::new(), 100_000).unwrap();
        let failure = callback.execute(100_000).unwrap_err();
        assert_eq!(failure.kind, crate::RuntimeErrorKind::DivisionByZero);
        assert!(
            failure
                .trace
                .iter()
                .any(|frame| frame.function.contains("closure"))
        );
        assert!(
            failure
                .trace
                .iter()
                .any(|frame| frame.function == "std/codec.encode")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn json_skip_predicates_resume_at_nested_paths_and_before_flattening() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               def is_zero: Fn(Int) -> Bool = fn(value) { value == 0 };
               def always: Fn(Any) -> Bool = fn(value) { 'True };
               @struct type Item = {
                   @json.skip_serializing_if(is_zero) value: Int,
               };
               @struct type Nested = {required: String};
               @struct type Model = {
                   items: Array(Item),
                   @json.skip_serializing_if(always)
                   @json.flatten nested: Nested,
               };
               codec.encode(Model, {
                   items: [{value: 0}, {value: 2}],
                   nested: {required: "present"},
               })"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "'Ok({items: [{}, {value: 2}]})"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn struct_json_codecs_reject_attribute_conflicts_and_invalid_defaults() {
        let directory = fixture_dir();
        let cases = [
            (
                "collision.telora",
                r#"import "std/codec" as codec;
                   import "std/json" as json;
                   @struct type T = {
                       @json.rename("same") first: Int,
                       @json.rename("same") second: Int,
                   };
                   codec.decode(T, {same: 1})"#,
                "duplicate external field name",
            ),
            (
                "flatten-type.telora",
                r#"import "std/codec" as codec;
                   import "std/json" as json;
                   @struct type T = {@json.flatten value: Int};
                   codec.decode(T, {})"#,
                "flatten requires Struct metadata",
            ),
            (
                "flatten-rename.telora",
                r#"import "std/codec" as codec;
                   import "std/json" as json;
                   @struct type Inner = {value: Int};
                   @struct type T = {
                       @json.flatten @json.rename("x") inner: Inner,
                   };
                   codec.decode(T, {value: 1})"#,
                "flatten cannot be combined",
            ),
            (
                "default.telora",
                r#"import "std/codec" as codec;
                   import "std/json" as json;
                   @struct type T = {@json.default("wrong") value: Int};
                   codec.decode(T, {})"#,
                "expected Int",
            ),
        ];
        for (name, source, expected) in cases {
            let path = directory.join(name);
            fs::write(&path, source).unwrap();
            let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
            let result = module.execute(100_000).unwrap();
            assert!(result.to_string().contains("'Err"), "{result}");
            assert!(result.to_string().contains(expected), "{result}");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_json_decorators_validate_policies_and_charge_allocations() {
        let directory = fixture_dir();
        let run_error = |name: &str, expression: &str| {
            let path = directory.join(name);
            fs::write(&path, format!("import \"std/json\" as json; {expression}")).unwrap();
            match load_module(path, BTreeMap::new(), 100_000) {
                Ok(module) => module.execute(100_000).unwrap_err().message,
                Err(error) => error.to_string(),
            }
        };
        assert!(run_error("rename.telora", "json.rename(1)").contains("String"));
        assert!(run_error("case.telora", "json.rename_all('SnakeCase)").contains("CamelCase"));
        assert!(run_error("skip.telora", "json.skip_serializing_if('Zero)").contains("'Empty"));

        let path = directory.join("quota.telora");
        fs::write(&path, "import \"std/json\" as json; json.rename(\"name\")").unwrap();
        let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
        let mut account = QuotaAccount::new(Quota::new(10, 1_000, 0));
        let error = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            )
            .err()
            .expect("decorator factory must exhaust allocation quota");
        assert_eq!(error.kind, crate::RuntimeErrorKind::AllocationQuotaExceeded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_dict_rejects_invalid_arguments_pairs_and_duplicates() {
        let directory = fixture_dir();
        let run_error = |name: &str, expression: &str| {
            let path = directory.join(name);
            fs::write(&path, format!("import \"std/dict\" as dicts; {expression}")).unwrap();
            match load_module(path, BTreeMap::new(), 100_000) {
                Ok(module) => module.execute(100_000).unwrap_err().message,
                Err(error) => error.to_string(),
            }
        };

        assert!(run_error("keys.telora", "dicts.keys([])").contains("Dict"));
        assert!(run_error("merge.telora", "dicts.merge({}, [])").contains("right Dict"));
        assert!(run_error("pairs-array.telora", "dicts.from_pairs({})").contains("Array"));
        assert!(!run_error("pair-shape.telora", "dicts.from_pairs([(\"a\", 1, 2)])").is_empty());
        assert!(run_error("pair-key.telora", "dicts.from_pairs([('a, 1)])").contains("String"));
        let duplicate = run_error(
            "duplicate.telora",
            "dicts.from_pairs([(\"a\", 1), (\"a\", 2)])",
        );
        assert!(duplicate.contains("duplicate field"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn diamond_dependencies_reuse_the_same_persistent_root() {
        let directory = fixture_dir();
        let c = directory.join("c.telora");
        let a = directory.join("a.telora");
        let b = directory.join("b.telora");
        fs::write(&c, r#"{value: [1, 2, 3]}"#).unwrap();
        fs::write(&a, r#"import "./c.telora" as c; c"#).unwrap();
        fs::write(&b, r#"import "./c.telora" as c; c"#).unwrap();
        let mut loader = ModuleLoader {
            resolver: ModuleResolver::for_root(&a).unwrap(),
            cache: HashMap::new(),
            core_modules: HashMap::new(),
            main: MainWorld::building(),
            visiting: Vec::new(),
            dependencies: BTreeSet::new(),
            module_quota: Quota::with_fuel(100_000),
            debug_sink: Arc::new(DiscardDebugSink),
            sources: SourceDatabase::default(),
            semantic_inputs: BTreeMap::new(),
            source_policy: ModuleSourcePolicy::ExpressionHarness,
        };

        loader.load_value(&a).unwrap();
        let counts_after_a = loader.main.heap.counts();
        loader.load_value(&b).unwrap();
        let a_id = loader.resolver.resolve_root(&a).unwrap().id;
        let b_id = loader.resolver.resolve_root(&b).unwrap().id;
        let c_id = loader
            .resolver
            .resolve_import(&a_id, "./c.telora")
            .unwrap()
            .id;
        let root = |id: &ModuleId| match loader.cache.get(id).unwrap() {
            ModuleState::Ready { root, .. } => *root,
        };

        assert_eq!(root(&a_id), root(&c_id));
        assert_eq!(root(&b_id), root(&c_id));
        assert_eq!(counts_after_a, loader.main.heap.counts());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exported_closures_preserve_module_up_links() {
        let directory = fixture_dir();
        let library = directory.join("library.telora");
        let main = directory.join("main.telora");
        fs::write(
            &library,
            r#"import "std/rt-types/exec.telora" as exec_types;
               import "std/hash" as hash;
               type ExecSettings = exec_types.ExecSettings;
               type ExecRequest = exec_types.ExecRequest;
               type ExecEnv = exec_types.ExecEnv;
               @struct type Platform = {os: String, arch: String};
               @struct type Config = {platform: Platform, offset: Int};
               def helper = fn(value) { value + 1 };
               def helper2 = fn(value) { helper(value) + 1 };
               def select = fn(platform) {
                   let host = `\{platform.os}-\{platform.arch}`;
                   match host {
                       "linux-x86_64" => 1,
                       _ => 0,
                   }
               };
               def even = fn(value: Int) {
                   if value == 0 { 'True } else { odd(value - 1) }
               };
               def odd = fn(value: Int) {
                   if value == 0 { 'False } else { even(value - 1) }
               };
               export { even };
               export def direct = fn(value) { helper(value) };
               export def factory:
                   Fn(Config) -> Fn(Int) -> Int = fn(config) {
                   fn(value) {
                       let ignored = hash.sha256("gcc");
                       helper2(value) + config.offset + select(config.platform) - 2
                   }
               };
               export def command:
                   Fn(String) -> Fn(ExecSettings, ExecRequest) -> ExecEnv = fn(tool) {
                   fn(settings, request) {
                       let selected = select(settings.platform);
                       let suffix = helper(selected);
                       {
                           install: [],
                           cwd: 'Some(request.cwd),
                           bin: `\{settings.install_prefix}/\{tool}-\{suffix}`,
                           args: request.args,
                           env: {clear: 'False, update: {}},
                       }
                   }
               };"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./library.telora" as library;
               export let output = (
                   library.direct(40),
                   library.factory({platform: {os: "linux", arch: "x86_64"}, offset: 2})(39),
                   library.command("gcc")(
                       {
                           platform: {os: "linux", arch: "x86_64"},
                           download_prefix: "/downloads",
                           install_prefix: "/cache",
                       },
                       {args: ["-c", "x.c"], env: {TARGET: "aarch64"}, cwd: "/work"},
                   ),
                   library.even(10),
               );"#,
        )
        .unwrap();

        let engine = recovery_engine();
        let loaded = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(
            named_output(engine.execute(&loaded).unwrap()).to_string(),
            "(41, 42, {args: [\"-c\", \"x.c\"], bin: \"/cache/gcc-2\", cwd: 'Some(\"/work\"), env: {clear: 'False, update: {}}, install: []}, 'True)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pending_modules_defer_imports_and_cache_initialization_outcomes() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"option "test.action" 1;
               option "test.action" 2;
               import "./missing.telora" as missing;
               export { missing as output };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let pending = engine.prepare_module(&main).unwrap();
        assert_eq!(pending.path(), main);
        let first = pending.initialize().unwrap_err().to_string();
        let second = pending.initialize().unwrap_err().to_string();
        assert_eq!(first, second);
        assert!(first.contains("missing.telora"), "{first}");

        fs::write(&main, "export let output = 42;").unwrap();
        let pending = engine.prepare_module(&main).unwrap();
        let first = pending.initialize().unwrap();
        let second = pending.initialize().unwrap();
        assert!(std::ptr::eq(first.module(), second.module()));
        assert_eq!(
            named_output(engine.execute(first.module()).unwrap()).to_string(),
            "42"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn host_invocation_materializes_ready_definition_captures() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"def helper = fn(value) { value + 1 };
               def helper2 = fn(value) { helper(value) + 1 };
               export def factory: Fn(Int) -> Fn(Int) -> Int = fn(offset) {
                   fn(value) { helper2(value) + offset }
               };"#,
        )
        .unwrap();

        let engine = recovery_engine();
        let loaded = engine.load_module(&main, BTreeMap::new()).unwrap();
        let Value::Dict(exports) = engine.execute(&loaded).unwrap() else {
            panic!("expected export record")
        };
        let factory = exports.get("factory").unwrap();
        let generated = engine.invoke(&loaded, factory, &[Value::Int(2)]).unwrap();
        assert_eq!(
            engine
                .invoke(&loaded, &generated, &[Value::Int(38)])
                .unwrap()
                .to_string(),
            "42"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_blocks_failed_imports_and_keeps_independent_facts() {
        let directory = fixture_dir();
        let model = directory.join("model.telora");
        let main = directory.join("main.telora");
        fs::write(
            &model,
            "type Broken = missing(Int); type Good = String; export { Good };",
        )
        .unwrap();
        fs::write(
            &main,
            "import \"./model.telora\" as model;\
             type Local = String;\
             type Uses = model.Good;\
             type Down = Array(Uses);\
             export { Local as output };",
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let main = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        let model = snapshot
            .module_by_path(&canonicalize(&model).unwrap())
            .unwrap();
        assert_eq!(main.state, WorkspaceModuleState::Partial);
        assert_eq!(model.state, WorkspaceModuleState::Partial);
        let fact = |module, name: &str| {
            &snapshot
                .definitions()
                .iter()
                .find(|definition| definition.module == module && definition.name == name)
                .unwrap()
                .ty
        };
        assert_eq!(fact(main.id, "Local").state, crate::FactState::Known);
        assert!(matches!(
            fact(main.id, "Uses").state,
            crate::FactState::Unknown(crate::UnknownReason::BlockedBy(_))
        ));
        assert!(matches!(
            fact(main.id, "Down").state,
            crate::FactState::Unknown(crate::UnknownReason::BlockedBy(_))
        ));
        assert_eq!(fact(model.id, "Good").state, crate::FactState::Known);
        let broken = fact(model.id, "Broken");
        let diagnostic = broken.diagnostics[0];
        assert!(
            snapshot.diagnostics()[diagnostic.index()]
                .message
                .contains("unknown binding")
        );
        assert!(main.imports.iter().any(|import| import.target == model.id));
        assert_ne!(main.source, model.source);
        let model_path = model.path.as_ref().unwrap();
        assert_eq!(model.name, "@src/model.telora");
        assert_eq!(
            snapshot.sources().get(model.source.unwrap()).name.as_ref(),
            model_path.to_string_lossy()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_prefers_complete_analysis() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(&main, "type Item = String; export { Item };").unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Known);
        let item = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.module == root.id && definition.name == "Item")
            .unwrap();
        assert_eq!(item.ty.state, crate::FactState::Known);
        assert!(snapshot.diagnostics().is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_publishes_precise_type_family_schemes() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(&main, "@struct type Box(A) = {value: A}; export { Box };").unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Known);
        let family = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.module == root.id && definition.name == "Box")
            .unwrap();
        assert_eq!(
            family.scheme.as_deref(),
            Some("for(A) Fn(TypeOf(A)) -> TypeOf({value: A})")
        );
        assert!(!family.scheme.as_deref().unwrap().contains("Any"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_keeps_an_independent_type_family_scheme() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            "@struct type Box(A) = {value: A}; let broken = missing; export { Box };",
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Partial);
        let family = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.module == root.id && definition.name == "Box")
            .unwrap();
        assert_eq!(family.ty.state, crate::FactState::Known);
        assert_eq!(
            family.scheme.as_deref(),
            Some("for(A) Fn(TypeOf(A)) -> TypeOf({value: A})")
        );
        assert!(!family.scheme.as_deref().unwrap().contains("Any"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_publishes_runtime_blame_with_data_and_rule_sources() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let data = directory.join("data.json");
        fs::write(&data, r#"{"name":"Telora"}"#).unwrap();
        fs::write(
            &main,
            r#"import "std/result" as result;
               import "./data.json" as data;
               let output = fail!("invalid name", data.name);
               export { output };"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Partial);
        let diagnostic = snapshot
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.message == "invalid name")
            .expect("runtime blame diagnostic");
        assert_eq!(
            diagnostic.labels.len(),
            2,
            "workspace diagnostics: {:#?}",
            snapshot.diagnostics()
        );
        let data_source = snapshot
            .module_by_path(&canonicalize(&data).unwrap())
            .unwrap()
            .source
            .unwrap();
        assert_eq!(diagnostic.labels[0].location.source, data_source);
        assert_eq!(diagnostic.labels[1].location.source, root.source.unwrap());
        assert!(diagnostic.labels[0].primary);
        assert!(!diagnostic.labels[1].primary);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_continues_independent_runtime_bindings_without_cascades() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            "let first = 1 / 0;\n\
             let blocked = first + 1;\n\
             let second = 2 / 0;\n\
             export let output = blocked + second;",
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Partial);
        let division_errors = snapshot
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("division by zero"))
            .collect::<Vec<_>>();
        assert_eq!(division_errors.len(), 2, "{division_errors:#?}");
        assert!(
            division_errors[0].labels[0].location.start
                < division_errors[1].labels[0].location.start
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_preserves_partial_arrays_across_bindings() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
let first = array.map([1, 2], fn(item) {
    if item == 1 { fail!("first", item) } else { item }
});
let second = array.map(first, fn(item) {
    if item == 2 { fail!("second", item) } else { item }
});
export let output = `unexpected \{array.length(second)}`;"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .filter(|message| matches!(*message, "first" | "second"))
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            ["first", "second"],
            "{:#?}",
            snapshot.diagnostics()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_local_annotations_do_not_reduce_diagnostic_coverage() {
        for (annotation, prelude) in [
            ("", "1"),
            (": Int", "1"),
            (": Tuple([Int, String])", "(1, \"one\")"),
            (": Pair", "{left: 1, right: \"two\"}"),
            (": Array(Tuple([Int, String]))", "[(1, \"one\")]"),
        ] {
            let directory = fixture_dir();
            let main = directory.join("main.telora");
            fs::write(
                &main,
                format!(
                    r#"import "std/array" as array;
@enum type A = {{ Bad: 'None }};
@enum type B = {{ Bad: 'None }};
@struct type Pair = {{ left: Int, right: String }};
def fail_a: Fn(A) -> Int = fn(value) {{ fail!("diagnostic A", value) }};
def fail_b: Fn(B) -> Int = fn(value) {{ fail!("diagnostic B", value) }};
def run_both: Fn(Array(A), Array(B)) -> Int = fn(values_a, values_b) {{
    let pre{annotation} = {prelude};
    let first = array.map(values_a, fail_a);
    let second = array.map(values_b, fail_b);
    array.length(first) + array.length(second)
}};
let result = run_both(['Bad], ['Bad]);
export let output = "unreachable";"#
                ),
            )
            .unwrap();

            let snapshot = recovery_engine().recover_workspace(&main).unwrap();
            let messages = snapshot
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .filter(|message| matches!(*message, "diagnostic A" | "diagnostic B"))
                .collect::<Vec<_>>();
            assert_eq!(messages, ["diagnostic A", "diagnostic B"], "{annotation}");
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn recoverable_workspace_keeps_strict_recursive_types_after_runtime_failure() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"@struct type CallExpr = {args: Array(Expr)};
@struct type BinExpr = {left: Expr, right: Expr};
@enum type Expr = {Call: CallExpr, Bin: BinExpr, Text: String};
@struct type Plan(A) = {root: Expr, value: A};
def render: Fn(Expr) -> String = fn(expr) {
    match expr {
        'Call(call) => render(call.args[0]),
        'Bin(bin) => `\{render(bin.left)}\{render(bin.right)}`,
        'Text(text) => text,
    }
};
def transform: for(A) Fn(Plan(A)) -> String = fn(plan) { render(plan.root) };
def duplicate: Fn(Array(Expr)) -> Array(Expr) = fn(items) { items };
def reject: Fn(Int) -> Expr = fn(value) { fail!("expected failure", value) };
let failed = reject(1);
export let output = "unreachable";"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Partial);
        let messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert_eq!(messages, ["expected failure"]);
        for name in ["CallExpr", "BinExpr", "Expr", "Plan"] {
            let definition = snapshot
                .definitions()
                .iter()
                .find(|definition| definition.module == root.id && definition.name == name)
                .unwrap();
            assert_eq!(definition.ty.state, crate::FactState::Known, "{name}");
        }

        let dependency = directory.join("dependency.telora");
        fs::rename(&main, &dependency).unwrap();
        fs::write(
            &main,
            "import \"./dependency.telora\" as dependency; export let output = \"root\";",
        )
        .unwrap();
        let dependent = recovery_engine().recover_workspace(&main).unwrap();
        assert!(
            dependent
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "expected failure")
        );
        assert!(
            !dependent
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("cannot be partially evaluated") })
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_continues_healthy_array_slots_and_skips_failed_slots() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def first: Fn(Int) -> Int = fn(item) {
    if item == 2 { fail!("two", item) }
    else if item == 4 { fail!("four", item) }
    else { item + 10 }
};
def second: Fn(Int) -> Int = fn(item) {
    if item == 13 { fail!("three", item) } else { item + 100 }
};
export let output = [1, 2, 3, 4]
    |> array.map\(_, first)
    |> array.map\(_, second);"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Partial);
        let mut messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .filter(|message| matches!(*message, "two" | "three" | "four"))
            .collect::<Vec<_>>();
        messages.sort_unstable();
        assert_eq!(messages, ["four", "three", "two"]);

        let strict = load_module(&main, BTreeMap::new(), 100_000)
            .unwrap()
            .execute(100_000);
        assert!(
            strict.is_err(),
            "strict execution published a partial Array"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_does_not_publish_a_clean_root_after_internal_failure() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def transform: Fn(Int) -> Int = fn(item) {
    if item == 2 { fail!("two", item) } else { item + 10 }
};
export let output = match array.get(array.map([1, 2, 3], transform), 0) {
    'Some(value) => value,
    'None => 0,
};"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Partial);
        assert_eq!(
            snapshot
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message == "two")
                .count(),
            1
        );

        let strict = load_module(&main, BTreeMap::new(), 100_000)
            .unwrap()
            .execute(100_000);
        assert!(strict.is_err(), "strict execution accepted a failed world");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_retains_direct_failed_children_for_diagnostics() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def transform: Fn(Int) -> Int = fn(item) {
    if item == 2 { fail!("two", item) }
    else if item == 3 { fail!("three", item) }
    else { item }
};
export let output = array.length([1, transform(2), 1 / 0, transform(3), 4]);"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Partial);
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "two")
        );
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "three")
        );
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message.contains("division by zero"))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_filter_continues_predicates_but_fold_stops_after_failed_accumulator() {
        let directory = fixture_dir();
        let filter = directory.join("filter.telora");
        fs::write(
            &filter,
            r#"import "std/array" as array;
export let output = array.filter([1, 2, 3, 4], fn(item) {
    if item == 2 { fail!("filter-two", item) }
    else if item == 4 { fail!("filter-four", item) }
    else { item > 0 }
});"#,
        )
        .unwrap();
        let filtered = recovery_engine().recover_workspace(&filter).unwrap();
        for message in ["filter-two", "filter-four"] {
            assert_eq!(
                filtered
                    .diagnostics()
                    .iter()
                    .filter(|diagnostic| diagnostic.message == message)
                    .count(),
                1
            );
        }

        let fold = directory.join("fold.telora");
        fs::write(
            &fold,
            r#"import "std/array" as array;
export let output = array.fold([1, 2, 3], 0, fn(acc, item) {
    if item == 2 { fail!("fold-stop", item) }
    else if item == 3 { fail!("fold-must-not-run", item) }
    else { acc + item }
});"#,
        )
        .unwrap();
        let folded = recovery_engine().recover_workspace(&fold).unwrap();
        assert!(
            folded
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "fold-stop")
        );
        assert!(
            !folded
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "fold-must-not-run")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_non_shape_array_operations_propagate_without_type_cascades() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def pieces: Fn(Int) -> Array(Int) = fn(item) {
    if item == 2 { fail!("piece-two", item) } else { [item] }
};
let flattened = array.flat_map([1, 2, 3], pieces);
let concatenated = array.concat([[0], flattened, [4]]);
let independent = array.map([5, 6], fn(item) {
    if item == 6 { fail!("independent-six", item) } else { item }
});
export let output = array.length(concatenated) + array.length(independent);"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert!(messages.contains(&"piece-two"), "{messages:#?}");
        assert!(messages.contains(&"independent-six"), "{messages:#?}");
        assert!(
            !messages.iter().any(|message| {
                message.contains("concat item")
                    || message.contains("flat_map callback")
                    || message.contains("expected Func")
            }),
            "{messages:#?}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_data_consumers_do_not_observe_failed_children_as_values() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def reject: Fn(Int) -> Int = fn(item) { fail!("nested", item) };
let failed: Array(Int) = array.map([1], reject);
let compared = failed == [1];
let selected = failed[0] == 1;
export let output = (compared, selected);"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            messages
                .iter()
                .filter(|message| **message == "nested")
                .count(),
            1,
            "{messages:#?}"
        );
        assert!(
            !messages.iter().any(|message| {
                message.contains("expected") || message.contains("non-exhaustive")
            }),
            "{messages:#?}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_records_panic_and_continues_independent_bindings() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            "let failed = panic!(\"broken\");\nlet independent = 2 + 3;\nexport { failed, independent };",
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Partial);
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.message == "broken"
                && diagnostic.labels[0].location.source == root.source.unwrap()
        }));
        assert_eq!(
            snapshot
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message == "broken")
                .count(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_links_json_and_core_values() {
        let directory = fixture_dir();
        let data = directory.join("data.json");
        let model = directory.join("model.telora");
        let main = directory.join("main.telora");
        fs::write(&data, r#"{"kind":"int"}"#).unwrap();
        fs::write(&model, "type Shared = String; export { Shared };").unwrap();
        fs::write(
            &main,
            "import \"./data.json\" as data;\
             import \"./model.telora\" as model;\
             import \"std/attributes\" as attributes;\
             type FromData = if data.kind == \"int\" { Int } else { String };\
             type FromTelora = model.Shared;\
             type FromCore = attributes.strip(String);\
             type Broken = missing(Int);\
             export { FromData as output };",
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        let fact = |name: &str| {
            &snapshot
                .definitions()
                .iter()
                .find(|definition| definition.module == root.id && definition.name == name)
                .unwrap()
                .ty
        };
        for (name, expected) in [
            ("FromData", "Int"),
            ("FromTelora", "String"),
            ("FromCore", "String"),
        ] {
            assert_eq!(fact(name).state, crate::FactState::Known, "{name}");
            assert_eq!(
                snapshot.types().display(fact(name).value.unwrap()).unwrap(),
                expected
            );
        }
        assert!(snapshot.modules().iter().any(|module| {
            module.kind == WorkspaceModuleKind::Json && module.state == WorkspaceModuleState::Known
        }));
        assert!(snapshot.modules().iter().any(|module| {
            module.kind == WorkspaceModuleKind::Core && module.state == WorkspaceModuleState::Known
        }));
        assert_eq!(
            snapshot
                .module_by_path(&canonicalize(&model).unwrap())
                .unwrap()
                .state,
            WorkspaceModuleState::Known
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_retains_module_cycles_once() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let a = directory.join("a.telora");
        let b = directory.join("b.telora");
        fs::write(&main, "import \"./a.telora\" as a; export { a as output };").unwrap();
        fs::write(
            &a,
            "import \"./b.telora\" as b; type A = String; export { A };",
        )
        .unwrap();
        fs::write(
            &b,
            "import \"./a.telora\" as a; type B = String; export { B };",
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        assert_eq!(
            snapshot
                .modules()
                .iter()
                .filter(|module| module.kind == WorkspaceModuleKind::Telora)
                .filter(|module| module.state == WorkspaceModuleState::Unavailable)
                .count(),
            2
        );
        assert_eq!(
            snapshot
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("module cycle"))
                .count(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_json_parses_and_decodes_strings_with_blame_results() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/json" as json;
               import "std/result" as result;
               export let output = {
                   parsed: result.unwrap(json.parse("{\"answer\": 42}")),
                   decoded: result.unwrap(json.decode(Int, "42")),
                   failed: json.parse("{")
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("parsed: {answer: 42}"), "{output}");
        assert!(output.contains("decoded: 42"), "{output}");
        assert!(output.contains("failed: 'Err("), "{output}");
        assert!(output.contains("data: \"{\""), "{output}");
        assert!(output.contains("rule: 'Json"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn regex_native_values_validate_structs_and_drive_typed_decode() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/regex" as re;
               import "std/result" as result;
               import "std/string" as string;
               let pattern = re.compile(r"^(?P<name>\w+)=(?P<value>\d+)(?:;(?P<unit>\w+))?$");
               @re.parse_by(pattern)
               @struct type Rec = {
                   name: String,
                   value: Int,
                   unit: Option(String),
               };
               {
                   matched: re.is_match(pattern, "answer=42"),
                   equal: pattern == re.compile(r"^(?P<name>\w+)=(?P<value>\d+)(?:;(?P<unit>\w+))?$"),
                   text: result.unwrap(string.parse(String, "plain")),
                   number: result.unwrap(string.parse(Int, "42")),
                   float: result.unwrap(string.parse(Float, "1.5")),
                   first: result.unwrap(string.parse(Rec, "answer=42")),
                   second: result.unwrap(string.parse(Rec, "size=7;px")),
                   failed: string.parse(Rec, "not a record"),
                   bad_int: string.parse(Int, "4x"),
                   bad_float: string.parse(Float, "1.5x"),
               }"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 500_000).unwrap();
        let output = module.execute(500_000).unwrap().to_string();
        assert!(output.contains("matched: 'True"), "{output}");
        assert!(output.contains("equal: 'True"), "{output}");
        assert!(output.contains("text: \"plain\""), "{output}");
        assert!(output.contains("number: 42"), "{output}");
        assert!(output.contains("float: 1.5"), "{output}");
        assert!(
            output.contains("first: {name: \"answer\", unit: 'None, value: 42}"),
            "{output}"
        );
        assert!(
            output.contains("second: {name: \"size\", unit: 'Some(\"px\"), value: 7}"),
            "{output}"
        );
        assert!(output.contains("failed: 'Err("), "{output}");
        assert!(output.contains("bad_int: 'Err("), "{output}");
        assert!(output.contains("bad_float: 'Err("), "{output}");

        fs::write(
            &main,
            r#"import "std/regex" as re;
               @re.parse_by(re.compile(r"(?P<name>\w+)"))
               @struct type Bad = { value: Int };
               { Bad }"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 500_000).unwrap_err();
        assert!(
            error
                .message()
                .contains("captures must match struct fields")
        );

        fs::write(&main, r#"import "std/regex" as re; re.compile(r"(")"#).unwrap();
        let module = load_module(&main, BTreeMap::new(), 500_000).unwrap();
        let error = module.execute(500_000).unwrap_err();
        assert!(error.message.contains("invalid regular expression"));

        fs::write(
            &main,
            r#"import "std/regex" as re;
               import "std/result" as result;
               import "std/string" as string;
               @re.parse_by(re.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
               @struct type Endpoint = { host: String, port: Int };
               @re.parse_by(re.compile(r"^(?P<name>\w+)@(?P<endpoint>.+)$"))
               @struct type Service = { name: String, endpoint: Endpoint };
               export let output = result.unwrap(string.parse(Service, "api@localhost:8080"));"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(engine.execute(&module).unwrap()).to_string();
        assert!(
            output.contains("{endpoint: {host: \"localhost\", port: 8080}, name: \"api\"}"),
            "{output}"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn display_templates_validate_once_and_compose_nested_types() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{host}:{port}")
               @struct type Endpoint = { host: String, port: Int };
               @fmt.display_by("{name}@{endpoint} {{ready}} {ratio} {name}")
               @struct type Service = { name: String, endpoint: Endpoint, ratio: Float };
               export let output = fmt.display(Service, {
                   name: "api",
                   endpoint: { host: "localhost", port: 8080 },
                   ratio: -0.0,
               });"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(
            named_output(engine.execute(&module).unwrap()).to_string(),
            "\"api@localhost:8080 {ready} -0 api\""
        );

        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{missing}")
               @struct type Bad = { value: Int };
               export { Bad };"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(error.message().contains("unknown field \"missing\""));

        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{value")
               @struct type Bad = { value: Int };
               export { Bad };"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(error.message().contains("unclosed Display template field"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn container_text_codec_bridge_round_trips_nested_values_and_schema() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/codec" as codec;
               import "std/fmt" as fmt;
               import "std/json" as json;
               import "std/regex" as re;
               import "std/result" as result;
               import "std/string" as string;

               @string.decode_by_parse
               @string.encode_by_display
               @fmt.display_by("{host}:{port}")
               @re.parse_by(re.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
               @struct type Endpoint = { host: String, port: Int };

               @struct type Config = { endpoint: Endpoint, name: String };
               let decoded = result.unwrap(codec.decode(Config, {
                   endpoint: "localhost:8080",
                   name: "dev",
               }));
               export let output = {
                   decoded,
                   encoded: result.unwrap(codec.encode(Config, decoded)),
                   direct: result.unwrap(codec.decode(Endpoint, "example.com:443")),
                   schema: json.schema(Config),
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(engine.execute(&module).unwrap()).to_string();
        assert!(
            output
                .contains("decoded: {endpoint: {host: \"localhost\", port: 8080}, name: \"dev\"}"),
            "{output}"
        );
        assert!(
            output.contains("encoded: {endpoint: \"localhost:8080\", name: \"dev\"}"),
            "{output}"
        );
        assert!(
            output.contains("direct: {host: \"example.com\", port: 443}"),
            "{output}"
        );
        assert!(output.contains("endpoint: {type: \"string\"}"), "{output}");

        fs::write(
            &main,
            r#"import "std/codec" as codec;
               import "std/regex" as re;
               import "std/string" as string;
               @string.decode_by_parse
               @re.parse_by(re.compile(r"^(?P<value>\d+)$"))
               @struct type Bad = { value: Int };
               export let output = codec.decode(Bad, "42");"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("must be used together"), "{output}");

        fs::write(
            &main,
            r#"import "std/string" as string;
               @struct type Bad = {
                   @string.decode_by_parse
                   value: String,
               };
               export { Bad };"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(
            error
                .message()
                .contains("only supported on a type container")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dependency_imports_preserve_identity_across_relative_edges() {
        let directory = fixture_dir();
        let app = directory.join("app");
        let models = directory.join("models");
        fs::create_dir(&app).unwrap();
        fs::create_dir(&models).unwrap();
        fs::write(
            directory.join("telora-deps.json"),
            r#"{"dependencies":{"models":{"path":"models"}}}"#,
        )
        .unwrap();
        fs::write(models.join("base.telora"), "export let answer = 42;").unwrap();
        fs::write(
            models.join("user.telora"),
            "import \"./base.telora\" as base; export { base as base };",
        )
        .unwrap();
        let main = app.join("main.telora");
        fs::write(
            &main,
            "import \"models/user.telora\" as user; export let output = user.base.answer;",
        )
        .unwrap();

        let engine = recovery_engine();
        let loaded = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(
            named_output(engine.execute(&loaded).unwrap()).to_string(),
            "42"
        );
        let names = loaded
            .workspace
            .modules()
            .iter()
            .map(|module| module.name.as_str())
            .collect::<HashSet<_>>();
        assert!(names.contains("models/user.telora"));
        assert!(names.contains("models/base.telora"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn telora_show_interpreter_renders_supported_values() {
        let directory = fixture_dir();
        fs::write(
            directory.join("reference-show.telora"),
            include_str!("../../../examples/reference-show.telora"),
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./reference-show.telora" as show;
               @struct type User = {name: String, scores: Array(Int)};
               @enum type Choice = {None: 'None, Some: String};
               type Pair = Tuple([Int, String]);
               type Unary = Fn(Int) -> Int;
               let user: User = {name: "Ada", scores: [2, 3]};
               let none: Choice = 'None;
               let some: Choice = 'Some("x");
               {
                   inferred: show.my_show(Int)(42),
                   explicit: show.my_show@[Int](Int)(42),
                   string: show.my_show(String)("a\"b\\c"),
                   array: show.my_show(Array(Int))([1, 2]),
                   tuple: show.my_show(Pair)((1, "x")),
                   record: show.my_show(User)(user),
                   atom: show.my_show(Choice)(none),
                   tagged: show.my_show(Choice)(some),
                   function_error: show.my_show(Unary)(fn(value) { value }),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 500_000).unwrap();
        let Value::Dict(output) = module.execute(500_000).unwrap() else {
            panic!("show interpreter test must return a Dict")
        };
        for (field, expected) in [
            ("inferred", "'Ok(\"42\")"),
            ("explicit", "'Ok(\"42\")"),
            ("string", "'Ok(\"\\\"a\\\\\\\"b\\\\\\\\c\\\"\")"),
            ("array", "'Ok(\"[1, 2]\")"),
            ("tuple", "'Ok(\"(1, \\\"x\\\")\")"),
            ("record", "'Ok(\"{name: \\\"Ada\\\", scores: [2, 3]}\")"),
            ("atom", "'Ok(\"'None\")"),
            ("tagged", "'Ok(\"'Some(\\\"x\\\")\")"),
        ] {
            assert_eq!(output.get(field).unwrap().to_string(), expected, "{field}");
        }
        let Value::Tagged { tag, payload } = output.get("function_error").unwrap() else {
            panic!("unsupported Function must return blame")
        };
        assert_eq!(tag.name(), "Err");
        assert_eq!(
            payload.to_string(),
            "\"unsupported my_show descriptor: Func\""
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn telora_hash_interpreter_threads_state_and_distinguishes_structure() {
        let directory = fixture_dir();
        fs::write(
            directory.join("reference-hash.telora"),
            include_str!("../../../examples/reference-hash.telora"),
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./reference-hash.telora" as reference;
               import "std/hash" as hash;
               @struct type User = {name: String, scores: Array(Int)};
               @struct type Renamed = {label: String, scores: Array(Int)};
               @enum type Choice = {None: 'None, Some: String};
               type Pair = Tuple([Int, Int]);
               type Unary = Fn(Int) -> Int;
               let user: User = {name: "Ada", scores: [2, 3]};
               let changed: User = {name: "Ada", scores: [2, 4]};
               let renamed: Renamed = {label: "Ada", scores: [2, 3]};
               let state = hash.new();
               let first = reference.my_hash(User)(user, state);
               {
                   equal: first == reference.my_hash(User)(user, state),
                   changed: first == reference.my_hash(User)(changed, state),
                   field_name: first == reference.my_hash(Renamed)(renamed, state),
                   array_tuple: reference.my_hash(Array(Int))([1, 2], state) ==
                       reference.my_hash(Pair)((1, 2), state),
                   tag_payload: reference.my_hash(Choice)('None, state) ==
                       reference.my_hash(Choice)('Some(""), state),
                   alias_unchanged: hash.finish(state) == hash.finish(hash.new()),
                   function_error: reference.my_hash(Unary)(fn(value) { value }, state),
                   float_error: reference.my_hash(Float)(1.5, state),
                   opaque_error: reference.my_hash(hash.HashState)(state, state),
                   recursive_error: reference.my_hash(Array(Float))([1.0, 2.0], state),
               }"#,
        )
        .unwrap();
        let module =
            load_module(directory.join("main.telora"), BTreeMap::new(), 1_000_000).unwrap();
        let Value::Dict(output) = module.execute(1_000_000).unwrap() else {
            panic!("hash interpreter test must return a Dict")
        };
        for field in ["equal", "alias_unchanged"] {
            assert_eq!(output.get(field).unwrap().to_string(), "'True", "{field}");
        }
        for field in ["changed", "field_name", "array_tuple", "tag_payload"] {
            assert_eq!(output.get(field).unwrap().to_string(), "'False", "{field}");
        }
        for field in [
            "function_error",
            "float_error",
            "opaque_error",
            "recursive_error",
        ] {
            assert!(
                output.get(field).unwrap().to_string().starts_with("'Err("),
                "{field}"
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_diagnostics_collect_without_implicit_host_publication() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let data = directory.join("project.json");
        fs::write(
            directory.join("explicit-diagnostics.telora"),
            include_str!("../../../examples/explicit-diagnostics.telora"),
        )
        .unwrap();
        fs::write(
            &data,
            r#"{"name":"","packages":[{"name":"","version":0},{"name":"ok","version":1}]}"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./explicit-diagnostics.telora" as validation;
import "std/array" as arrays;
import "./project.json" as project;
let initial: Array(validation.DiagnosticRecord) = [];
let output = match validation.validate_project(project, initial) {
    (checked, diagnostics) => {
        count: arrays.length(diagnostics),
        initial_count: arrays.length(initial),
        unchanged: checked == project,
        messages: arrays.map(diagnostics, fn(item) { item.message }),
    },
};
export { output };"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 500_000).unwrap();
        let Value::Dict(output) = named_output(module.execute(500_000).unwrap()) else {
            panic!("diagnostic collection test must return a Dict")
        };
        assert_eq!(output.get("count").unwrap().to_string(), "3");
        assert_eq!(output.get("initial_count").unwrap().to_string(), "0");
        assert_eq!(output.get("unchanged").unwrap().to_string(), "'True");
        let Value::Array(messages) = output.get("messages").unwrap() else {
            panic!("messages must be an Array")
        };
        assert_eq!(
            messages.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "\"project name must not be empty\"",
                "\"package name must not be empty\"",
                "\"package version must be positive\"",
            ]
        );

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        assert!(snapshot.diagnostics().is_empty());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fail_intrinsic_preserves_data_and_authored_rule_locations() {
        let directory = fixture_dir();
        fs::write(directory.join("user.json"), r#"{"age":42}"#).unwrap();
        let source = r#"import "./user.json" as user;
import "std/result" as result;
import "std/dyn" as dyn;
@struct type User = {age: Int};
def inspect_i: Fn(Dyn) -> Int = fn(value) {
    match dyn.field(value, "age") {
        'Ok(age) => fail!("age rejected", age),
        'Err(error) => fail!(error.message, error),
    }
};
def inspect: for(A) Fn(TypeOf(A)) -> Fn(A) -> Int = interpreter!(inspect_i);
inspect(User)(user)"#;
        fs::write(directory.join("main.telora"), source).unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(error.message.contains("age rejected"));
        let data = error.data_location().expect("blame data location");
        assert_eq!(
            module.sources.get(data.source).name.as_ref(),
            directory.join("user.json").display().to_string()
        );
        assert_eq!(
            module.sources.get(data.source).slice(data).as_deref(),
            Some("42")
        );
        let rule = error.rule_location().expect("blame rule location");
        assert_eq!(
            module.sources.get(rule.source).slice(rule).as_deref(),
            Some("fail!(\"age rejected\", age)")
        );
        let rendered = error.to_string();
        assert!(rendered.contains("user.json:1:8"), "{rendered}");
        assert!(rendered.contains("main.telora:7:21"), "{rendered}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interpreter_lifts_parameters_independently() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"def unary_i: Fn(Dyn) -> String = fn(value) { "unary" };
               def unary: for(A) Fn(TypeOf(A)) -> Fn(A) -> String = interpreter!(unary_i);

               def mixed_i: Fn(Dyn, Bool) -> String = fn(value, verbose) { "mixed" };
               def mixed: for(A) Fn(TypeOf(A)) -> Fn(A, Bool) -> String = interpreter!(mixed_i);

               def many_i: Fn(String, Dyn, Bool, Dyn, Dyn) -> String =
                   fn(prefix, a, flag, b, again_a) { prefix };
               def many: for(A, B) Fn(TypeOf(B), TypeOf(A)) ->
                   Fn(String, A, Bool, B, A) -> String = interpreter!(many_i);

               def metadata_i: Fn(Bool) -> String = fn(flag) { "metadata" };
               def metadata: for(A) Fn(TypeOf(A)) -> Fn(Bool) -> String =
                   interpreter!(metadata_i);

               {
                   unary: unary(Int)(1),
                   mixed: mixed(Int)(1, 'True),
                   many: many(Int, String)("many", "a", 'False, 2, "b"),
                   metadata: metadata(String)('True),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 200_000).unwrap();
        let Value::Dict(output) = module.execute(200_000).unwrap() else {
            panic!("parameter-wise interpreter test must return a Dict")
        };
        for (field, expected) in [
            ("unary", "\"unary\""),
            ("mixed", "\"mixed\""),
            ("many", "\"many\""),
            ("metadata", "\"metadata\""),
        ] {
            assert_eq!(output.get(field).unwrap().to_string(), expected);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn should_and_must_ok_select_warning_or_failure() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"def reject: Fn(String) -> Result(String, String) = fn(value) { 'Err("deprecated") };
export let output = reject.should_ok!("old");"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            named_output(module.execute(100_000).unwrap()).to_string(),
            "'None"
        );
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.severity == crate::source::Severity::Warning
                && diagnostic.message == "deprecated"
        }));

        fs::write(
            &main,
            r#"def reject: Fn(Int) -> Result(Int, String) = fn(value) { 'Err("invalid") };
export let output = reject.must_ok!(42);"#,
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let error = snapshot
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.message == "invalid")
            .expect("reported error");
        assert_eq!(error.severity, crate::source::Severity::Error);
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module.execute(100_000).unwrap_err();
        assert_eq!(failure.kind, crate::RuntimeErrorKind::RaisedBlame);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn intelligent_reporting_collects_independent_domain_diagnostics() {
        let examples =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/intelligent-reporting");
        let example = examples.join("invalid.telora");
        let snapshot = recovery_engine().recover_workspace(&example).unwrap();
        let diagnostics = snapshot
            .diagnostics()
            .iter()
            .filter(|diagnostic| {
                diagnostic.message.contains("not compatible")
                    || diagnostic.message.contains("not available")
                    || diagnostic.message.contains("expands the measure grain")
                    || diagnostic.message.contains("ordered dimension")
            })
            .collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 4, "{diagnostics:#?}");
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.labels.len() >= 2)
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("not available"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("expands the measure grain"))
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("ordered dimension"))
        );
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("render field missing_render_field is absent")
        }));

        let blocked = recovery_engine()
            .recover_workspace(examples.join("invalid-measures.telora"))
            .unwrap();
        let messages = blocked
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("incompatible grains"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("ordered dimension"))
        );
        assert!(!messages.iter().any(|message| {
            message.contains("not compatible") || message.contains("expands the measure grain")
        }));

        let host =
            load_module(examples.join("host-plan.telora"), BTreeMap::new(), 500_000).unwrap();
        let encoded = named_output(host.execute(500_000).unwrap()).to_string();
        assert!(encoded.contains("parameters"), "{encoded}");
        assert!(encoded.contains("customer_region"), "{encoded}");
        assert!(encoded.contains("result_schema"), "{encoded}");
        assert!(encoded.contains("money_cents"), "{encoded}");
        assert!(encoded.contains("render"), "{encoded}");
        assert!(encoded.contains("analytics-policy-v1"), "{encoded}");

        let restricted = recovery_engine()
            .recover_workspace(examples.join("invalid-restriction.telora"))
            .unwrap();
        let unauthorized = restricted
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("unauthorized entity"))
            .collect::<Vec<_>>();
        assert_eq!(unauthorized.len(), 2, "{unauthorized:#?}");
        for diagnostic in unauthorized {
            let source_names = diagnostic
                .labels
                .iter()
                .map(|label| {
                    restricted
                        .sources()
                        .get(label.location.source)
                        .name
                        .as_ref()
                })
                .collect::<Vec<_>>();
            assert!(
                source_names
                    .iter()
                    .any(|name| name.ends_with("invalid-restriction.telora")),
                "{source_names:#?}"
            );
            assert!(
                source_names
                    .iter()
                    .any(|name| name.ends_with("restricted.json")),
                "{source_names:#?}"
            );
            assert!(
                source_names
                    .iter()
                    .any(|name| name.ends_with("b2b-model.telora")),
                "{source_names:#?}"
            );
        }
    }

    #[test]
    fn executable_type_metadata_constructors_cross_module_boundaries() {
        let examples =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ontology-construction");
        let valid = load_module(examples.join("valid.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = valid.execute(100_000).unwrap().to_string();
        assert!(output.contains("Revenue"), "{output}");
        assert!(output.contains("Order"), "{output}");

        let invalid = load_module(examples.join("invalid.telora"), BTreeMap::new(), 100_000)
            .unwrap_err()
            .to_string();
        assert!(invalid.contains("field lower"), "{invalid}");
        assert!(invalid.contains("String"), "{invalid}");

        let missing = recovery_engine()
            .recover_workspace(examples.join("missing.telora"))
            .unwrap();
        let diagnostic = missing
            .diagnostics()
            .iter()
            .find(|diagnostic| {
                diagnostic
                    .message
                    .contains("no ontology capability is defined for Units")
            })
            .expect("missing capability diagnostic");
        assert!(diagnostic.labels.len() >= 2, "{diagnostic:#?}");
        let names = diagnostic
            .labels
            .iter()
            .map(|label| missing.sources().get(label.location.source).name.as_ref())
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name.ends_with("missing.telora")));
        assert!(names.iter().any(|name| name.ends_with("ontology.telora")));
    }

    #[test]
    fn b2c_model_reuses_ontology_methods_with_independent_diagnostics() {
        let examples =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/b2c-reporting");
        let valid = load_module(examples.join("valid.telora"), BTreeMap::new(), 300_000).unwrap();
        let output = valid.execute(300_000).unwrap().to_string();
        assert!(output.contains("b2c-model-v1"), "{output}");
        assert!(output.contains("campaigns"), "{output}");
        assert!(output.contains("regions"), "{output}");

        let invalid = recovery_engine()
            .recover_workspace(examples.join("invalid.telora"))
            .unwrap();
        let messages = invalid
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert!(
            messages.iter().any(
                |message| message.contains("no ontology capability is defined for LoyaltyTier")
            ),
            "{messages:#?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("expands the measure grain")),
            "{messages:#?}"
        );
    }

    #[test]
    fn third_enterprise_builds_a_model_from_the_ontology_dsl_tutorial() {
        let examples =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/saas-support-reporting");
        let valid = load_module(examples.join("valid.telora"), BTreeMap::new(), 300_000).unwrap();
        let output = valid.execute(300_000).unwrap().to_string();
        assert!(output.contains("saas-support-v1"), "{output}");
        assert!(output.contains("ResolvedTickets"), "{output}");
        assert!(output.contains("plans"), "{output}");
        assert!(output.contains("teams"), "{output}");
        assert!(output.contains("sla_policies"), "{output}");

        let invalid = recovery_engine()
            .recover_workspace(examples.join("invalid.telora"))
            .unwrap();
        let messages = invalid
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert!(
            messages.iter().any(|message| {
                message.contains("no ontology capability is defined for CustomerSegment")
            }),
            "{messages:#?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("expands the measure grain")),
            "{messages:#?}"
        );
    }
}
