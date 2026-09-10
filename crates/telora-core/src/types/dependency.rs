fn tool_value_dependencies(hir: &HirProgram) -> HashSet<String> {
    let mut needed = HashSet::new();
    let mut frontier = Vec::new();
    for expression in hir.expressions() {
        let Some(reference) = expression.reference.and_then(|id| hir.reference(id)) else { continue; };
        let HirResolution::Definition(definition) = reference.resolution else { continue; };
        let mut parent = Some(expression.id);
        while let Some(id) = parent {
            let expression = hir.expression(id).expect("HIR expression exists");
            if hir.is_tool_root(expression.location) {
                frontier.push(definition);
                break;
            }
            parent = expression.parent;
        }
    }
    while let Some(id) = frontier.pop() {
        let Some(definition) = hir.definition(id) else { continue; };
        if !definition.top_level || !needed.insert(id) || definition.value.is_none() {
            continue;
        }
        frontier.extend(definition_dependencies(hir, id));
    }
    needed.into_iter().filter_map(|id| hir.definition(id).map(|definition| definition.name.clone())).collect()
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ModuleAnalysisContext {
    #[default]
    Ordinary,
    Builtin { defines_display_trait: bool },
}

impl ModuleAnalysisContext {
    const fn defines_display_trait(self) -> bool {
        matches!(
            self,
            Self::Builtin {
                defines_display_trait: true
            }
        )
    }
}

#[cfg(test)]
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
    let external_interfaces = external_values.iter().map(|(name, value)| {
        value.static_interface(name).map(|interface| (name.clone(), interface))
            .ok_or_else(|| frontend_error(source_name, format!("Host binding {name:?} requires an explicit type interface")))
    }).collect::<Result<BTreeMap<_, _>, _>>()?;
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
        ModuleAnalysisContext::Ordinary,
        program,
        resolve_module_hir_with_interfaces(program, external_roots.keys().cloned(), &external_interfaces),
        account,
        &external_roots,
        dynamic_bindings,
        sources,
        external_provenance,
        &external_interfaces,
        &debug_sink,
        &mut tool_heap,
        &mut type_store,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_program_with_bindings_observed(
    source_name: &str,
    module_id: crate::ModuleId,
    module_context: ModuleAnalysisContext,
    program: &Program,
    hir: HirProgram,
    account: &mut QuotaAccount,
    external_roots: &BTreeMap<String, PersistentValue>,
    dynamic_bindings: &HashSet<String>,
    sources: &SourceDatabase,
    external_provenance: &BTreeMap<String, Provenance>,
    external_interfaces: &BTreeMap<String, ModuleInterface>,
    debug_sink: &Arc<dyn DebugSink>,
    tool_heap: &mut Heap,
    type_store: &mut TypeStore,
    dependency_facts: &[ModuleTypeFacts<'_>],
) -> Result<Analysis, FrontendError> {
    account.register_sources(sources);
    let external_names = external_roots.keys().cloned().collect();
    let solved = solve_module_plan(source_name, module_id, module_context, program,
        hir, &external_names, sources, external_provenance, external_interfaces,
        dependency_facts, account.query_context(), type_store)?;
    execute_module_plan(source_name, module_id, program, solved, account,
        external_roots, dynamic_bindings, sources, debug_sink, tool_heap)
}

#[allow(clippy::too_many_arguments)]
fn solve_module_plan<'a>(
    source_name: &str,
    module_id: crate::ModuleId,
    module_context: ModuleAnalysisContext,
    program: &'a Program,
    hir: HirProgram,
    external_names: &BTreeSet<String>,
    sources: &SourceDatabase,
    external_provenance: &BTreeMap<String, Provenance>,
    external_interfaces: &BTreeMap<String, ModuleInterface>,
    dependency_facts: &[ModuleTypeFacts<'_>],
    query: Option<crate::query::QueryContext>,
    type_store: &mut TypeStore,
) -> Result<SolvedModulePlan<'a>, FrontendError> {
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
    let prelude_value_names = prelude
        .types
        .keys()
        .filter(|name| !external_names.contains(*name))
        .filter(|name| !authored_names.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut boundary = TypeBoundary::new(&hir, external_interfaces);
    boundary.external_data.extend(external_names.iter().cloned());
    boundary.bindings(&program.value.body.value.bindings);
    if program.value.authored_result { boundary.data_expression(&program.value.body.value.result); }
    if let Some(diagnostic) = boundary.diagnostics.first().cloned() {
        return Err(FrontendError::from_diagnostic(sources, diagnostic));
    }
    let prelude_names = prelude
        .types
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let BootstrapPrelude {
        types: mut static_environment,
        schemes: mut binding_schemes,
    } = prelude;
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
    for name in external_names {
        if authored_names.contains(name.as_str()) {
            continue;
        }
        let interface = qualified_external_interfaces.get(name);
        let scheme = imported_binding_contract(name, &qualified_external_interfaces)
            .or_else(|| dependency_facts.iter().find_map(|facts|
                facts.trait_implementations.iter().find(|implementation| implementation.dictionary == *name)
                    .map(|implementation| implementation.dictionary_scheme.clone())
                    .or_else(|| facts.type_properties.iter().find(|property| property.root == *name)
                        .map(|property| TypeScheme { parameters: Vec::new(), constraints: Vec::new(), body: property.property.clone() }))));
        let inferred = scheme.as_ref().map(|scheme| scheme.body.clone())
            .or_else(|| interface.and_then(imported_interface_descriptor))
            .ok_or_else(|| frontend_error(source_name, format!("Host binding {name:?} requires an explicit type interface")))?;
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
    validate_export_references(program, prelude_names.iter(), external_names.iter(), sources)?;

    for binding in &program.value.body.value.bindings {
        if binding.value.kind == BindingKind::NativeType {
            let name = &binding.value.name.value;
            let descriptor = native_type_contract(name, &qualified_external_interfaces)
                .map_err(|message| frontend_error(source_name, message))?;
            let witness = TypeDescriptor::TypeOf(Box::new(descriptor.clone()));
            declared_types.insert(name.clone(), descriptor);
            static_environment.insert(name.clone(), witness.clone());
            binding_types.insert(name.clone(), witness.clone());
            binding_schemes.insert(name.clone(), TypeScheme {
                parameters: Vec::new(), constraints: Vec::new(), body: witness,
            });
            continue;
        }
        if matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait) {
            static_environment.insert(binding.value.name.value.clone(), TypeDescriptor::Type);
            binding_types.insert(binding.value.name.value.clone(), TypeDescriptor::Type);
        }
    }

    let type_bindings = type_definition_bindings(&hir, &program.value.body.value.bindings);
    let type_definitions = type_bindings.keys().copied().collect::<HashSet<_>>();
    let type_dependencies = type_dependency_graph(&hir, &type_definitions);
    let dependency_plan = TypeDependencyPlan::new(&type_dependencies);
    // Every type declaration belongs to static solving, including declarations
    // whose unresolved references previously deferred them to value execution.
    let scheduled_types = type_definitions.iter().copied().collect::<BTreeSet<_>>();

    let mut pending_types = scheduled_types.clone();
    let mut evaluated_types = BTreeSet::new();
    let mut types = TypeGraph::default();
    let mut declaration_plans = Vec::new();
    let contract_external_names = external_names.iter().map(String::as_str).collect();
    let mut contract_families = static_type_families(&mut types, &qualified_external_interfaces);
    let mut schedule = dependency_plan.order(&scheduled_types).into_iter();
    while !pending_types.is_empty() {
        let component = schedule.next().expect("pending type has a scheduled component");
        let mut progressed = false;
        for definition in component.iter().copied().filter(|definition| !dependency_plan.is_cyclic(*definition)) {
            debug_assert!(dependency_plan.node(definition).dependencies.iter()
                .all(|dependency| evaluated_types.contains(dependency)));
            let binding = type_bindings[&definition];
            if binding.value.type_parameters.is_empty() {
                let static_body = StaticContractScope {
                    hir: &hir, environment: &static_environment, external_names: &contract_external_names,
                    interfaces: &qualified_external_interfaces, parameters: &[], families: &contract_families,
                }.elaborate(&binding.value.value, &mut types);
                if let Some(diagnostic) = types.elaboration_conflicts.first() {
                    return Err(FrontendError::from_diagnostic(sources, diagnostic.clone()));
                }
                let root = static_body.ok_or_else(|| FrontendError::from_diagnostic(sources,
                    Diagnostic::error("type declaration remains unknown after static solving",
                        binding.value.value.location)))?;
                let root = prepare_static_declaration(root, &mut types, binding, module_id,
                    &declared_initializer_slots, source_name, type_store)?;
                let descriptor = types.descriptor(root).map_err(|message| frontend_error(source_name, message))?;
                declaration_plans.push(DeclarationPlan::Concrete { binding, root });
                let name = binding.value.name.value.clone();
                declared_types.insert(name.clone(), descriptor.clone());
                declared_type_spans.insert(name.clone(), binding.location);
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
                pending_types.remove(&definition);
                evaluated_types.insert(definition);
                progressed = true;
                continue;
            }

            let parameters = static_contract_parameters(binding, sources)?;
            let static_scope = StaticContractScope {
                hir: &hir, environment: &static_environment, external_names: &contract_external_names,
                interfaces: &qualified_external_interfaces, parameters: &parameters, families: &contract_families,
            };
            let static_body = static_scope.elaborate(&binding.value.value, &mut types);
            let static_constraints = static_scope.constraints(&binding.value.type_parameter_bounds, &trait_ids, &mut types);
            if let Some(diagnostic) = types.elaboration_conflicts.first().or(static_constraints.unknown.first()) {
                return Err(FrontendError::from_diagnostic(sources, diagnostic.clone()));
            }
            let static_body = static_body.ok_or_else(|| FrontendError::from_diagnostic(sources,
                Diagnostic::error("type family remains unknown after static solving",
                    binding.value.value.location)))?;
            let constraints = finish_type_constraints(static_constraints.known, sources)?;
            let descriptor = types.descriptor(static_body).map_err(|message| frontend_error(source_name, message))?;
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

                TypeDescriptor::Declared(DeclaredTypeDescriptor {
                    id: crate::value::DeclaredTypeId::applied(
                        constructor.id.module,
                        constructor.id.local,
                        &arguments,
                    ),
                    name: constructor.name.clone(),
                    body: Arc::new(descriptor),
                })
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
            declaration_plans.push(DeclarationPlan::Family {
                binding,
                body: static_body,
                parameters: parameters.clone(),
                rebuild_at_runtime: contains_named_type(&descriptor),
                constructor,
            });
            let scheme = TypeScheme {
                parameters: parameters.clone(),
                constraints,
                body: TypeDescriptor::Function {
                    parameters: parameters
                        .iter()
                        .map(|parameter| {
                            TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(parameter.id)))
                        })
                        .collect(),
                    result: Box::new(TypeDescriptor::TypeOf(Box::new(descriptor))),
                },
            };
            let projected = scheme.body.clone();
            static_environment.insert(binding.value.name.value.clone(), projected.clone());
            binding_types.insert(binding.value.name.value.clone(), projected);
            if let Some(family) = StaticTypeFamily::from_scheme(&scheme, &mut types) {
                contract_families.insert(binding.value.name.value.clone(), family);
            }
            binding_schemes.insert(binding.value.name.value.clone(), scheme);
            pending_types.remove(&definition);
            evaluated_types.insert(definition);
            progressed = true;
        }
        if !progressed {
            let root = component[0];
            debug_assert!(dependency_plan.is_cyclic(root));
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
                let parameters = static_contract_parameters(binding, sources)?;
                let solved = elaborate_recursive_family(binding, module_id,
                    declared_initializer_slots[&binding.value.name.location], &parameters, &hir,
                    &static_environment, &contract_external_names, &qualified_external_interfaces,
                    &mut contract_families, &mut types);
                if let Some(diagnostic) = types.elaboration_conflicts.first() {
                    return Err(FrontendError::from_diagnostic(sources, diagnostic.clone()));
                }
                let solved = solved.ok_or_else(|| FrontendError::from_diagnostic(sources,
                    Diagnostic::error("recursive type family remains unknown after static solving",
                        binding.value.value.location)))?;
                let (scheme, rebuild_at_runtime) = prepare_recursive_family_scheme(
                    binding, parameters, &types, &solved, sources)?;
                declaration_plans.push(DeclarationPlan::RecursiveFamily {
                    binding,
                    solved,
                    parameters: scheme.parameters.clone(),
                    rebuild_at_runtime,
                });
                let projected = scheme.body.clone();
                static_environment.insert(binding.value.name.value.clone(), projected.clone());
                binding_types.insert(binding.value.name.value.clone(), projected);
                if let Some(family) = StaticTypeFamily::from_scheme(&scheme, &mut types) {
                    contract_families.insert(binding.value.name.value.clone(), family);
                }
                binding_schemes.insert(binding.value.name.value.clone(), scheme);
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
                let recursive_bindings = component.iter().map(|definition| type_bindings[definition]).collect::<Vec<_>>();
                let static_bodies = elaborate_recursive_bodies(&recursive_bindings, module_id,
                    &declared_initializer_slots, &mut static_environment, &hir, &contract_external_names,
                    &qualified_external_interfaces, &contract_families, &mut types);
                if let Some(diagnostic) = types.elaboration_conflicts.first() {
                    return Err(FrontendError::from_diagnostic(sources, diagnostic.clone()));
                }
                let static_bodies = static_bodies.ok_or_else(|| FrontendError::from_diagnostic(sources,
                    Diagnostic::error("recursive type declarations remain unknown after static solving",
                        binding.value.value.location)))?;
                for (index, definition) in component.iter().enumerate() {
                    let binding = type_bindings[definition];
                    validate_declared_graph(source_name, binding, &types, static_bodies[index].body)?;
                }
                for (index, definition) in component.into_iter().enumerate() {
                    let binding = type_bindings[&definition];
                    let descriptor = recursive_declaration_descriptor(
                        &static_bodies[index], &types,
                        binding, source_name, type_store)?;
                    let name = binding.value.name.value.clone();
                    declared_types.insert(name.clone(), descriptor.clone());
                    declared_type_spans.insert(name.clone(), binding.location);
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
                    pending_types.remove(&definition);
                    evaluated_types.insert(definition);
                }
                declaration_plans.push(DeclarationPlan::RecursiveGroup {
                    bindings: recursive_bindings, roots: static_bodies,
                });
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
        let scheme_parameters = static_contract_parameters(binding, sources)?;
        let static_scope = StaticContractScope {
            hir: &hir,
            environment: &static_environment,
            external_names: &contract_external_names,
            interfaces: &qualified_external_interfaces,
            parameters: &scheme_parameters,
            families: &contract_families,
        };
        let static_contract = static_scope.elaborate(contract, &mut types);
        let static_constraints = static_scope.constraints(&binding.value.type_parameter_bounds, &trait_ids, &mut types);
        if let Some(diagnostic) = types.elaboration_conflicts.first().or(static_constraints.unknown.first()) {
            return Err(FrontendError::from_diagnostic(sources, diagnostic.clone()));
        }
        let root = static_contract.ok_or_else(|| FrontendError::from_diagnostic(sources,
            static_scope.unknown_diagnostic(contract)))?;
        let mut scheme_constraints = finish_type_constraints(static_constraints.known, sources)?;
        if matches!(binding.value.kind, BindingKind::Def | BindingKind::Decl)
            && !program.value.body.value.bindings.iter().any(|candidate| {
                candidate.value.name.value == *name && matches!(candidate.value.value.value, ExprKind::Interpreter { .. })
            }) {
            scheme_constraints.extend(scheme_parameters.iter().map(|parameter| TypeConstraint {
                parameter: parameter.id,
                capability: TypeCapability::RuntimeType,
                location: parameter.location,
            }));
        }
        let descriptor = types.descriptor(root)
            .map_err(|message| frontend_error(source_name, message))?;
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
        let projected = descriptor.clone();
        static_environment.insert(name.clone(), projected.clone());
        binding_types.insert(name.clone(), projected);
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
    let local_trait_implementations = collect_trait_implementations(
        module_id,
        program,
        &trait_ids,
        &qualified_external_interfaces,
        &definition_contracts,
        &binding_schemes,
        sources,
    )?;
    let mut trait_implementations = qualified_external_interfaces
        .values()
        .flat_map(|interface| interface.trait_implementations.iter().cloned())
        .chain(dependency_facts.iter().flat_map(|facts| facts.trait_implementations.iter().cloned()))
        .chain(local_trait_implementations)
        .collect::<Vec<_>>();
    trait_implementations.sort_by_key(|implementation| implementation.id);
    trait_implementations.dedup_by_key(|implementation| implementation.id);
    for (index, implementation) in trait_implementations.iter().enumerate() {
        if let Some(overlap) = trait_implementations
            .iter()
            .skip(index + 1)
            .find(|candidate| trait_implementations_overlap(implementation, candidate))
        {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    "overlapping trait implementations",
                    overlap.location,
                )
                .with_secondary("overlapping implementation", implementation.location),
            ));
        }
    }

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
            hir.resolution_diagnostic(reference),
        ));
    }

    let tool_dependencies = tool_value_dependencies(&hir);
    let mut tool_bindings_to_execute = Vec::new();
    for binding in &program.value.body.value.bindings {
        match binding.value.kind {
            BindingKind::OpenImport | BindingKind::Export => continue,
            BindingKind::Decl => continue,
            BindingKind::Native | BindingKind::NativeType => continue,
            BindingKind::Type | BindingKind::Trait => continue,
            BindingKind::Let | BindingKind::Impl => {
                let inferred = (binding.value.kind == BindingKind::Let)
                    .then(|| infer_expr_projection(&binding.value.value, &static_environment)).flatten();
                let checked = if binding.value.kind == BindingKind::Impl {
                    Some(definition_contracts
                        .get(&binding.value.name.value)
                        .cloned()
                        .expect("impl contract was evaluated with its type parameter scope"))
                } else if let Some(annotation) = &binding.value.annotation {
                    let expected = StaticAnnotationContext {
                        scope: StaticContractScope { hir: &hir, environment: &static_environment,
                            external_names: &contract_external_names, interfaces: &qualified_external_interfaces,
                            parameters: &[], families: &contract_families }, graph: &mut types,
                    }.elaborate(annotation, sources)?;
                    // Preliminary let diagnostics still consume descriptor paths.
                    let expected = types.descriptor(expected)
                        .map_err(|message| frontend_error(source_name, message))?;
                    if let Some(inferred) = &inferred
                        && !contains_named_type(&expected)
                        && !same_nominal_head_with_erased_arguments(&inferred, &expected)
                        && !assignable(&inferred, &expected)
                        && !is_declared_literal_construction(
                            &binding.value.value,
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
                    Some(expected)
                } else {
                    inferred
                };
                set_projected_type(&mut static_environment, &binding.value.name.value, checked.clone());
                if let Some(checked) = checked { binding_types.insert(binding.value.name.value.clone(), checked); }
                else { binding_types.remove(&binding.value.name.value); }

                if tool_dependencies.contains(&binding.value.name.value) {
                    tool_bindings_to_execute.push(binding);
                }
            }
            BindingKind::Def => {
                let name = &binding.value.name.value;
                let checked = definition_contracts
                    .get(name)
                    .cloned();
                set_projected_type(&mut static_environment, name, checked.clone());
                if let Some(checked) = checked { binding_types.insert(name.clone(), checked); }
                else { binding_types.remove(name); }
                if tool_dependencies.contains(name) {
                    tool_bindings_to_execute.push(binding);
                }
            }
            BindingKind::Import => {
                let interface = qualified_external_interfaces.get(&binding.value.name.value);
                let scheme = interface
                    .and_then(ModuleInterface::binding_scheme)
                    .cloned();
                let inferred = interface.and_then(imported_interface_descriptor)
                    .ok_or_else(|| frontend_error(source_name, format!("import {:?} requires an explicit type interface", binding.value.name.value)))?;
                static_environment.insert(binding.value.name.value.clone(), inferred.clone());
                binding_types.insert(binding.value.name.value.clone(), inferred);
                if let Some(scheme) = scheme {
                    binding_schemes.insert(binding.value.name.value.clone(), scheme);
                }
            }
        }
    }

    let mut type_properties = qualified_external_interfaces
        .values()
        .flat_map(|interface| interface.type_properties.iter().cloned())
        .chain(dependency_facts.iter().flat_map(|facts| facts.type_properties.iter().cloned()))
        .collect::<Vec<_>>();
    type_properties.sort_by(|left, right| {
        TypeExprId::from_descriptor(&left.target)
            .cmp(&TypeExprId::from_descriptor(&right.target))
            .then_with(|| {
                TypeExprId::from_descriptor(&left.property)
                    .cmp(&TypeExprId::from_descriptor(&right.property))
            })
    });
    type_properties.dedup_by(|left, right| {
        TypeExprId::from_descriptor(&left.target) == TypeExprId::from_descriptor(&right.target)
            && TypeExprId::from_descriptor(&left.property)
                == TypeExprId::from_descriptor(&right.property)
    });

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

    let local_annotations = collect_program_annotations(
        program, sources,
        StaticContractScope { hir: &hir, environment: &static_environment,
            external_names: &contract_external_names, interfaces: &qualified_external_interfaces,
            parameters: &[], families: &contract_families }, &mut types,
    )?;
    let mut named_types = imported_named_types;
    named_types.extend(declared_types.clone());
    let dyn_namespaces = imported_dyn_namespaces(&program.value.body.value.bindings);
    let display_trait = program
        .value
        .body
        .value
        .bindings
        .iter()
        .find_map(|binding| {
            (matches!(binding.value.kind, BindingKind::Import | BindingKind::OpenImport)
                && matches!(&binding.value.value.value, ExprKind::String(path) if path == "std/fmt"))
            .then(|| {
                qualified_external_interfaces
                    .get(&binding.value.name.value)
                    .and_then(|interface| {
                        interface
                            .traits
                            .get("Display")
                            .or_else(|| interface.traits.get(&binding.value.name.value))
                            .copied()
                    })
                    .map(|id| {
                        let name = if binding.value.imported_name.is_none() {
                            format!("{}.Display", binding.value.name.value)
                        } else {
                            binding.value.name.value.clone()
                        };
                        (id, name)
                    })
            })
            .flatten()
        })
        .or_else(|| {
            qualified_external_interfaces
                .values()
                .find_map(|interface| interface.display_trait)
                .or_else(|| dependency_facts.iter().find_map(|facts| facts.display_trait))
                .map(|id| (id, "std/fmt.Display".to_owned()))
        });
    let (mut inference, checked_environment, result_type) = solve_program_types(
        source_name, program, sources, &types, ProgramTypeInputs {
            module_id,
            declaration_locations: &declaration_locations,
            binding_schemes: &binding_schemes,
            hir: &hir,
            qualified_external_interfaces: &qualified_external_interfaces,
            external_interfaces: &external_interfaces,
            named_types: &named_types,
            local_annotations: &local_annotations,
            trait_implementations: &trait_implementations,
            type_properties: &type_properties,
            trait_ids: &trait_ids,
            display_trait: display_trait,
            dyn_namespaces: &dyn_namespaces,
            static_environment: &static_environment,
            definition_contracts: &definition_contracts,
            contract_external_names: &contract_external_names,
            contract_families: &contract_families,
            builtin_tuple_available: !external_names.contains("Tuple"),
            query: query.clone(),
        }, &mut binding_types,
    )?;
    let installed_named_types = types.install_named_descriptors(&named_types);
    // Preserve binding-first nominal reservations before publishing expressions.
    // Solving is complete: keep this projection for validation and interface/ID
    // publication instead of normalizing the same bindings for each consumer.
    let binding_types = binding_types.into_iter().map(|(name, descriptor)| {
        let resolved = inference.normalize(&descriptor);
        types.intern_resolved_descriptor(&resolved);
        (name, resolved)
    }).collect::<BTreeMap<_, _>>();
    let mut publication = InferencePublication::new(&inference.variables);
    let published_expressions = publish_program_expressions(&inference, &mut publication, &mut types, sources)?;
    for (&location, &slot) in &inference.records {
        // Runtime owner bridges still consume nominal descriptors. Structural
        // expression types live only in the final graph, not a second type tree.
        if let Some(ty) = inference.variables.known(slot)
            && (matches!(inference.variables.constructor(ty), InferenceConstructor::Declared { .. } | InferenceConstructor::PendingAlternatives)
                || inference.value_constructors.contains_key(&location))
        {
            expression_descriptors.insert(location, inference.normalize(&TypeDescriptor::Inference(slot)));
        }
    }
    inference.top_level_inferred_schemes = inference
        .top_level_inferred_schemes
        .iter()
        .map(|(name, scheme)| {
            let mut scheme = scheme.clone();
            scheme.body = inference.normalize(&scheme.body);
            (name.clone(), scheme)
        })
        .collect();
    inference.inferred_schemes = inference
        .inferred_schemes
        .iter()
        .map(|(location, scheme)| {
            let mut scheme = scheme.clone();
            scheme.body = inference.normalize(&scheme.body);
            (*location, scheme)
        })
        .collect();
    let mut binding_schemes = binding_schemes.clone();
    binding_schemes.extend(inference.top_level_inferred_schemes.clone());
    let namespace_bindings = qualified_external_interfaces.iter()
        .filter(|(_, interface)| interface.value_binding.is_none())
        .map(|(name, _)| name.clone()).collect::<HashSet<_>>();
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
        if explicitly_exported_locals.contains(name.as_str()) && !namespace_bindings.contains(name) {
            binding_schemes
                .entry(name.clone())
                .or_insert_with(|| TypeScheme {
                    parameters: Vec::new(),
                    constraints: Vec::new(),
                    body: descriptor.clone(),
                });
        }
    }
    let mut resolved_result = inference.normalize(&result_type);
    // Exported callable values retain their quantified contracts, rather than an
    // unconstrained instantiation created while checking the export expression.
    match (&program.value.body.value.result.value, &mut resolved_result) {
        (ExprKind::Variable(name), descriptor) => {
            if let Some(scheme) = binding_schemes.get(&name.value) {
                *descriptor = scheme.body.clone();
            }
        }
        (ExprKind::Dict(fields), TypeDescriptor::Struct(descriptors)) => {
            for field in fields {
                if let (Some(name), ExprKind::Variable(binding)) =
                    (&field.value.name, &field.value.value.value)
                    && let Some(scheme) = binding_schemes.get(&binding.value)
                {
                    descriptors.insert(name.value.clone(), scheme.body.clone());
                    expression_descriptors.insert(field.value.value.location, scheme.body.clone());
                }
            }
        }
        _ => {}
    }
    expression_descriptors.insert(program.value.body.value.result.location, resolved_result.clone());
    let result_scheme = match &program.value.body.value.result.value {
        ExprKind::Variable(name) => binding_schemes.get(&name.value).cloned(),
        _ => None,
    }.or_else(|| (!contains_type_variable(&resolved_result)).then(|| TypeScheme {
        parameters: Vec::new(),
        constraints: Vec::new(),
        body: resolved_result.clone(),
    })).filter(|scheme| validate_publishable_scheme(scheme).is_ok());
    for (name, resolved) in &binding_types {
        if contains_standalone_sum(resolved) {
            return Err(frontend_error(source_name, format!(
                "binding {name:?} has no resolved enum owner",
            )));
        }
        if contains_type_variable(resolved) {
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
    let interface_binding_types = binding_types;
    let declared_type_names = declared_types.keys().cloned().collect::<Vec<_>>();
    let declared_types = declared_type_names
        .into_iter()
        .map(|name| (name.clone(), installed_named_types[&name]))
        .collect::<BTreeMap<_, _>>();
    let binding_types: BTreeMap<String, AnalysisTypeId> = interface_binding_types
        .iter()
        .map(|(name, descriptor)| {
            (name.clone(), types.intern_descriptor(descriptor))
        })
        .collect();
    let result_type = types.intern_resolved_descriptor(&resolved_result).ok_or_else(|| {
        frontend_error(source_name, "cannot publish unresolved result type; provide an explicit type context")
    })?;
    let expression_types: BTreeMap<HirExpressionId, AnalysisTypeId> = hir
        .expressions()
        .iter()
        .filter_map(|expression| {
            let ty = match expression_descriptors.get(&expression.location) {
                Some(descriptor) => types.intern_resolved_descriptor(descriptor),
                None => published_expressions.get(&expression.location).copied(),
            };
            ty.map(|ty| (expression.id, ty))
        })
        .collect();
    let pattern_definition_types = hir
        .definitions()
        .iter()
        .filter(|definition| definition.kind == HirDefinitionKind::Pattern)
        .filter_map(|definition| {
            inference
                .pattern_binding_types
                .get(&definition.location)
                .and_then(|descriptor| types.intern_resolved_descriptor(&inference.normalize(descriptor)))
                .map(|ty| (definition.id, ty))
        })
        .collect::<HashMap<_, _>>();
    let definition_types = hir
        .definitions()
        .iter()
        .filter_map(|definition| {
            let ty = if definition.top_level {
                binding_types.get(&definition.name).copied()
            } else {
                definition
                    .value
                    .and_then(|value| expression_types.get(&value).copied())
            }
            .or_else(|| pattern_definition_types.get(&definition.id).copied());
            ty.map(|ty| (definition.id, ty))
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
    let static_environment = static_environment.iter()
        .map(|(name, descriptor)| (name.clone(), inference.normalize(descriptor)))
        .chain(interface_binding_types.iter().map(|(name, descriptor)| (name.clone(), descriptor.clone())))
        .chain(binding_schemes.iter().filter(|(_, scheme)| !scheme.parameters.is_empty())
            .map(|(name, scheme)| (name.clone(), scheme.body.clone())))
        .collect::<HashMap<_, _>>();
    let mut tool_context = ToolInferenceContext::new(
        types, &hir, qualified_external_interfaces.clone(), static_environment.clone(),
        binding_schemes.clone(), named_types.clone(),
        !external_names.contains("Tuple"), dyn_namespaces.clone(),
    );
    tool_context.type_properties.extend(inference.local_type_properties.iter().cloned());
    // Main expressions and tool bindings share the frozen solver's publication
    // table and the same arena. Moving the graph preserves every published ID.
    let tool_bindings_to_execute = tool_bindings_to_execute.into_iter().map(|binding| {
        prepare_solved_tool_binding(binding, &inference, &mut publication, &mut tool_context, sources)
            .map(|plan| SolvedToolBinding { binding, plan })
            .map_err(|message| FrontendError::from_diagnostic(sources,
                Diagnostic::error(message, binding.value.value.location)))
    }).collect::<Result<Vec<_>, _>>()?;
    drop(publication);
    let construction_checks = prepare_construction_checks(program, &static_environment,
        query.clone(), sources, &mut tool_context)?;
    let property_plans = prepare_property_plans(program, &inference.property_contracts, &static_environment,
        query, sources, &mut tool_context)?;
    let ToolInferenceContext { mut types, .. } = tool_context;
    let type_family_constructors = declaration_plans.iter().filter_map(|plan| {
        match plan {
            DeclarationPlan::Family { binding, constructor: Some(constructor), .. } =>
                Some((binding.value.name.value.clone(), constructor.clone())),
            DeclarationPlan::RecursiveFamily { binding, .. } => Some((binding.value.name.value.clone(),
                NominalTypeConstructor {
                    id: crate::TypeConstructorId { module: module_id, local: declared_initializer_slots[&binding.value.name.location] },
                    name: binding.value.name.value.clone(),
                })),
            _ => None,
        }
    }).collect::<BTreeMap<_, _>>();
    let (declared_value_owners, owner_plans) = prepare_declared_value_owners(
        program, &expression_descriptors, &binding_schemes, &inference, &mut types,
    );
    let runtime_type_evidence = std::mem::take(&mut inference.runtime_type_evidence)
        .into_iter().map(|(name, descriptor)| (name, inference.normalize(&descriptor)))
        .collect::<BTreeMap<_, _>>();
    let local_type_property_evidence = std::mem::take(&mut inference.local_type_properties);
    let module_display_trait = if module_context.defines_display_trait() {
        trait_ids.get("Display").copied()
    } else {
        qualified_external_interfaces
            .values()
            .find_map(|interface| interface.display_trait)
            .or_else(|| dependency_facts.iter().find_map(|facts| facts.display_trait))
    };
    let module_interface = ModuleInterface {
        value_binding: None,
        member_constructors: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields.iter().filter_map(|field| {
                Some((field.value.name.as_ref()?.value.clone(),
                    inference.member_constructor_reference(&field.value.value)
                        .or_else(|| matches!(boundary.role(&field.value.value), SurfaceRole::Constructor)
                            .then_some(ValueConstructor::Newtype))?))
            }).collect(),
            _ => BTreeMap::new(),
        },
        type_declarations: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields.iter().filter_map(|field| {
                matches!(boundary.role(&field.value.value), SurfaceRole::Type | SurfaceRole::Constructor)
                    .then(|| field.value.name.as_ref().map(|name| name.value.clone())).flatten()
            }).collect(),
            _ => BTreeSet::new(),
        },
        namespaces: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields.iter().filter_map(|field| {
                let ExprKind::Variable(binding) = &field.value.value.value else { return None; };
                let interface = qualified_external_interfaces.get(&binding.value)?;
                if !namespace_bindings.contains(&binding.value) { return None; }
                Some((field.value.name.as_ref()?.value.clone(), interface.clone()))
            }).collect(),
            _ => BTreeMap::new(),
        },
        exports: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields
                .iter()
                .filter_map(|field| {
                    let ExprKind::Variable(binding) = &field.value.value.value else {
                        return None;
                    };
                    if namespace_bindings.contains(&binding.value) { return None; }
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
                                    body: inference.normalize(body),
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
        trait_implementations: trait_implementations
            .iter()
            .filter(|implementation| {
                module_context.defines_display_trait()
                    || !module_display_trait.is_some_and(|display| {
                        implementation.trait_id == display
                            && implementation.id.module == display.module
                    })
            })
            .map(published_trait_implementation)
            .collect(),
        type_properties: local_type_property_evidence.clone(),
        display_trait: module_display_trait,
        type_family_constructors: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields
                .iter()
                .filter_map(|field| {
                    let ExprKind::Variable(binding) = &field.value.value.value else {
                        return None;
                    };
                    field.value.name.as_ref().and_then(|name| {
                        type_family_constructors
                            .get(&binding.value)
                            .cloned()
                            .or_else(|| {
                                qualified_external_interfaces
                                    .get(&binding.value)
                                    .and_then(|interface| {
                                        interface.type_family_constructors.get(&binding.value)
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
    let trait_member_evidence = std::mem::take(&mut inference.resolved_trait_members);
    let generic_call_evidence = std::mem::take(&mut inference.resolved_call_evidence);
    let interpolation_evidence =
        std::mem::take(&mut inference.resolved_interpolation_evidence);
    let mut generic_evidence_parameters: HashMap<_, _> = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter_map(|binding| {
            let scheme = binding_schemes.get(&binding.value.name.value)?;
            (!scheme.constraints.is_empty()).then(|| {
                (
                    binding.value.value.location,
                    scheme
                        .constraints
                        .iter()
                        .enumerate()
                        .map(|(index, _)| evidence_parameter_name(&binding.value.name.value, index))
                        .collect(),
                )
            })
        })
        .collect();
    generic_evidence_parameters.extend(inference.inferred_runtime_scopes.iter().map(|(location, evidence)| {
        (*location, evidence.iter().map(|evidence| evidence.name.clone()).collect())
    }));
    let generic_dictionary_factories = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|&binding| binding.value.kind == BindingKind::Impl
                && !binding.value.type_parameters.is_empty()).map(|binding| {
                let scheme = binding_schemes
                    .get(&binding.value.name.value)
                    .expect("blanket impl has a static scheme");
                let mut parameters = binding
                    .value
                    .type_parameters
                    .iter()
                    .map(|parameter| parameter.value.clone())
                    .collect::<Vec<_>>();
                parameters.extend(
                    scheme
                        .constraints
                        .iter()
                        .enumerate()
                        .map(|(index, _)| evidence_parameter_name(&binding.value.name.value, index)),
                );
                (binding.value.value.location, parameters)
            })
        .collect();
    let value_constructors = std::mem::take(&mut inference.value_constructors);
    // All inference-dependent interface and compiler evidence is now final.
    // Execution below cannot query, normalize through, or revive this solver.
    drop(inference);
    Ok(SolvedModulePlan {
        types, declared_types, binding_types, trait_ids, trait_implementations,
        result_type, result_scheme, hir, definition_types, definition_schemes,
        expression_types, module_interface, propagation_families, not_families,
        trait_member_evidence, generic_call_evidence, interpolation_evidence,
        generic_evidence_parameters, generic_dictionary_factories,
        declared_value_owners, value_constructors, authored_names, prelude_value_names,
        declared_initializer_slots, declaration_plans, tool_bindings_to_execute,
        construction_checks, property_plans, local_type_property_evidence,
        static_environment, runtime_type_evidence, owner_plans,
    })
}

#[allow(clippy::too_many_arguments)]
fn execute_module_plan(
    source_name: &str,
    module_id: crate::ModuleId,
    program: &Program,
    solved: SolvedModulePlan<'_>,
    account: &mut QuotaAccount,
    external_roots: &BTreeMap<String, PersistentValue>,
    dynamic_bindings: &HashSet<String>,
    sources: &SourceDatabase,
    debug_sink: &Arc<dyn DebugSink>,
    tool_heap: &mut Heap,
) -> Result<Analysis, FrontendError> {
    let SolvedModulePlan {
        types, declared_types, binding_types, trait_ids, trait_implementations,
        result_type, result_scheme, hir, definition_types, definition_schemes,
        expression_types, module_interface, propagation_families, not_families,
        trait_member_evidence, generic_call_evidence, interpolation_evidence,
        generic_evidence_parameters, generic_dictionary_factories,
        declared_value_owners, value_constructors, authored_names, prelude_value_names,
        declared_initializer_slots, declaration_plans, tool_bindings_to_execute,
        construction_checks, property_plans, local_type_property_evidence,
        static_environment, runtime_type_evidence, owner_plans,
    } = solved;
    let mut tool_bindings_to_execute = tool_bindings_to_execute.into_iter()
        .map(|SolvedToolBinding { binding, plan }| ToolBindingTask { binding, plan, value: None })
        .collect::<Vec<_>>();
    // Execution resources are acquired only after this module's static plans
    // have been solved. Runtime linking is also deferred to this boundary.
    let mut tool_values = link_external_tool_values(
        source_name, sources, program, external_roots, dynamic_bindings, &authored_names,
    )?;
    let cached_bootstrap_root = tool_heap.bootstrap_root();
    let mut evaluator = ToolEvaluator::new(Arc::clone(debug_sink), tool_heap);
    let mut bootstrap_values = if let Some(root) = cached_bootstrap_root {
        prelude_value_names.iter().map(|name| {
            let value = root.export_get(evaluator.main, name)
                .expect("bootstrap exports root is a Dict")
                .unwrap_or_else(|| panic!("bootstrap exports root is missing {name:?}"));
            (name.clone(), value.runtime())
        }).collect()
    } else {
        evaluator.install_bootstrap()?
    };
    bootstrap_values.append(&mut tool_values);
    let mut tool_values = bootstrap_values;
    stage_pending_construction_checks(module_id, program, &declared_initializer_slots, &mut evaluator)?;
    let mut type_family_values = BTreeMap::new();
    materialize_declarations(declaration_plans, &types, module_id, &declared_initializer_slots,
        source_name, &mut tool_values, &mut type_family_values, &mut evaluator)?;
    evaluator.tool_types = types;
    prepare_construction_dependencies(source_name, program, &hir, &mut tool_bindings_to_execute,
        &construction_checks, &mut tool_values,
        account, sources, &mut evaluator)?;
    for task in &mut tool_bindings_to_execute {
        evaluate_construction_checks(source_name, &construction_checks, &tool_values,
            account, sources, &mut evaluator, false)?;
        if let Ok(value) = task.execute(source_name,
            &tool_values, account, sources, &mut evaluator) {
            tool_values.insert(task.binding.value.name.value.clone(), value);
        }
    }
    evaluate_construction_checks(source_name, &construction_checks, &tool_values,
        account, sources, &mut evaluator, true)?;
    // Property presence follows the provider contract. Values are materialized
    // only after the module's static obligations have succeeded.
    let local_type_properties = evaluate_declared_properties(
        source_name, program, &property_plans, &local_type_property_evidence, &tool_values, &static_environment,
        account, sources, &mut evaluator,
    )?;
    let local_type_property_roots = local_type_properties.iter()
        .map(|(evidence, root)| (evidence.root.clone(), *root))
        .collect::<BTreeMap<_, _>>();
    if local_type_property_evidence.iter().any(|evidence| !local_type_property_roots.contains_key(&evidence.root)) {
        return Err(frontend_error(source_name, "declared property evidence was not materialized"));
    }
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
    runtime_roots.extend(local_type_property_roots);
    if !runtime_type_evidence.is_empty() {
        let names = runtime_type_evidence.keys().cloned().collect::<Vec<_>>();
        let mut values = Vec::new();
        for (name, descriptor) in runtime_type_evidence {
            let value = evaluator.descriptor(&descriptor)?;
            let mut parameters = Vec::new();
            collect_bound_parameters(&descriptor, &mut parameters);
            let value = if name.starts_with("\0type_argument:") && !parameters.is_empty() {
                let arity = parameters.iter().map(|parameter| parameter.0 as usize + 1).max().unwrap();
                evaluator.create_type_family(value, arity, None)?.0
            } else { value };
            values.push((name, value));
        }
        let root = evaluator.persist_table(values)?;
        for name in names {
            let value = root
                .export_get(evaluator.main, &name)
                .expect("runtime type evidence table is a Module")
                .expect("runtime type evidence is present");
            runtime_roots.insert(name, value);
        }
    }
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
    let owner_values = evaluator.work.type_graph_values_in(
        Some(evaluator.main), &evaluator.tool_types, owner_plans.iter().map(|plan| plan.root),
        &mut evaluator.tool_type_values,
    ).map_err(|error| frontend_error(source_name, error.to_string()))?;
    for (plan, value) in owner_plans.into_iter().zip(owner_values) {
        let value = if plan.arity == 0 { value } else {
            evaluator.create_type_family(value, plan.arity, None)?.0
        };
        pending_owner_roots.push((plan.key, value));
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
    let types = std::mem::take(&mut evaluator.tool_types);
    Ok(Analysis {
        types,
        declared_types,
        binding_types,
        trait_ids,
        trait_implementations,
        result_type,
        result_scheme,
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
        trait_member_evidence,
        generic_call_evidence,
        interpolation_evidence,
        generic_evidence_parameters,
        generic_dictionary_factories,
        runtime_roots,
        external_bindings,
        dynamic_bindings: dynamic_bindings.clone(),
        type_family_values,
        declared_value_owners,
        value_constructors,
    })
}
