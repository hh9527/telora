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
                | BindingKind::Type
                | BindingKind::Trait
                | BindingKind::Impl => {
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
                BindingKind::Type | BindingKind::Trait
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
            for implementation in &module.interface.trait_implementations {
                let evidence = module
                    .root
                    .export_get(&main.heap, &implementation.dictionary)
                    .map_err(|error| ModuleError::new(error.to_string()))?
                    .ok_or_else(|| {
                        ModuleError::new(format!(
                            "built-in module {request:?} is missing trait implementation root {:?}",
                            implementation.dictionary
                        ))
                    })?;
                external_roots
                    .entry(implementation.dictionary.clone())
                    .or_insert(evidence);
            }
            for property in &module.interface.type_properties {
                let evidence = module
                    .root
                    .export_get(&main.heap, &property.root)
                    .map_err(|error| ModuleError::new(error.to_string()))?
                    .ok_or_else(|| {
                        ModuleError::new(format!(
                            "built-in module {request:?} is missing type property root {:?}",
                            property.root
                        ))
                    })?;
                external_roots
                    .entry(property.root.clone())
                    .or_insert(evidence);
            }
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
        let mut runtime_program = program.clone();
        if let ExprKind::Dict(fields) = &mut runtime_program.value.body.value.result.value {
            let location = runtime_program.value.body.value.result.location;
            for published in &analysis.module_interface.trait_implementations {
                let source = analysis
                    .trait_implementations
                    .iter()
                    .find(|implementation| implementation.id == published.id)
                    .map_or(published.dictionary.as_str(), |implementation| {
                        implementation.dictionary.as_str()
                    });
                fields.push(located(
                    DictFieldKind {
                        decorators: Vec::new(),
                        name: Some(located(published.dictionary.clone(), location)),
                        value: located(
                            ExprKind::Variable(located(source.to_owned(), location)),
                            location,
                        ),
                    },
                    location,
                ));
            }
            for property in &analysis.module_interface.type_properties {
                fields.push(located(
                    DictFieldKind {
                        decorators: Vec::new(),
                        name: Some(located(property.root.clone(), location)),
                        value: located(
                            ExprKind::Variable(located(property.root.clone(), location)),
                            location,
                        ),
                    },
                    location,
                ));
            }
        }
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
            &runtime_program,
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
            traits: interface
                .traits
                .get(&exported.value)
                .copied()
                .map(|id| BTreeMap::from([(local.to_owned(), id)]))
                .unwrap_or_default(),
            trait_implementations: interface.trait_implementations,
            type_properties: interface.type_properties,
            display_trait: interface.display_trait,
            type_family_templates: interface
                .type_family_templates
                .get(&exported.value)
                .cloned()
                .map(|family| BTreeMap::from([(local.to_owned(), family)]))
                .unwrap_or_default(),
        },
    ))
}
