fn expression_descends_from(
    hir: &HirProgram,
    mut expression: HirExpressionId,
    root: HirExpressionId,
) -> bool {
    loop {
        if expression == root {
            return true;
        }
        let Some(parent) = hir
            .expression(expression)
            .and_then(|expression| expression.parent)
        else {
            return false;
        };
        expression = parent;
    }
}

fn dependency_reaches(
    graph: &SemanticDependencyGraph,
    current: HirDefinitionId,
    target: HirDefinitionId,
) -> bool {
    fn visit(
        graph: &SemanticDependencyGraph,
        current: HirDefinitionId,
        target: HirDefinitionId,
        visited: &mut HashSet<HirDefinitionId>,
    ) -> bool {
        let Some(node) = graph.nodes.iter().find(|node| node.definition == current) else {
            return false;
        };
        node.dependencies.iter().any(|dependency| {
            *dependency == target
                || visited.insert(*dependency) && visit(graph, *dependency, target, visited)
        })
    }
    visit(graph, current, target, &mut HashSet::new())
}

fn expression_dependencies(hir: &HirProgram, root: HirExpressionId) -> Vec<HirDefinitionId> {
    let mut dependencies = hir
        .expressions()
        .iter()
        .filter(|expression| expression_descends_from(hir, expression.id, root))
        .filter_map(|expression| expression.reference)
        .filter_map(|reference| hir.reference(reference))
        .filter_map(|reference| match reference.resolution {
            HirResolution::Definition(dependency) => Some(dependency),
            _ => None,
        })
        .collect::<Vec<_>>();
    dependencies.sort_unstable();
    dependencies.dedup();
    dependencies
}

fn definition_dependencies(hir: &HirProgram, definition: HirDefinitionId) -> Vec<HirDefinitionId> {
    let root = hir
        .definition(definition)
        .and_then(|definition| definition.value)
        .expect("type definition has a value expression");
    expression_dependencies(hir, root)
}

fn type_dependency_graph(
    hir: &HirProgram,
    type_definitions: &HashSet<HirDefinitionId>,
) -> SemanticDependencyGraph {
    let mut nodes = type_definitions
        .iter()
        .copied()
        .map(|definition| SemanticDependencyNode {
            definition,
            dependencies: definition_dependencies(hir, definition)
                .into_iter()
                .filter(|dependency| type_definitions.contains(dependency))
                .collect(),
        })
        .collect::<Vec<_>>();
    nodes.sort_by_key(|node| node.definition);
    SemanticDependencyGraph { nodes }
}

fn type_definition_bindings<'a>(
    hir: &HirProgram,
    bindings: &'a [Binding],
) -> BTreeMap<HirDefinitionId, &'a Binding> {
    bindings
        .iter()
        .filter(|binding| matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait))
        .filter_map(|binding| {
            hir.definitions()
                .iter()
                .find(|definition| {
                    definition.top_level
                        && definition.kind == HirDefinitionKind::Type
                        && definition.location == binding.value.name.location
                        && definition.value.is_some()
                })
                .map(|definition| (definition.id, binding))
        })
        .collect()
}

fn classify_partial_error(message: &str) -> FactState {
    if message.contains("not assignable") || message.contains("incompatible") {
        FactState::Conflicted(Conflict::IncompatibleContract)
    } else if message.contains("fuel exhausted")
        || message.contains("quota")
        || message.contains("stack limit")
    {
        FactState::Incomputable(IncomputableReason::QuotaExceeded)
    } else if message.contains("native symbol") || message.contains("has not been resolved") {
        FactState::Incomputable(IncomputableReason::RuntimeOnly)
    } else {
        FactState::Incomputable(IncomputableReason::UnsupportedOperation)
    }
}

pub(crate) fn analyze_program_registered(
    source_name: &str,
    sources: &SourceDatabase,
    program: &Program,
    evaluation_fuel: usize,
) -> Result<Analysis, FrontendError> {
    let mut account = QuotaAccount::new(Quota::with_fuel(evaluation_fuel));
    analyze_program_with_bindings(
        source_name,
        program,
        &mut account,
        &BTreeMap::new(),
        &HashSet::new(),
        sources,
        &BTreeMap::new(),
    )
}

pub(crate) fn analyze_program_with_bindings(
    source_name: &str,
    program: &Program,
    account: &mut QuotaAccount,
    external_values: &BTreeMap<String, crate::DataWorld>,
    dynamic_bindings: &HashSet<String>,
    sources: &SourceDatabase,
    external_provenance: &BTreeMap<String, Provenance>,
) -> Result<Analysis, FrontendError> {
    let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
    let mut tool_heap = Heap::main();
    let mut type_store = TypeStore::default();
    let external_roots = external_values
        .iter()
        .map(|(name, value)| {
            value
                .publish(&mut tool_heap)
                .map(|root| (name.clone(), root))
                .map_err(|error| frontend_error(source_name, error.to_string()))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    analyze_program_with_bindings_observed(
        source_name,
        crate::ModuleId::ANONYMOUS,
        program,
        account,
        &external_roots,
        dynamic_bindings,
        sources,
        external_provenance,
        &BTreeMap::new(),
        &debug_sink,
        &mut tool_heap,
        &mut type_store,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_program_with_bindings_observed(
    source_name: &str,
    module_id: crate::ModuleId,
    program: &Program,
    account: &mut QuotaAccount,
    external_roots: &BTreeMap<String, PersistentValue>,
    dynamic_bindings: &HashSet<String>,
    sources: &SourceDatabase,
    external_provenance: &BTreeMap<String, Provenance>,
    external_interfaces: &BTreeMap<String, ModuleInterface>,
    debug_sink: &Arc<dyn DebugSink>,
    tool_heap: &mut Heap,
    type_store: &mut TypeStore,
) -> Result<Analysis, FrontendError> {
    let prelude = BootstrapPrelude::new();
    let authored_names = program
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
    let hir = HirProgram::resolve(
        program,
        prelude
            .types
            .keys()
            .filter(|name| source_name.ends_with(".native.telora") || name.as_str() != "BlameError")
            .filter(|name| !external_roots.contains_key(*name))
            .chain(external_roots.keys())
            .cloned()
            .collect::<Vec<_>>(),
    );
    let prelude_value_names = prelude
        .types
        .keys()
        .filter(|name| !external_roots.contains_key(*name))
        .filter(|name| !authored_names.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let native_abi = source_name.ends_with(".native.telora");
    let prelude_names = prelude
        .types
        .keys()
        .filter(|name| native_abi || name.as_str() != "BlameError")
        .cloned()
        .collect::<Vec<_>>();
    let BootstrapPrelude {
        types: mut static_environment,
        schemes: mut binding_schemes,
    } = prelude;
    let cached_bootstrap_root = tool_heap.bootstrap_root();
    let mut evaluator = ToolEvaluator::new(Arc::clone(debug_sink), tool_heap);
    let mut tool_values = if let Some(root) = cached_bootstrap_root {
        prelude_value_names
            .iter()
            .map(|name| {
                let value = root
                    .export_get(evaluator.main, name)
                    .expect("bootstrap exports root is a Dict")
                    .unwrap_or_else(|| panic!("bootstrap exports root is missing {name:?}"));
                (name.clone(), value.runtime())
            })
            .collect()
    } else {
        evaluator.install_bootstrap()?
    };
    let mut declared_types = BTreeMap::new();
    let mut binding_types = BTreeMap::new();
    let mut declared_type_spans = HashMap::new();
    let mut expression_descriptors = HashMap::new();
    let mut next_type_constructor = crate::FIRST_DYNAMIC_MODULE_LOCAL;
    let mut declared_initializer_slots = HashMap::new();
    for binding in &program.value.body.value.bindings {
        if !matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait)
            || binding.value.declared_initializer.is_none()
        {
            continue;
        }
        declared_initializer_slots.insert(binding.value.name.location, next_type_constructor);
        next_type_constructor = next_type_constructor
            .checked_add(1)
            .expect("type constructor slot exceeds u32");
    }
    let trait_ids = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| binding.value.kind == BindingKind::Trait)
        .map(|binding| {
            (
                binding.value.name.value.clone(),
                crate::TraitId {
                    module: module_id,
                    local: declared_initializer_slots[&binding.value.name.location],
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut canonical_nominals = HashMap::<crate::Location, TypeId>::new();
    let mut canonical_nominal_names = HashMap::<String, TypeId>::new();
    for binding in &program.value.body.value.bindings {
        if !matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait)
            || binding.value.declared_initializer.is_none()
            || !binding.value.type_parameters.is_empty()
        {
            continue;
        }
        let constructor = crate::TypeConstructorId {
            module: module_id,
            local: declared_initializer_slots[&binding.value.name.location],
        };
        let id = match type_store.begin(constructor, []) {
            InternType::Existing(id) | InternType::Reserved(id) => id,
        };
        canonical_nominals.insert(binding.value.name.location, id);
        canonical_nominal_names.insert(binding.value.name.value.clone(), id);
    }
    let qualified_external_interfaces = external_interfaces
        .iter()
        .map(|(name, interface)| (name.clone(), interface.qualified(name)))
        .collect::<BTreeMap<_, _>>();

    let authored_names = program
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
    for (name, root) in external_roots {
        if authored_names.contains(name.as_str()) {
            continue;
        }
        let interface = qualified_external_interfaces.get(name);
        let scheme = interface
            .and_then(|interface| interface.exports.get(name))
            .cloned();
        tool_values.insert(name.clone(), root.runtime());
        let inferred = imported_static_descriptor(
            ValueRef::persistent(*root, evaluator.main),
            interface,
            name,
        );
        static_environment.insert(name.clone(), inferred.clone());
        binding_types.insert(name.clone(), inferred);
        if let Some(scheme) = scheme {
            binding_schemes.insert(name.clone(), scheme);
        }
    }
    let imported_named_types = qualified_external_interfaces
        .values()
        .flat_map(|interface| interface.concrete_types.clone())
        .collect::<BTreeMap<_, _>>();
    validate_export_references(program, prelude_names.iter(), external_roots, sources)?;

    let any_metadata = *tool_values.get("Any").expect("core prelude defines Any");
    for binding in &program.value.body.value.bindings {
        if matches!(
            binding.value.kind,
            BindingKind::Type | BindingKind::Trait | BindingKind::NativeType
        ) {
            tool_values.insert(binding.value.name.value.clone(), any_metadata);
            static_environment.insert(binding.value.name.value.clone(), TypeDescriptor::Type);
            binding_types.insert(binding.value.name.value.clone(), TypeDescriptor::Type);
        }
    }

    for name in dynamic_bindings {
        if !external_roots.contains_key(name) {
            return Err(frontend_error(
                source_name,
                format!("dynamic binding {name:?} has no value"),
            ));
        }
        static_environment.insert(name.clone(), TypeDescriptor::Any);
        binding_types.insert(name.clone(), TypeDescriptor::Any);
    }

    // Definition contracts are evaluated before the source-order binding pass.
    // Make resolved imports available at that same tool stage so selectively
    // imported TypeMetadata can be used directly as a contract.
    for binding in &program.value.body.value.bindings {
        if binding.value.kind != BindingKind::Import {
            continue;
        }
        let name = &binding.value.name.value;
        let value = external_roots.get(name).copied().ok_or_else(|| {
            frontend_error(source_name, format!("import {name} has not been resolved"))
        })?;
        tool_values.insert(name.clone(), value.runtime());
    }

    let type_bindings = type_definition_bindings(&hir, &program.value.body.value.bindings);
    let type_definitions = type_bindings.keys().copied().collect::<HashSet<_>>();
    let type_dependencies = type_dependency_graph(&hir, &type_definitions);
    for node in &type_dependencies.nodes {
        let binding = type_bindings[&node.definition];
        if binding.value.type_parameters.is_empty()
            && binding.value.declared_initializer.is_some()
            && dependency_reaches(&type_dependencies, node.definition, node.definition)
        {
            let name = binding.value.name.value.clone();
            let value = evaluator.descriptor(&TypeDescriptor::Named(name.clone()))?;
            tool_values.insert(name, value);
        }
    }
    let contract_type_definitions = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| {
            matches!(
                binding.value.kind,
                BindingKind::Decl | BindingKind::Native | BindingKind::Impl
            )
                || binding.value.kind == BindingKind::Def && binding.value.annotation.is_some()
        })
        .filter_map(|binding| binding.value.annotation.as_ref())
        .flat_map(|annotation| hir.expression_ids_at(annotation.location))
        .flat_map(|root| expression_dependencies(&hir, root))
        .filter(|definition| type_definitions.contains(definition))
        .collect::<Vec<_>>();
    let family_definitions = type_bindings
        .iter()
        .filter(|(_, binding)| !binding.value.type_parameters.is_empty())
        .map(|(definition, _)| *definition)
        .collect::<Vec<_>>();
    let family_dependents = type_definitions
        .iter()
        .copied()
        .filter(|definition| {
            family_definitions.iter().any(|family| {
                *definition == *family
                    || dependency_reaches(&type_dependencies, *definition, *family)
            })
        })
        .collect::<Vec<_>>();
    let mut scheduled_types = BTreeSet::new();
    let mut frontier = family_dependents;
    frontier.extend(contract_type_definitions);
    frontier.extend(
        type_definitions
            .iter()
            .copied()
            .filter(|definition| dependency_reaches(&type_dependencies, *definition, *definition)),
    );
    while let Some(definition) = frontier.pop() {
        if !scheduled_types.insert(definition) {
            continue;
        }
        if let Some(node) = type_dependencies
            .nodes
            .iter()
            .find(|node| node.definition == definition)
        {
            frontier.extend(node.dependencies.iter().copied());
        }
    }

    let mut pending_types = scheduled_types.clone();
    let mut evaluated_types = BTreeSet::new();
    let mut evaluated_concrete_type_names = HashSet::new();
    let mut type_family_values = BTreeMap::new();
    let mut type_family_templates = BTreeMap::new();
    while !pending_types.is_empty() {
        let mut progressed = false;
        for definition in pending_types.iter().copied().collect::<Vec<_>>() {
            let node = type_dependencies
                .nodes
                .iter()
                .find(|node| node.definition == definition)
                .expect("scheduled type has a dependency node");
            if node
                .dependencies
                .iter()
                .any(|dependency| !evaluated_types.contains(dependency))
            {
                continue;
            }
            let binding = type_bindings[&definition];
            if binding.value.type_parameters.is_empty() {
                let value = evaluate_tool_expression(
                    source_name,
                    &binding.value.value,
                    &tool_values,
                    account,
                    sources,
                    &mut evaluator,
                )?;
                let value = declare_metadata_value(
                    source_name,
                    module_id,
                    binding,
                    &declared_initializer_slots,
                    value,
                    &mut evaluator,
                )?;
                let (graph, root) =
                    evaluator
                        .decode_type_graph(value, "Type")
                        .map_err(|message| {
                            FrontendError::from_diagnostic(
                                sources,
                                Diagnostic::error(
                                    format!(
                                        "type {} produced invalid metadata: {message}",
                                        binding.value.name.value
                                    ),
                                    binding.value.value.location,
                                ),
                            )
                        })?;
                let descriptor = graph.descriptor(root).map_err(|message| {
                    frontend_error(
                        source_name,
                        format!(
                            "type {} produced invalid metadata: {message}",
                            binding.value.name.value
                        ),
                    )
                })?;
                graph
                    .canonicalize(root, type_store)
                    .map_err(|message| frontend_error(source_name, message))?;
                let name = binding.value.name.value.clone();
                declared_types.insert(name.clone(), descriptor.clone());
                declared_type_spans.insert(name.clone(), binding.location);
                tool_values.insert(name.clone(), value);
                let witness = TypeDescriptor::TypeOf(Box::new(descriptor));
                static_environment.insert(name.clone(), witness.clone());
                binding_types.insert(name.clone(), witness.clone());
                binding_schemes.insert(
                    name.clone(),
                    TypeScheme {
                        parameters: Vec::new(),
                        constraints: Vec::new(),
                        body: witness,
                    },
                );
                evaluated_concrete_type_names.insert(name);
                pending_types.remove(&definition);
                evaluated_types.insert(definition);
                progressed = true;
                continue;
            }

            let mut names = HashSet::new();
            let mut parameters = Vec::new();
            let mut bindings = tool_values.clone();
            for (parameter_index, parameter) in binding.value.type_parameters.iter().enumerate() {
                if !names.insert(parameter.value.as_str()) {
                    return Err(FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!("duplicate type parameter {:?}", parameter.value),
                            parameter.location,
                        ),
                    ));
                }
                let parameter_id =
                    TypeParameterId(u32::try_from(parameter_index).map_err(|_| {
                        frontend_error(source_name, "type family has too many parameters")
                    })?);
                parameters.push(TypeParameter {
                    id: parameter_id,
                    name: parameter.value.clone(),
                    location: parameter.location,
                });
                let value = evaluator.descriptor(&TypeDescriptor::Bound(parameter_id))?;
                bindings.insert(parameter.value.clone(), value);
            }
            let constraints = evaluate_type_constraints(
                source_name,
                &parameters,
                &binding.value.type_parameter_bounds,
                &bindings,
                &trait_ids,
                &qualified_external_interfaces,
                account,
                sources,
                &mut evaluator,
            )?;
            let value = evaluate_tool_expression(
                source_name,
                &binding.value.value,
                &bindings,
                account,
                sources,
                &mut evaluator,
            )?;
            let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
                FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!(
                            "type family {} produced invalid metadata: {message}",
                            binding.value.name.value
                        ),
                        binding.value.value.location,
                    ),
                )
            })?;
            let constructor =
                binding
                    .value
                    .declared_initializer
                    .as_ref()
                    .map(|_| NominalTypeConstructor {
                        id: crate::TypeConstructorId {
                            module: module_id,
                            local: declared_initializer_slots[&binding.value.name.location],
                        },
                        name: binding.value.name.value.clone(),
                    });
            let descriptor = if let Some(constructor) = &constructor {
                let arguments = parameters
                    .iter()
                    .map(|parameter| TypeDescriptor::Bound(parameter.id))
                    .collect::<Vec<_>>();
                let descriptor = TypeDescriptor::Declared(DeclaredTypeDescriptor {
                    id: crate::value::DeclaredTypeId::applied(
                        constructor.id.module,
                        constructor.id.local,
                        &arguments,
                    ),
                    name: constructor.name.clone(),
                    body: Arc::new(descriptor),
                });
                descriptor
            } else {
                descriptor
            };
            let mut bounds = Vec::new();
            collect_bound_parameters(&descriptor, &mut bounds);
            if let Some(foreign) = bounds
                .iter()
                .find(|bound| !parameters.iter().any(|parameter| parameter.id == **bound))
            {
                return Err(FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!(
                            "type family {} produced foreign bound parameter T{}",
                            binding.value.name.value, foreign.0
                        ),
                        binding.value.value.location,
                    ),
                ));
            }
            let (family_value, template_root, family_root) =
                evaluator.create_type_family(value, parameters.len(), constructor.as_ref())?;
            let family = TypeFamilyTemplate {
                parameters: parameters.clone(),
                template: template_root,
                root: family_root,
                rebuild_at_runtime: contains_named_type(&descriptor),
                constructor,
            };
            let scheme = TypeScheme {
                parameters,
                constraints,
                body: TypeDescriptor::Function {
                    parameters: family
                        .parameters
                        .iter()
                        .map(|parameter| {
                            TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(parameter.id)))
                        })
                        .collect(),
                    result: Box::new(TypeDescriptor::TypeOf(Box::new(descriptor))),
                },
            };
            let erased = erase_type_variables(&scheme.body);
            tool_values.insert(binding.value.name.value.clone(), family_value);
            static_environment.insert(binding.value.name.value.clone(), erased.clone());
            binding_types.insert(binding.value.name.value.clone(), erased);
            binding_schemes.insert(binding.value.name.value.clone(), scheme);
            type_family_templates.insert(binding.value.name.value.clone(), family.clone());
            type_family_values.insert(binding.value.name.value.clone(), family.clone());
            pending_types.remove(&definition);
            evaluated_types.insert(definition);
            progressed = true;
        }
        if !progressed {
            let root = pending_types
                .iter()
                .copied()
                .find(|definition| dependency_reaches(&type_dependencies, *definition, *definition))
                .expect("stalled type dependency schedule contains a cycle");
            let mut component = pending_types
                .iter()
                .copied()
                .filter(|definition| {
                    dependency_reaches(&type_dependencies, root, *definition)
                        && dependency_reaches(&type_dependencies, *definition, root)
                })
                .collect::<Vec<_>>();
            component.sort_unstable();
            let names = component
                .iter()
                .map(|definition| type_bindings[definition].value.name.value.as_str())
                .collect::<Vec<_>>();
            let binding = type_bindings[&root];
            let contains_family = component
                .iter()
                .any(|definition| !type_bindings[definition].value.type_parameters.is_empty());
            let contains_nominal = component.iter().any(|definition| {
                type_bindings[definition]
                    .value
                    .declared_initializer
                    .is_some()
            });
            let recursive_nominal_family =
                component.len() == 1 && contains_family && contains_nominal;
            if recursive_nominal_family {
                let definition = component[0];
                let binding = type_bindings[&definition];
                let built = build_recursive_type_family(
                    source_name,
                    module_id,
                    declared_initializer_slots[&binding.value.name.location],
                    binding,
                    &tool_values,
                    account,
                    sources,
                    &mut evaluator,
                )?;
                let erased = erase_type_variables(&built.scheme.body);
                tool_values.insert(binding.value.name.value.clone(), built.family_value);
                static_environment.insert(binding.value.name.value.clone(), erased.clone());
                binding_types.insert(binding.value.name.value.clone(), erased);
                binding_schemes.insert(binding.value.name.value.clone(), built.scheme);
                type_family_templates
                    .insert(binding.value.name.value.clone(), built.family.clone());
                type_family_values.insert(binding.value.name.value.clone(), built.family);
                pending_types.remove(&definition);
                evaluated_types.insert(definition);
                continue;
            }
            let concrete_nominal = !contains_family
                && component
                    .iter()
                    .all(|definition| {
                        type_bindings[definition]
                            .value
                            .declared_initializer
                            .is_some()
                    });
            if concrete_nominal {
                let mut type_refs = BTreeMap::new();
                for definition in &component {
                    let binding = type_bindings[definition];
                    let name = binding.value.name.value.clone();
                    let placeholder = tool_values[&name];
                    let type_ref = evaluator
                        .work
                        .reserve_type_ref(
                            module_id,
                            declared_initializer_slots[&binding.value.name.location],
                            name.as_str(),
                            placeholder,
                        )
                        .map_err(|error| {
                            frontend_error(
                                source_name,
                                format!("declared type reservation failed: {error}"),
                            )
                        })?;
                    tool_values.insert(name.clone(), type_ref);
                    type_refs.insert(*definition, type_ref);
                }
                let mut bodies = BTreeMap::new();
                for definition in &component {
                    let binding = type_bindings[&definition];
                    let value = evaluate_tool_expression(
                        source_name,
                        &binding.value.value,
                        &tool_values,
                        account,
                        sources,
                        &mut evaluator,
                    )?;
                    validate_declared_metadata(source_name, binding, value, &evaluator)?;
                    bodies.insert(*definition, value);
                }
                for definition in &component {
                    evaluator
                        .work
                        .seal_type_ref(type_refs[definition], bodies[definition])
                        .map_err(|error| frontend_error(source_name, error.to_string()))?;
                }
                for definition in component {
                    let binding = type_bindings[&definition];
                    let value = type_refs[&definition];
                    let (graph, root) =
                        evaluator
                            .decode_type_graph(value, "Type")
                            .map_err(|message| {
                                frontend_error(
                                    source_name,
                                    format!(
                                        "type {} produced invalid metadata: {message}",
                                        binding.value.name.value
                                    ),
                                )
                            })?;
                    let descriptor = graph.descriptor(root).map_err(|message| {
                        frontend_error(
                            source_name,
                            format!(
                                "type {} produced invalid metadata: {message}",
                                binding.value.name.value
                            ),
                        )
                    })?;
                    graph
                        .canonicalize(root, type_store)
                        .map_err(|message| frontend_error(source_name, message))?;
                    let name = binding.value.name.value.clone();
                    declared_types.insert(name.clone(), descriptor.clone());
                    declared_type_spans.insert(name.clone(), binding.location);
                    tool_values.insert(name.clone(), value);
                    let witness = TypeDescriptor::TypeOf(Box::new(descriptor));
                    static_environment.insert(name.clone(), witness.clone());
                    binding_types.insert(name.clone(), witness.clone());
                    binding_schemes.insert(
                        name.clone(),
                        TypeScheme {
                            parameters: Vec::new(),
                            constraints: Vec::new(),
                            body: witness,
                        },
                    );
                    evaluated_concrete_type_names.insert(name);
                    pending_types.remove(&definition);
                    evaluated_types.insert(definition);
                }
                continue;
            }
            let message = if !contains_nominal {
                format!(
                    "recursive type alias component containing {names:?} does not reach a struct or enum constructor"
                )
            } else if contains_family {
                format!("recursive type family component containing {names:?} is not supported")
            } else {
                format!(
                    "recursive type component required by a definition contract containing {names:?} is not supported"
                )
            };
            let mut diagnostic = Diagnostic::error(message, binding.value.name.location);
            for definition in component {
                if definition == root {
                    continue;
                }
                let participant = type_bindings[&definition];
                diagnostic =
                    diagnostic.with_secondary("cycle participant", participant.value.name.location);
            }
            return Err(FrontendError::from_diagnostic(sources, diagnostic));
        }
    }

    let mut definition_contracts = HashMap::new();
    let mut declaration_locations = HashMap::new();
    let mut definition_counts = HashMap::<String, usize>::new();
    for binding in &program.value.body.value.bindings {
        let name = &binding.value.name.value;
        if binding.value.kind == BindingKind::Def {
            *definition_counts.entry(name.clone()).or_default() += 1;
        }
        if !matches!(
            binding.value.kind,
            BindingKind::Decl | BindingKind::Native | BindingKind::Impl
        )
            && !(binding.value.kind == BindingKind::Def && binding.value.annotation.is_some())
        {
            continue;
        }
        if definition_contracts.contains_key(name) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(format!("duplicate declaration {name:?}"), binding.location),
            ));
        }
        let contract = binding
            .value
            .annotation
            .as_ref()
            .expect("declaration has a lowered contract");
        let mut contract_values = tool_values.clone();
        let mut parameter_names = HashSet::new();
        let mut scheme_parameters = Vec::new();
        for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
            if !parameter_names.insert(parameter.value.clone()) {
                return Err(FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!("duplicate type parameter {:?}", parameter.value),
                        parameter.location,
                    ),
                ));
            }
            let id = TypeParameterId(index as u32);
            scheme_parameters.push(TypeParameter {
                id,
                name: parameter.value.clone(),
                location: parameter.location,
            });
            let value = evaluator.descriptor(&TypeDescriptor::Bound(id))?;
            contract_values.insert(parameter.value.clone(), value);
        }
        let scheme_constraints = evaluate_type_constraints(
            source_name,
            &scheme_parameters,
            &binding.value.type_parameter_bounds,
            &contract_values,
            &trait_ids,
            &qualified_external_interfaces,
            account,
            sources,
            &mut evaluator,
        )?;
        let metadata = evaluate_tool_expression(
            source_name,
            contract,
            &contract_values,
            account,
            sources,
            &mut evaluator,
        )?;
        let descriptor = evaluator.decode_type(metadata, "Type").map_err(|message| {
            frontend_error(
                source_name,
                format!("declaration {name} has invalid contract metadata: {message}"),
            )
        })?;
        if binding.value.kind != BindingKind::Native
            || !scheme_parameters.is_empty()
            || contains_metatype(&descriptor)
        {
            binding_schemes.insert(
                name.clone(),
                TypeScheme {
                    parameters: scheme_parameters,
                    constraints: scheme_constraints,
                    body: descriptor.clone(),
                },
            );
        }
        let erased = erase_type_variables(&descriptor);
        static_environment.insert(name.clone(), erased.clone());
        binding_types.insert(name.clone(), erased);
        if matches!(
            binding.value.kind,
            BindingKind::Decl | BindingKind::Def | BindingKind::Impl
        ) {
            definition_contracts.insert(name.clone(), descriptor);
            if binding.value.kind != BindingKind::Impl {
                declaration_locations.insert(name.clone(), binding.location);
            }
        }
    }
    for (name, count) in &definition_counts {
        if *count > 1 {
            return Err(frontend_error(
                source_name,
                format!("definition {name:?} is initialized more than once"),
            ));
        }
    }
    let trait_implementations = collect_trait_implementations(
        module_id,
        program,
        &trait_ids,
        &qualified_external_interfaces,
        &definition_contracts,
        &binding_schemes,
        sources,
    )?;

    for binding in &program.value.body.value.bindings {
        if matches!(binding.value.value.value, ExprKind::Interpreter { .. }) {
            let contract = definition_contracts.get(&binding.value.name.value);
            validate_interpreter_contract(&binding.value.type_parameters, contract).map_err(
                |message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(message, binding.value.value.location),
                    )
                },
            )?;
        }
    }
    if let Some(reference) = hir.unresolved().next() {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!("unknown binding {:?}", reference.name),
                reference.location,
            ),
        ));
    }

    for binding in &program.value.body.value.bindings {
        check_interpolations(&binding.value.value, &static_environment, sources)?;
        let inferred_expression = infer_expr_recorded(
            &binding.value.value,
            &static_environment,
            &mut expression_descriptors,
        );
        if let Some(annotation) = &binding.value.annotation {
            check_interpolations(annotation, &static_environment, sources)?;
            infer_expr_recorded(annotation, &static_environment, &mut expression_descriptors);
        }
        match binding.value.kind {
            BindingKind::OpenImport | BindingKind::Export => continue,
            BindingKind::Decl => continue,
            BindingKind::Native | BindingKind::NativeType => {
                let value = external_roots
                    .get(&binding.value.name.value)
                    .copied()
                    .ok_or_else(|| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!(
                                    "native symbol {:?} has not been linked",
                                    binding.value.name.value
                                ),
                                binding.location,
                            ),
                        )
                    })?;
                tool_values.insert(binding.value.name.value.clone(), value.runtime());
                if binding.value.kind == BindingKind::NativeType {
                    let value = tool_values[&binding.value.name.value];
                    let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!(
                                    "native type {} is invalid: {message}",
                                    binding.value.name.value
                                ),
                                binding.location,
                            ),
                        )
                    })?;
                    let witness = TypeDescriptor::TypeOf(Box::new(descriptor.clone()));
                    declared_types.insert(binding.value.name.value.clone(), descriptor);
                    static_environment.insert(binding.value.name.value.clone(), witness.clone());
                    binding_types.insert(binding.value.name.value.clone(), witness.clone());
                    binding_schemes.insert(
                        binding.value.name.value.clone(),
                        TypeScheme {
                            parameters: Vec::new(),
                            constraints: Vec::new(),
                            body: witness,
                        },
                    );
                }
            }
            BindingKind::Type | BindingKind::Trait => {
                if !binding.value.type_parameters.is_empty()
                    || evaluated_concrete_type_names.contains(&binding.value.name.value)
                {
                    continue;
                }
                let value = evaluate_tool_expression(
                    source_name,
                    &binding.value.value,
                    &tool_values,
                    account,
                    sources,
                    &mut evaluator,
                )?;
                let value = declare_metadata_value(
                    source_name,
                    module_id,
                    binding,
                    &declared_initializer_slots,
                    value,
                    &mut evaluator,
                )?;
                let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!(
                                "type {} produced invalid metadata: {message}",
                                binding.value.name.value
                            ),
                            binding.value.value.location,
                        ),
                    )
                })?;
                declared_types.insert(binding.value.name.value.clone(), descriptor);
                declared_type_spans.insert(binding.value.name.value.clone(), binding.location);
                tool_values.insert(binding.value.name.value.clone(), value);
                let witness = TypeDescriptor::TypeOf(Box::new(
                    declared_types[&binding.value.name.value].clone(),
                ));
                static_environment.insert(binding.value.name.value.clone(), witness.clone());
                binding_types.insert(binding.value.name.value.clone(), witness.clone());
                binding_schemes.insert(
                    binding.value.name.value.clone(),
                    TypeScheme {
                        parameters: Vec::new(),
                        constraints: Vec::new(),
                        body: witness,
                    },
                );
            }
            BindingKind::Let | BindingKind::Impl => {
                let inferred = inferred_expression;
                let checked = if let Some(annotation) = &binding.value.annotation {
                    let metadata = evaluate_tool_expression(
                        source_name,
                        annotation,
                        &tool_values,
                        account,
                        sources,
                        &mut evaluator,
                    )?;
                    let expected = evaluator.decode_type(metadata, "Type").map_err(|message| {
                        frontend_error(
                            source_name,
                            format!(
                                "annotation on {} is invalid: {message}",
                                binding.value.name.value
                            ),
                        )
                    })?;
                    if !contains_any_descriptor(&inferred)
                        && !contains_named_type(&expected)
                        && !same_nominal_head_with_erased_arguments(&inferred, &expected)
                        && !assignable(&inferred, &expected)
                        && !is_declared_literal_construction(
                            &binding.value.value,
                            &inferred,
                            &expected,
                        )
                    {
                        let message = format!(
                            "binding {} has type {}, which is not assignable to {}",
                            binding.value.name.value,
                            inferred.display_name(),
                            expected.display_name()
                        );
                        {
                            let path =
                                incompatibility_path(&inferred, &expected).unwrap_or_default();
                            let data_span = match &binding.value.value.value {
                                ExprKind::Variable(name) => external_provenance
                                    .get(&name.value)
                                    .and_then(|provenance| {
                                        provenance
                                            .values
                                            .get(&path)
                                            .or_else(|| provenance.values.get(&Vec::new()))
                                    })
                                    .cloned(),
                                _ => expression_location_at_path(&binding.value.value, &path)
                                    .or(Some(binding.value.value.location)),
                            }
                            .unwrap_or(binding.location);
                            let rule_span = match &annotation.value {
                                ExprKind::Variable(name) => {
                                    declared_type_spans.get(&name.value).copied()
                                }
                                _ => Some(annotation.location),
                            }
                            .unwrap_or(binding.location);
                            let diagnostic = Diagnostic::error(message, data_span)
                                .with_secondary("type requirement declared here", rule_span);
                            return Err(FrontendError::from_diagnostic(sources, diagnostic));
                        }
                    }
                    expected
                } else {
                    inferred
                };
                static_environment.insert(binding.value.name.value.clone(), checked.clone());
                binding_types.insert(binding.value.name.value.clone(), checked);

                if let Ok(value) = evaluate_typed_tool_expression_silent(
                    source_name,
                    &binding.value.value,
                    &tool_values,
                    &expression_descriptors,
                    account,
                    sources,
                    &mut evaluator,
                ) {
                    tool_values.insert(binding.value.name.value.clone(), value);
                }
            }
            BindingKind::Def => {
                let name = &binding.value.name.value;
                let inferred = inferred_expression;
                let checked = definition_contracts
                    .get(name)
                    .map(erase_type_variables)
                    .unwrap_or(inferred);
                static_environment.insert(name.clone(), checked.clone());
                binding_types.insert(name.clone(), checked);
                if let Ok(value) = evaluate_typed_tool_expression_silent(
                    source_name,
                    &binding.value.value,
                    &tool_values,
                    &expression_descriptors,
                    account,
                    sources,
                    &mut evaluator,
                ) {
                    tool_values.insert(name.clone(), value);
                }
            }
            BindingKind::Import => {
                let value = external_roots
                    .get(&binding.value.name.value)
                    .copied()
                    .ok_or_else(|| {
                        frontend_error(
                            source_name,
                            format!("import {} has not been resolved", binding.value.name.value),
                        )
                    })?;
                let interface = qualified_external_interfaces.get(&binding.value.name.value);
                let scheme = interface
                    .and_then(|interface| interface.exports.get(&binding.value.name.value))
                    .cloned();
                let inferred = imported_static_descriptor(
                    ValueRef::persistent(value, evaluator.main),
                    interface,
                    &binding.value.name.value,
                );
                static_environment.insert(binding.value.name.value.clone(), inferred.clone());
                binding_types.insert(binding.value.name.value.clone(), inferred);
                if let Some(scheme) = scheme {
                    binding_schemes.insert(binding.value.name.value.clone(), scheme);
                    tool_values.insert(binding.value.name.value.clone(), value.runtime());
                } else {
                    tool_values.insert(binding.value.name.value.clone(), value.runtime());
                }
            }
        }
    }

    evaluate_declared_properties(
        source_name,
        program,
        &tool_values,
        &static_environment,
        account,
        sources,
        &mut evaluator,
    )?;

    for (name, location) in &declaration_locations {
        if definition_counts.get(name).copied().unwrap_or(0) == 0 {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!("definition {name:?} was declared but never initialized"),
                    *location,
                ),
            ));
        }
    }

    check_interpolations(
        &program.value.body.value.result,
        &static_environment,
        sources,
    )?;
    infer_expr_recorded(
        &program.value.body.value.result,
        &static_environment,
        &mut expression_descriptors,
    );
    let mut local_annotations = HashMap::new();
    for binding in &program.value.body.value.bindings {
        let mut annotation_values = tool_values.clone();
        for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
            let value =
                evaluator.descriptor(&TypeDescriptor::Bound(TypeParameterId(index as u32)))?;
            annotation_values.insert(parameter.value.clone(), value);
        }
        collect_nested_annotation_types(
            source_name,
            &binding.value.value,
            &annotation_values,
            account,
            sources,
            &mut evaluator,
            &mut local_annotations,
        )?;
    }
    collect_nested_annotation_types(
        source_name,
        &program.value.body.value.result,
        &tool_values,
        account,
        sources,
        &mut evaluator,
        &mut local_annotations,
    )?;
    let mut named_types = imported_named_types;
    named_types.extend(declared_types.clone());
    let dyn_namespaces = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter_map(|binding| {
            (binding.value.kind == BindingKind::Import
                && binding.value.imported_name.is_none()
                && matches!(&binding.value.value.value, ExprKind::String(path) if path == "std/dyn"))
            .then(|| binding.value.name.value.clone())
        })
        .collect::<HashSet<_>>();
    let mut inference = GenericInference::new(
        &binding_schemes,
        &hir,
        &qualified_external_interfaces,
        &named_types,
        &local_annotations,
        &trait_implementations,
        &trait_ids,
        &dyn_namespaces,
        !external_roots.contains_key("Tuple"),
        account.query_context(),
    );
    for binding in &program.value.body.value.bindings {
        if binding.value.annotation.is_some()
            && binding_types
                .get(&binding.value.name.value)
                .is_some_and(contains_any_descriptor)
        {
            inference
                .authored_any_definitions
                .insert(binding.value.name.location);
        }
    }
    let mut checked_environment = static_environment.clone();
    let type_metadata_expected = TypeDescriptor::Type;
    let mut delayed_bindings = Vec::new();
    let mut recursive_skeletons = HashMap::new();
    let component_plan = definition_component_plan(&program.value.body, &hir);
    if let Some(location) = component_plan.indirect_recursive.iter().next() {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                "indirect recursive definition requires an explicit contract",
                *location,
            ),
        ));
    }
    for binding in &program.value.body.value.bindings {
        if binding.value.kind != BindingKind::Def
            || binding.value.annotation.is_some()
            || definition_contracts.contains_key(&binding.value.name.value)
            || !component_plan
                .recursive
                .contains(&binding.value.name.location)
        {
            continue;
        }
        let first_owned_variable = inference.next_variable;
        if let Some(skeleton) = inference.recursive_closure_skeleton(&binding.value.value) {
            checked_environment.insert(binding.value.name.value.clone(), skeleton.clone());
            inference.set_local_scheme(binding.value.name.value.clone(), None);
            recursive_skeletons.insert(
                binding.value.name.value.clone(),
                (skeleton.clone(), first_owned_variable),
            );
            delayed_bindings.push((
                binding.value.name.value.clone(),
                binding.value.value.location,
                skeleton,
                first_owned_variable,
            ));
        }
    }
    let recursive_variables = recursive_skeletons
        .values()
        .filter_map(|(skeleton, _)| GenericInference::recursive_result_variable(skeleton))
        .collect::<HashSet<_>>();
    for binding in &program.value.body.value.bindings {
        let Some((skeleton, _)) = recursive_skeletons.get(&binding.value.name.value) else {
            continue;
        };
        inference.delayed_initializer_depth += 1;
        inference.recursive_body_inference_depth += 1;
        let recursive_expected = GenericInference::recursive_expected(skeleton);
        let inferred = inference.infer(
            &binding.value.value,
            &checked_environment,
            Some(&recursive_expected),
        );
        inference.recursive_body_inference_depth -= 1;
        inference.delayed_initializer_depth -= 1;
        let inferred = inferred.map_err(|message| {
            let diagnostic = inference.take_failure_diagnostic(
                binding.value.value.location,
                message,
                binding
                    .value
                    .annotation
                    .as_ref()
                    .map(|annotation| annotation.location),
            );
            FrontendError::from_diagnostic(sources, diagnostic)
        })?;
        if let (
            Some(variable),
            TypeDescriptor::Function {
                result: inferred_result,
                ..
            },
        ) = (
            GenericInference::recursive_result_variable(skeleton),
            inferred,
        ) {
            inference
                .recursive_equations
                .insert(variable, *inferred_result);
        }
        binding_types.insert(binding.value.name.value.clone(), skeleton.clone());
    }
    inference
        .solve_recursive_equations(&recursive_variables)
        .map_err(|message| frontend_error(source_name, message))?;
    for location in &component_plan.acyclic {
        let binding = program
            .value
            .body
            .value
            .bindings
            .iter()
            .find(|binding| binding.value.name.location == *location)
            .expect("component binding exists");
        let first_owned_variable = inference.next_variable;
        inference.delayed_initializer_depth += 1;
        let inferred = inference.infer(&binding.value.value, &checked_environment, None);
        inference.delayed_initializer_depth -= 1;
        let inferred = inferred.map_err(|message| {
            let diagnostic = inference.take_failure_diagnostic(
                binding.value.value.location,
                message,
                binding
                    .value
                    .annotation
                    .as_ref()
                    .map(|annotation| annotation.location),
            );
            FrontendError::from_diagnostic(sources, diagnostic)
        })?;
        let scheme = inference
            .generalize_local_closure(&inferred, first_owned_variable, binding.value.name.location)
            .map_err(|message| {
                FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(message, binding.value.value.location),
                )
            })?;
        let descriptor = scheme.as_ref().map_or_else(
            || inference.resolve(&inferred),
            |scheme| scheme.body.clone(),
        );
        checked_environment.insert(binding.value.name.value.clone(), descriptor.clone());
        binding_types.insert(binding.value.name.value.clone(), descriptor);
        inference.set_local_scheme(binding.value.name.value.clone(), scheme.clone());
        if let Some(scheme) = scheme {
            inference
                .inferred_schemes
                .insert(binding.value.name.location, scheme.clone());
            inference
                .top_level_inferred_schemes
                .insert(binding.value.name.value.clone(), scheme);
        } else {
            delayed_bindings.push((
                binding.value.name.value.clone(),
                binding.value.value.location,
                inferred,
                first_owned_variable,
            ));
        }
    }
    for binding in &program.value.body.value.bindings {
        if matches!(
            binding.value.kind,
            BindingKind::Decl
                | BindingKind::Native
                | BindingKind::Import
                | BindingKind::OpenImport
                | BindingKind::Export
        ) {
            if binding.value.kind == BindingKind::Import {
                let scheme = external_interfaces
                    .get(&binding.value.name.value)
                    .and_then(|interface| interface.exports.get(&binding.value.name.value))
                    .cloned();
                inference.set_local_scheme(binding.value.name.value.clone(), scheme);
            }
            continue;
        }
        if recursive_skeletons.contains_key(&binding.value.name.value) {
            continue;
        }
        if component_plan
            .acyclic
            .contains(&binding.value.name.location)
        {
            continue;
        }
        let expected = if matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait) {
            Some(&type_metadata_expected)
        } else {
            definition_contracts
                .get(&binding.value.name.value)
                .or_else(|| {
                    binding
                        .value
                        .annotation
                        .as_ref()
                        .and_then(|_| binding_types.get(&binding.value.name.value))
                })
                .or_else(|| {
                    recursive_skeletons
                        .get(&binding.value.name.value)
                        .map(|(skeleton, _)| skeleton)
                })
        };
        let is_recursive = recursive_skeletons.contains_key(&binding.value.name.value);
        if binding.value.kind == BindingKind::Def
            && binding.value.annotation.is_none()
            && !is_recursive
            && !definition_contracts.contains_key(&binding.value.name.value)
            && expression_references_names(
                &binding.value.value,
                &HashSet::from([binding.value.name.value.clone()]),
                &HashSet::new(),
            )
        {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!(
                        "recursive definition {:?} requires a closure value or explicit contract",
                        binding.value.name.value
                    ),
                    binding.value.value.location,
                ),
            ));
        }
        let is_delayed = (expected.is_none() || is_recursive)
            && matches!(
                binding.value.kind,
                BindingKind::Let | BindingKind::Def | BindingKind::Impl
            )
            && !definition_contracts.contains_key(&binding.value.name.value);
        let first_owned_variable = recursive_skeletons
            .get(&binding.value.name.value)
            .map_or(inference.next_variable, |(_, first)| *first);
        if is_delayed {
            inference.delayed_initializer_depth += 1;
        }
        let mut initializer_environment = None;
        if is_delayed && binding.value.kind == BindingKind::Def && !is_recursive {
            let mut environment = checked_environment.clone();
            environment.remove(&binding.value.name.value);
            initializer_environment = Some(environment);
        } else if matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait)
            && !binding.value.type_parameters.is_empty()
        {
            let mut environment = checked_environment.clone();
            for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
                environment.insert(
                    parameter.value.clone(),
                    TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(TypeParameterId(
                        index as u32,
                    )))),
                );
            }
            initializer_environment = Some(environment);
        }
        let environment = initializer_environment
            .as_ref()
            .unwrap_or(&checked_environment);
        let inferred = if matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait) {
            inference.infer(&binding.value.value, environment, expected)
        } else {
            inference.infer_authored_boundary(&binding.value.value, environment, expected)
        };
        if is_delayed {
            inference.delayed_initializer_depth -= 1;
        }
        let inferred = inferred.map_err(|message| {
            let expected_location = binding
                .value
                .annotation
                .as_ref()
                .map(|annotation| annotation.location)
                .or_else(|| {
                    declaration_locations
                        .get(&binding.value.name.value)
                        .copied()
                });
            let diagnostic = inference.take_failure_diagnostic(
                binding.value.value.location,
                message,
                expected_location,
            );
            FrontendError::from_diagnostic(sources, diagnostic)
        })?;
        if matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait) {
            continue;
        }
        if matches!(
            binding.value.kind,
            BindingKind::Let | BindingKind::Def | BindingKind::Impl
        ) {
            let inferred_scheme = if binding.value.kind == BindingKind::Let
                && binding.value.annotation.is_none()
                && binding.value.type_parameters.is_empty()
                && matches!(binding.value.value.value, ExprKind::Closure { .. })
            {
                inference
                    .generalize_local_closure(
                        &inferred,
                        first_owned_variable,
                        binding.value.name.location,
                    )
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(message, binding.value.value.location),
                        )
                    })?
            } else {
                None
            };
            let checked = inferred_scheme.as_ref().map_or_else(
                || expected.cloned().unwrap_or(inferred),
                |scheme| scheme.body.clone(),
            );
            checked_environment.insert(binding.value.name.value.clone(), checked.clone());
            binding_types.insert(binding.value.name.value.clone(), checked.clone());
            if inferred_scheme.is_some()
                || binding.value.kind == BindingKind::Let
                || binding.value.annotation.is_none()
                    && !definition_contracts.contains_key(&binding.value.name.value)
            {
                inference
                    .set_local_scheme(binding.value.name.value.clone(), inferred_scheme.clone());
            }
            if let Some(scheme) = &inferred_scheme {
                inference
                    .inferred_schemes
                    .insert(binding.value.name.location, scheme.clone());
            }
            if let Some(scheme) = inferred_scheme {
                inference
                    .top_level_inferred_schemes
                    .insert(binding.value.name.value.clone(), scheme);
            } else if is_delayed && !is_recursive {
                delayed_bindings.push((
                    binding.value.name.value.clone(),
                    binding.value.value.location,
                    checked,
                    first_owned_variable,
                ));
            }
        }
    }
    let result_type = inference
        .infer(&program.value.body.value.result, &checked_environment, None)
        .map_err(|message| {
            let diagnostic = inference.take_failure_diagnostic(
                program.value.body.value.result.location,
                message,
                None,
            );
            FrontendError::from_diagnostic(sources, diagnostic)
        })?;
    let module_requirement = inference
        .propagation_boundaries
        .pop()
        .expect("module propagation boundary exists");
    let result_type = inference
        .finish_propagation_boundary(result_type, None, module_requirement)
        .map_err(|message| {
            FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(message, program.value.body.value.result.location),
            )
        })?;
    if let Some((location, message)) = inference.pattern_diagnostics.first_key_value() {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(message.clone(), *location),
        ));
    }
    if let Some((location, message)) = inference.unresolved_placeholder_since(0) {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(message, location),
        ));
    }
    inference
        .finish_type_constraints()
        .map_err(|(location, message)| {
            FrontendError::from_diagnostic(sources, Diagnostic::error(message, location))
        })?;
    for (name, location, descriptor, first_owned_variable) in delayed_bindings {
        if let Some(query) = &inference.query {
            query.check().map_err(|error| {
                FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(error.to_string(), location),
                )
            })?;
        }
        let resolved = inference.resolve(&descriptor);
        if contains_inference_variable_at_or_after(&resolved, first_owned_variable) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!(
                        "cannot infer monomorphic binding {name:?}: unresolved {}",
                        resolved.display_name()
                    ),
                    location,
                ),
            ));
        }
    }
    expression_descriptors.extend(
        inference
            .records
            .iter()
            .map(|(location, ty)| (*location, inference.resolve(ty))),
    );
    inference.top_level_inferred_schemes = inference
        .top_level_inferred_schemes
        .iter()
        .map(|(name, scheme)| {
            let mut scheme = scheme.clone();
            scheme.body = inference.resolve(&scheme.body);
            (name.clone(), scheme)
        })
        .collect();
    inference.inferred_schemes = inference
        .inferred_schemes
        .iter()
        .map(|(location, scheme)| {
            let mut scheme = scheme.clone();
            scheme.body = inference.resolve(&scheme.body);
            (*location, scheme)
        })
        .collect();
    binding_schemes.extend(inference.top_level_inferred_schemes.clone());
    let explicitly_exported_locals = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| binding.value.kind == BindingKind::Export)
        .filter_map(|binding| binding.value.imported_name.as_deref())
        .map(|name| name.value.as_str())
        .collect::<HashSet<_>>();
    for (name, descriptor) in &binding_types {
        if explicitly_exported_locals.contains(name.as_str()) {
            binding_schemes
                .entry(name.clone())
                .or_insert_with(|| TypeScheme {
                    parameters: Vec::new(),
                    constraints: Vec::new(),
                    body: inference.resolve(descriptor),
                });
        }
    }
    let resolved_result = inference.resolve(&result_type);
    for (name, descriptor) in &binding_types {
        let resolved = inference.resolve(descriptor);
        if contains_type_variable(&resolved) {
            return Err(frontend_error(
                source_name,
                format!(
                    "cannot publish unresolved binding {name:?}: {}",
                    resolved.display_name()
                ),
            ));
        }
    }
    for (name, scheme) in &inference.top_level_inferred_schemes {
        validate_publishable_scheme(scheme).map_err(|message| {
            frontend_error(
                source_name,
                format!("cannot publish scheme for {name:?}: {message}"),
            )
        })?;
    }
    let interface_binding_types = binding_types
        .iter()
        .map(|(name, descriptor)| (name.clone(), inference.resolve(descriptor)))
        .collect::<BTreeMap<_, _>>();
    let mut types = TypeGraph::default();
    let declared_type_names = declared_types.keys().cloned().collect::<Vec<_>>();
    let installed_named_types = types.install_named_descriptors(&named_types);
    let declared_types = declared_type_names
        .into_iter()
        .map(|name| (name.clone(), installed_named_types[&name]))
        .collect::<BTreeMap<_, _>>();
    let binding_types: BTreeMap<String, AnalysisTypeId> = binding_types
        .into_iter()
        .map(|(name, descriptor)| {
            let descriptor = inference.resolve(&descriptor);
            (name, types.intern_erased_descriptor(&descriptor))
        })
        .collect();
    let result_type = types.intern_erased_descriptor(&resolved_result);
    let expression_types: BTreeMap<HirExpressionId, AnalysisTypeId> = hir
        .expressions()
        .iter()
        .filter_map(|expression| {
            expression_descriptors
                .get(&expression.location)
                .map(|descriptor| (expression.id, types.intern_erased_descriptor(descriptor)))
        })
        .collect();
    let any_type = types.intern_descriptor(&TypeDescriptor::Any);
    let pattern_definition_types = hir
        .definitions()
        .iter()
        .filter(|definition| definition.kind == HirDefinitionKind::Pattern)
        .filter_map(|definition| {
            inference
                .pattern_binding_types
                .get(&definition.location)
                .map(|descriptor| {
                    (
                        definition.id,
                        types.intern_erased_descriptor(&inference.resolve(descriptor)),
                    )
                })
        })
        .collect::<HashMap<_, _>>();
    let definition_types = hir
        .definitions()
        .iter()
        .map(|definition| {
            let ty = if definition.top_level {
                binding_types.get(&definition.name).copied()
            } else {
                definition
                    .value
                    .and_then(|value| expression_types.get(&value).copied())
            }
            .or_else(|| pattern_definition_types.get(&definition.id).copied())
            .unwrap_or(any_type);
            (definition.id, ty)
        })
        .collect();
    let definition_schemes = hir
        .definitions()
        .iter()
        .filter_map(|definition| {
            inference
                .inferred_schemes
                .get(&definition.location)
                .cloned()
                .or_else(|| {
                    definition
                        .top_level
                        .then(|| binding_schemes.get(&definition.name))
                        .flatten()
                        .filter(|scheme| !scheme.parameters.is_empty())
                        .cloned()
                })
                .map(|scheme| (definition.id, scheme))
        })
        .collect::<BTreeMap<_, _>>();
    for (definition, scheme) in &definition_schemes {
        if hir
            .definition(*definition)
            .is_some_and(|definition| definition.top_level)
        {
            validate_publishable_scheme(scheme)
                .map_err(|message| frontend_error(source_name, message))?;
        }
    }
    let module_interface = ModuleInterface {
        exports: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields
                .iter()
                .filter_map(|field| {
                    let ExprKind::Variable(binding) = &field.value.value.value else {
                        return None;
                    };
                    binding_schemes
                        .get(&binding.value)
                        .cloned()
                        .or_else(|| {
                            interface_binding_types
                                .get(&binding.value)
                                .map(|body| TypeScheme {
                                    parameters: Vec::new(),
                                    constraints: Vec::new(),
                                    body: body.clone(),
                                })
                        })
                        .or_else(|| {
                            checked_environment
                                .get(&binding.value)
                                .map(|body| TypeScheme {
                                    parameters: Vec::new(),
                                    constraints: Vec::new(),
                                    body: inference.resolve(body),
                                })
                        })
                        .and_then(|scheme| {
                            field
                                .value
                                .name
                                .as_ref()
                                .map(|name| (name.value.clone(), scheme))
                        })
                })
                .collect(),
            _ => BTreeMap::new(),
        },
        concrete_types: named_types
            .iter()
            .filter(|(_, descriptor)| contains_named_type(descriptor))
            .map(|(name, descriptor)| (name.clone(), descriptor.clone()))
            .collect(),
        traits: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields
                .iter()
                .filter_map(|field| {
                    let ExprKind::Variable(binding) = &field.value.value.value else {
                        return None;
                    };
                    let id = trait_ids.get(&binding.value).copied().or_else(|| {
                        qualified_external_interfaces
                            .get(&binding.value)
                            .and_then(|interface| interface.traits.get(&binding.value))
                            .copied()
                    })?;
                    field
                        .value
                        .name
                        .as_ref()
                        .map(|name| (name.value.clone(), id))
                })
                .collect(),
            _ => BTreeMap::new(),
        },
        type_family_templates: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields
                .iter()
                .filter_map(|field| {
                    let ExprKind::Variable(binding) = &field.value.value.value else {
                        return None;
                    };
                    field.value.name.as_ref().and_then(|name| {
                        type_family_templates
                            .get(&binding.value)
                            .cloned()
                            .or_else(|| {
                                qualified_external_interfaces
                                    .get(&binding.value)
                                    .and_then(|interface| {
                                        interface.type_family_templates.get(&binding.value)
                                    })
                                    .cloned()
                            })
                            .map(|family| (name.value.clone(), family))
                    })
                })
                .collect(),
            _ => BTreeMap::new(),
        },
    };
    for scheme in module_interface.exports.values() {
        validate_publishable_scheme(scheme)
            .map_err(|message| frontend_error(source_name, message))?;
    }
    let propagation_families = std::mem::take(&mut inference.propagation_families);
    let not_families = std::mem::take(&mut inference.not_families);
    let trait_evidence = std::mem::take(&mut inference.resolved_trait_evidence);
    let bootstrap_root = if let Some(root) = cached_bootstrap_root {
        root
    } else {
        let root = evaluator.persist_table(prelude_value_names.iter().map(|name| {
            (
                name.clone(),
                *tool_values
                    .get(name)
                    .unwrap_or_else(|| panic!("core prelude runtime value {name:?} is available")),
            )
        }))?;
        evaluator.main.set_bootstrap_root(root);
        root
    };
    let mut runtime_roots = prelude_value_names
        .iter()
        .map(|name| {
            let root = bootstrap_root
                .export_get(evaluator.main, name)
                .expect("bootstrap exports root is a Dict")
                .expect("bootstrap exports root is complete");
            (name.clone(), root)
        })
        .collect::<BTreeMap<_, _>>();
    let concrete_type_names = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| {
            matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait)
                && binding.value.type_parameters.is_empty()
        })
        .map(|binding| binding.value.name.value.clone())
        .collect::<Vec<_>>();
    if !concrete_type_names.is_empty() {
        let roots = evaluator.persist_table(concrete_type_names.iter().map(|name| {
            (
                name.clone(),
                *tool_values
                    .get(name)
                    .expect("analyzed concrete Type has a runtime root"),
            )
        }))?;
        for name in concrete_type_names {
            let root = roots
                .export_get(evaluator.main, &name)
                .expect("concrete Type root table is a Module")
                .expect("concrete Type root is present");
            runtime_roots.insert(crate::compiler::type_link_key(&name), root);
        }
    }
    let mut pending_owner_roots = Vec::new();
    let mut declared_value_owners = HashMap::new();
    for (location, descriptor) in expression_descriptors.iter().filter(|(_, descriptor)| {
        matches!(descriptor, TypeDescriptor::Declared(_)) && !type_identity_is_symbolic(descriptor)
    }) {
        let key = crate::compiler::declared_owner_link_key(*location);
        let value = evaluator.descriptor(descriptor)?;
        pending_owner_roots.push((key.clone(), value));
        declared_value_owners.insert(*location, key);
    }
    if !pending_owner_roots.is_empty() {
        let names = pending_owner_roots
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let root = evaluator.persist_table(pending_owner_roots)?;
        for name in names {
            let value = root
                .export_get(evaluator.main, &name)
                .expect("analysis runtime exports root is a Dict")
                .expect("analysis runtime export is present");
            runtime_roots.insert(name, value);
        }
    }
    let external_bindings = external_roots
        .keys()
        .chain(runtime_roots.keys())
        .cloned()
        .collect();
    Ok(Analysis {
        types,
        declared_types,
        binding_types,
        trait_ids,
        trait_implementations,
        result_type,
        hir,
        definition_types,
        definition_schemes,
        expression_types,
        module_interface,
        explicit_exports: program
            .value
            .body
            .value
            .bindings
            .iter()
            .any(|binding| binding.value.kind == BindingKind::Export),
        propagation_families,
        not_families,
        trait_evidence,
        runtime_roots,
        external_bindings,
        dynamic_bindings: dynamic_bindings.clone(),
        type_family_values,
        declared_value_owners,
    })
}
