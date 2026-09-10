struct ProgramTypeInputs<'a> {
    module_id: crate::ModuleId,
    declaration_locations: &'a HashMap<String, crate::Location>,
    binding_schemes: &'a HashMap<String, TypeScheme>,
    hir: &'a HirProgram,
    qualified_external_interfaces: &'a BTreeMap<String, ModuleInterface>,
    external_interfaces: &'a BTreeMap<String, ModuleInterface>,
    named_types: &'a BTreeMap<String, TypeDescriptor>,
    local_annotations: &'a HashMap<crate::Location, AnalysisTypeId>,
    trait_implementations: &'a [TraitImplementation],
    type_properties: &'a [TypePropertyEvidence],
    trait_ids: &'a BTreeMap<String, crate::TraitId>,
    display_trait: Option<(crate::TraitId, String)>,
    dyn_namespaces: &'a HashSet<String>,
    static_environment: &'a HashMap<String, TypeDescriptor>,
    definition_contracts: &'a HashMap<String, TypeDescriptor>,
    contract_external_names: &'a HashSet<&'a str>,
    contract_families: &'a BTreeMap<String, StaticTypeFamily>,
    builtin_tuple_available: bool,
    query: Option<crate::query::QueryContext>,
}

// Complete program inference has no evaluator, VM or runtime heap input.
fn solve_program_types<'a>(
    source_name: &str,
    program: &Program,
    sources: &SourceDatabase,
    annotation_graph: &TypeGraph,
    inputs: ProgramTypeInputs<'a>,
    binding_types: &mut BTreeMap<String, TypeDescriptor>,
) -> Result<
    (
        GenericInference<'a>,
        ScopedTypeEnvironment<'a>,
        TypeDescriptor,
    ),
    FrontendError,
> {
    let ProgramTypeInputs {
        module_id,
        declaration_locations,
        binding_schemes,
        hir,
        qualified_external_interfaces,
        external_interfaces,
        named_types,
        local_annotations,
        trait_implementations,
        type_properties,
        trait_ids,
        display_trait,
        dyn_namespaces,
        static_environment,
        definition_contracts,
        contract_external_names,
        contract_families,
        builtin_tuple_available,
        query,
    } = inputs;
    let annotation_inputs = InferenceAnnotationInputs::from_graph(annotation_graph, local_annotations, sources)?;
    let mut inference = GenericInference::new(
        binding_schemes,
        hir,
        qualified_external_interfaces,
        named_types,
        annotation_inputs,
        trait_implementations,
        type_properties,
        trait_ids,
        display_trait,
        dyn_namespaces,
        builtin_tuple_available,
        None,
        query,
    );
    let mut checked_environment = ScopedTypeEnvironment::new(static_environment);
    let mut value_slots = HashMap::new();
    for binding in &program.value.body.value.bindings {
        if binding.value.kind == BindingKind::Def
            && binding.value.annotation.is_none()
            && !definition_contracts.contains_key(&binding.value.name.value)
            && !matches!(binding.value.value.value, ExprKind::Closure { .. })
            && !static_environment.contains_key(&binding.value.name.value)
        {
            let slot = inference.fresh_variable();
            checked_environment.insert(binding.value.name.value.clone(), slot.clone());
            value_slots.insert(binding.value.name.location, slot.clone());
            inference.bind_local(
                &mut checked_environment,
                binding.value.name.location,
                &binding.value.name.value,
                slot,
                None,
            );
        }
    }
    for definition in hir
        .definitions()
        .iter()
        .filter(|definition| definition.top_level)
    {
        if let Some(descriptor) = static_environment.get(&definition.name) {
            inference.bind_local(
                &mut checked_environment,
                definition.location,
                &definition.name,
                descriptor.clone(),
                binding_schemes.get(&definition.name).cloned(),
            );
        }
    }
    let type_metadata_expected = TypeDescriptor::Type;
    let mut delayed_bindings = Vec::new();
    let mut recursive_skeletons = HashMap::new();
    let component_plan = definition_component_plan(&program.value.body, hir);
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
        let first_owned_variable = inference.variables.next_id();
        if let Some(skeleton) = inference.recursive_closure_skeleton(&binding.value.value) {
            checked_environment.insert(binding.value.name.value.clone(), skeleton.clone());
            inference.bind_local(
                &mut checked_environment,
                binding.value.name.location,
                &binding.value.name.value,
                skeleton.clone(),
                None,
            );
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
        if binding.value.kind != BindingKind::Def {
            continue;
        }
        inference.delayed_initializer_depth += 1;
        inference.recursive_body_inference_depth += 1;
        let inferred = inference.infer(&binding.value.value, &checked_environment, Some(skeleton));
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
            (*inference.variables.head(&inferred)).clone(),
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
        let first_owned_variable = inference.variables.next_id();
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
            .generalize_local_closure(
                &inferred,
                first_owned_variable,
                binding.value.name.location,
                binding.value.value.location,
            )
            .map_err(|message| {
                FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(message, binding.value.value.location),
                )
            })?;
        let descriptor = scheme.as_ref().map_or_else(
            || inference.normalize(&inferred),
            |scheme| scheme.body.clone(),
        );
        checked_environment.insert(binding.value.name.value.clone(), descriptor.clone());
        inference.bind_local(
            &mut checked_environment,
            binding.value.name.location,
            &binding.value.name.value,
            descriptor.clone(),
            scheme.clone(),
        );
        binding_types.insert(binding.value.name.value.clone(), descriptor);
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
            // Import paths are syntax inputs to module resolution, not value
            // initializers, but their literal type still belongs in the solved
            // expression graph used by semantic consumers.
            if matches!(binding.value.kind, BindingKind::Import | BindingKind::OpenImport)
                && matches!(binding.value.value.value, ExprKind::String(_))
            {
                let slot = inference.variables.structure_edge(TypeDescriptor::String);
                inference.records.insert(binding.value.value.location, slot);
            }
            if binding.value.kind == BindingKind::Import {
                let scheme = external_interfaces
                    .get(&binding.value.name.value)
                    .and_then(ModuleInterface::binding_scheme)
                    .cloned();
                if let Some(descriptor) =
                    checked_environment.get(&binding.value.name.value).cloned()
                {
                    inference.bind_local(
                        &mut checked_environment,
                        binding.value.name.location,
                        &binding.value.name.value,
                        descriptor,
                        scheme,
                    );
                } else {
                    inference.set_local_scheme(binding.value.name.value.clone(), scheme);
                }
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
            .map_or(inference.variables.next_id(), |(_, first)| *first);
        if is_delayed {
            inference.delayed_initializer_depth += 1;
        }
        let mut initializer_environment = None;
        if is_delayed && binding.value.kind == BindingKind::Def && !is_recursive {
            let mut environment = ScopedTypeEnvironment::new(&checked_environment);
            environment.remove(&binding.value.name.value);
            if let Some(definition) =
                hir.definition_at(binding.value.name.location, &binding.value.name.value)
            {
                inference.definition_bindings[definition.id.index()] = None;
            }
            initializer_environment = Some(environment);
        } else if matches!(
            binding.value.kind,
            BindingKind::Type | BindingKind::Trait | BindingKind::Impl
        ) && !binding.value.type_parameters.is_empty()
        {
            let mut environment = ScopedTypeEnvironment::new(&checked_environment);
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
        let environment = initializer_environment.as_ref().map_or(
            &checked_environment as &dyn TypeEnvironment,
            |environment| environment,
        );
        let lexical_evidence_start = binding_schemes
            .get(&binding.value.name.value)
            .cloned()
            .map(|scheme| inference.push_lexical_evidence(&binding.value.name.value, &scheme));
        if let Some(annotation) = &binding.value.annotation {
            let mut contract_environment = ScopedTypeEnvironment::new(environment);
            for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
                contract_environment.insert(
                    parameter.value.clone(),
                    TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(TypeParameterId(
                        index as u32,
                    )))),
                );
            }
            StaticContractScope {
                hir: hir,
                environment: static_environment,
                external_names: contract_external_names,
                interfaces: qualified_external_interfaces,
                parameters: binding_schemes
                    .get(&binding.value.name.value)
                    .map_or(&[], |scheme| &scheme.parameters),
                families: contract_families,
            }
            .check_contract_obligations(annotation, &mut inference, &contract_environment)
            .map_err(|(location, message)| {
                FrontendError::from_diagnostic(
                    sources,
                    inference.take_failure_diagnostic(location, message, None),
                )
            })?;
        }
        if binding.value.kind == BindingKind::Impl {
            inference.lexical_type_evidence.extend(
                binding
                    .value
                    .type_parameters
                    .iter()
                    .enumerate()
                    .map(|(index, parameter)| LexicalTypeEvidence {
                        capability: TypeCapability::RuntimeType,
                        target: TypeDescriptor::Bound(TypeParameterId(index as u32)),
                        name: parameter.value.clone(),
                    }),
            );
        }
        let inferred = if matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait) {
            inference.infer(&binding.value.value, environment, expected)
        } else {
            inference.infer(&binding.value.value, environment, expected)
        };
        if let Some(start) = lexical_evidence_start {
            inference.pop_lexical_evidence(start);
        }
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
            if let Some(slot) = value_slots.get(&binding.value.name.location) {
                inference.unify(slot, &inferred).map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        inference.take_failure_diagnostic(
                            binding.value.value.location,
                            message,
                            None,
                        ),
                    )
                })?;
            }
            let inferred_scheme = if binding.value.is_member_import() {
                Some(
                    inference
                        .member_import_scheme(&binding.value, &inferred)
                        .map_err(|message| {
                            FrontendError::from_diagnostic(
                                sources,
                                Diagnostic::error(message, binding.value.value.location),
                            )
                        })?,
                )
            } else if binding.value.kind == BindingKind::Let
                && binding.value.annotation.is_none()
                && binding.value.type_parameters.is_empty()
                && matches!(binding.value.value.value, ExprKind::Closure { .. })
            {
                inference
                    .generalize_local_closure(
                        &inferred,
                        first_owned_variable,
                        binding.value.name.location,
                        binding.value.value.location,
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
            let checked_scheme = if inferred_scheme.is_some()
                || binding.value.kind == BindingKind::Let
                || binding.value.annotation.is_none()
                    && !definition_contracts.contains_key(&binding.value.name.value)
            {
                inferred_scheme.clone()
            } else {
                binding_schemes.get(&binding.value.name.value).cloned()
            };
            inference.bind_local(
                &mut checked_environment,
                binding.value.name.location,
                &binding.value.name.value,
                checked.clone(),
                checked_scheme,
            );
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
    if !program.value.authored_result
        && let ExprKind::Dict(fields) = &program.value.body.value.result.value
    {
        for field in fields {
            inference
                .type_facet_locations
                .insert(field.value.value.location);
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
    let property_contracts = solve_declared_property_contracts(
        program, &checked_environment, &mut inference, sources,
    )?;
    inference.local_type_properties = declared_property_evidence(module_id, property_contracts);
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
    inference
        .finish_interpolations()
        .map_err(|(location, message)| {
            FrontendError::from_diagnostic(sources, Diagnostic::error(message, location))
        })?;
    inference
        .finish_enum_constructors()
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
        if inference.contains_owned_unknown(&descriptor, first_owned_variable) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!(
                        "cannot infer monomorphic binding {name:?}: unresolved {}",
                        inference.normalize(&descriptor).display_name()
                    ),
                    location,
                ),
            ));
        }
    }
    inference.variables.canonicalize_slots();
    Ok((inference, checked_environment, result_type))
}
