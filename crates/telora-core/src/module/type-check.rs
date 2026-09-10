// This loader owns syntax and static interfaces only. It cannot execute Telora.
struct StaticWorkspace<'a> {
    graph: &'a ModuleGraph,
    resolved: Vec<Option<ResolvedStaticModule>>,
    sources: &'a mut SourceDatabase,
    interfaces: HashMap<ModuleId, ModuleInterface>,
    visiting: HashSet<ModuleId>,
    inputs: BTreeMap<String, SemanticModuleInput>,
    native_ids: HashMap<ModuleCName, u32>,
    type_store: TypeStore,
}

impl StaticWorkspace<'_> {
    fn solve(&mut self, id: ModuleId) -> Result<(), ModuleError> {
        if self.interfaces.contains_key(&id) {
            return Ok(());
        }
        let module = self.graph.module(id);
        if self.inputs.contains_key(&module.cname.to_string()) {
            return Err(ModuleError::new(format!(
                "type solving failed for {}",
                module.cname
            )));
        }
        if !self.visiting.insert(id) {
            return Err(ModuleError::new(format!(
                "cyclic module import at {}",
                module.cname
            )));
        }
        if let Some(resolved) = &self.graph.resolved[id.index()]
            && resolved.format != ModuleFormat::Telora
        {
            let value_id = self
                .graph
                .id(&ModuleCName::builtin(crate::core::VALUE_MODULE))
                .expect("builtin inventory");
            self.solve(value_id)?;
            let value = &self.interfaces[&value_id];
            let descriptor = match value.exports.get("Value").map(|scheme| &scheme.body) {
                Some(TypeDescriptor::TypeOf(descriptor)) => descriptor.as_ref().clone(),
                _ => return Err(ModuleError::new("std/value has no static Value type")),
            };
            let path = resolved
                .path()
                .ok_or_else(|| ModuleError::new("data module has no path"))?;
            // Data contents cannot affect the static contract. Parsing, limits
            // and content diagnostics belong to the later data-loading phase.
            let kind = static_data_kind(resolved.format)
                .ok_or_else(|| ModuleError::new("unsupported static data format"))?;
            let interface = static_data_interface(descriptor);
            self.inputs.insert(
                module.cname.to_string(),
                SemanticModuleInput {
                    key: module.cname.to_string(),
                    path: Some(path.to_owned()),
                    kind,
                    source: None,
                    result_location: None,
                    analysis: None,
                    partial: None,
                    interface: Some(SemanticModuleInterface::new(&interface)),
                    state: WorkspaceModuleState::Available,
                    imports: Vec::new(),
                    diagnostics: Vec::new(),
                },
            );
            self.visiting.remove(&id);
            self.interfaces.insert(id, interface);
            return Ok(());
        }
        let prepared = module.prepared.as_ref().ok_or_else(|| {
            ModuleError::new(format!(
                "static checking has no source for {}",
                module.cname
            ))
        })?;
        let mut diagnostics = prepared.diagnostics.clone();
        let mut imports = BTreeMap::new();
        let mut imported_types = HashMap::new();
        let mut interface = None;
        let mut semantic_interface = None;
        if let Some(program) = &prepared.program {
            let resolution = self.resolved[id.index()].take()
                .expect("module references were resolved before type solving");
            diagnostics.extend(resolution.diagnostics);
            if !self.native_ids.contains_key(&module.cname) {
                for binding in &program.value.body.value.bindings {
                    if matches!(
                        binding.value.kind,
                        BindingKind::Native | BindingKind::NativeType
                    ) {
                        diagnostics.push(Diagnostic::error(
                            "native declarations are only allowed in built-in std modules",
                            binding.location,
                        ));
                    }
                }
            }
            for edge in &module.imports {
                if let Err(error) = self.solve(edge.target) {
                    diagnostics.push(Diagnostic::error(error.to_string(), program.location));
                }
            }
            if diagnostics.is_empty() {
                for (local, target) in resolution.imports {
                    let interface = &self.interfaces[&target.module()];
                    let selected = match target.exported_name(self.graph) {
                        Some(exported) => select_import_interface(interface.clone(), exported, &local)?,
                        None => interface.clone(),
                    };
                    imports.insert(local, selected);
                }
            }
            if let Some(native_id) = self.native_ids.get(&module.cname) {
                for (_, (name, ty)) in declared_native_types(
                    program,
                    crate::value::NativeModuleId(*native_id),
                    &module.cname.to_string(),
                    self.sources,
                )? {
                    imported_types.insert(
                        name,
                        TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Opaque(ty))),
                    );
                }
            }
            if diagnostics.is_empty() {
                let dependency_facts = module.imports.iter()
                    .map(|edge| self.interfaces[&edge.target].type_facts()).collect::<Vec<_>>();
                match crate::types::check_module_types(
                    id,
                    self.sources,
                    prepared.source_id,
                    program,
                    resolution.hir,
                    imported_types,
                    imports,
                    &dependency_facts,
                    &mut self.type_store,
                ) {
                    Ok(solved) => {
                        semantic_interface = Some(SemanticModuleInterface::with_types(&solved.interface, solved.types));
                        interface = Some(solved.interface);
                    }
                    Err(error) => diagnostics.push(
                        error
                            .diagnostic
                            .map(|diagnostic| *diagnostic)
                            .unwrap_or_else(|| Diagnostic::error(error.message, program.location)),
                    ),
                }
            }
        }
        self.visiting.remove(&id);
        self.inputs.insert(
            module.cname.to_string(),
            SemanticModuleInput {
                key: module.cname.to_string(),
                path: self.graph.resolved[id.index()]
                    .as_ref()
                    .and_then(|module| module.path())
                    .map(Path::to_owned),
                kind: if self.native_ids.contains_key(&module.cname) {
                    WorkspaceModuleKind::Core
                } else {
                    WorkspaceModuleKind::Telora
                },
                source: Some(prepared.source_id),
                result_location: prepared
                    .program
                    .as_ref()
                    .map(|p| p.value.body.value.result.location),
                analysis: None,
                partial: None,
                interface: semantic_interface,
                state: if interface.is_some() {
                    WorkspaceModuleState::Available
                } else {
                    WorkspaceModuleState::Unavailable
                },
                imports: Vec::new(),
                diagnostics,
            },
        );
        let interface = interface
            .ok_or_else(|| ModuleError::new(format!("type solving failed for {}", module.cname)))?;
        self.interfaces.insert(id, interface);
        Ok(())
    }
}

impl Engine {
    /// Solve the reachable module graph without constructing a VM or runtime heap.
    pub fn check_types_with_resolver(
        &self,
        resolver: ModuleResolver,
    ) -> Result<WorkspaceSnapshot, ModuleError> {
        let resolver = resolver.with_builtins(builtin_list());
        let root = resolver
            .selected_root()
            .map_err(|error| ModuleError::new(error.to_string()))?;
        let specs = module_specs();
        let synthetic = specs
            .iter()
            .map(|spec| {
                (
                    ModuleCName::builtin(spec.name),
                    (PathBuf::from(spec.name), spec.source.to_owned()),
                )
            })
            .collect();
        let mut sources = SourceDatabase::default();
        let graph = ModuleGraph::discover(
            &resolver,
            vec![root.clone()],
            &synthetic,
            std::iter::empty(),
            None,
            true,
            &mut sources,
        )?;
        // Resolve every reachable source before solving any dependency. Module
        // solvers consume this inventory rather than rebuilding HIR from types.
        let resolved = StaticNames::new(&graph).resolve(graph.id(&root.id).expect("discovered root"));
        let native_ids = specs.iter().map(|spec|
            (ModuleCName::builtin(spec.name), spec.native_id)).collect();
        if let Some(inputs) = resolved.diagnostic_inputs(&graph, &native_ids) {
            return Ok(WorkspaceSnapshot::build(sources, inputs));
        }
        let mut workspace = StaticWorkspace {
            graph: &graph,
            resolved: resolved.modules,
            sources: &mut sources,
            interfaces: HashMap::new(),
            visiting: HashSet::new(),
            inputs: BTreeMap::new(),
            native_ids,
            type_store: TypeStore::default(),
        };
        let result = workspace.solve(graph.id(&root.id).expect("discovered root"));
        if workspace
            .inputs
            .values()
            .all(|input| input.diagnostics.is_empty())
        {
            result?;
        }
        let inputs = workspace.inputs.into_values().collect();
        Ok(WorkspaceSnapshot::build(sources, inputs))
    }
}
