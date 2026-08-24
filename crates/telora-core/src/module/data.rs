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
                constraints: Vec::new(),
                body: descriptor.clone(),
            },
        )]),
        concrete_types: BTreeMap::from([("Value".into(), descriptor)]),
        traits: BTreeMap::new(),
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
