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

struct ProgramTypeOutcome<'a> {
    inference: GenericInference<'a>,
    environment: ScopedTypeEnvironment<'a>,
    result: TypeDescriptor,
    diagnostics: Vec<Diagnostic>,
}

// Binding conflicts remain in this outcome with independently solved facts.
// No evaluator, VM or runtime heap is available to this solver.
fn solve_program_types<'a>(
    _source_name: &str,
    program: &Program,
    sources: &SourceDatabase,
    annotation_graph: &TypeGraph,
    inputs: ProgramTypeInputs<'a>,
    binding_types: &mut BTreeMap<String, TypeDescriptor>,
) -> Result<ProgramTypeOutcome<'a>, FrontendError> {
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
    let annotation_inputs =
        InferenceAnnotationInputs::from_graph(annotation_graph, local_annotations, sources)?;
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
    let mut diagnostics = Vec::new();
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
    for reference in hir.references() {
        if reference.resolution != HirResolution::External {
            continue;
        }
        if let Some(origin) = hir.reference_import_origin(reference.id)
            && let Some(descriptor) = static_environment.get(&reference.name)
        {
            inference.bind_import_origin(
                origin,
                descriptor.clone(),
                binding_schemes.get(&reference.name).cloned(),
            );
        }
    }
    let mut delayed_bindings = Vec::new();
    let mut recursive_skeletons = HashMap::new();
    let component_plan = definition_component_plan(&program.value.body, hir);
    for location in &component_plan.indirect_recursive {
        diagnostics.push(Diagnostic::error(
            "indirect recursive definition requires an explicit contract",
            *location,
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
        let outcome = (|| -> Result<(), FrontendError> {
            inference.delayed_initializer_depth += 1;
            inference.recursive_body_inference_depth += 1;
            let inferred =
                inference.infer(&binding.value.value, &checked_environment, Some(skeleton));
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
            Ok(())
        })();
        if let Err(error) = outcome {
            record_program_binding_conflict(
                &mut inference,
                &mut checked_environment,
                binding_types,
                binding,
                error,
                &mut diagnostics,
            );
        }
    }
    if let Err(message) = inference.solve_recursive_equations(&recursive_variables) {
        diagnostics.push(Diagnostic::error(message, program.location));
    }
    for location in &component_plan.acyclic {
        let binding = program
            .value
            .body
            .value
            .bindings
            .iter()
            .find(|binding| binding.value.name.location == *location)
            .expect("component binding exists");
        let outcome = (|| -> Result<(), FrontendError> {
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
            Ok(())
        })();
        if let Err(error) = outcome {
            record_program_binding_conflict(
                &mut inference,
                &mut checked_environment,
                binding_types,
                binding,
                error,
                &mut diagnostics,
            );
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
            if matches!(
                binding.value.kind,
                BindingKind::Import | BindingKind::OpenImport
            ) && matches!(binding.value.value.value, ExprKind::String(_))
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
        let outcome = (|| -> Result<(), FrontendError> {
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
                return Ok(());
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
            Ok(())
        })();
        if let Err(error) = outcome {
            record_program_binding_conflict(
                &mut inference,
                &mut checked_environment,
                binding_types,
                binding,
                error,
                &mut diagnostics,
            );
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
    let result_type =
        match inference.infer(&program.value.body.value.result, &checked_environment, None) {
            Ok(ty) => ty,
            Err(message) => {
                close_program_binding_scopes(&mut inference);
                let ty = inference.fresh_variable();
                inference.variables.record_conflict(&ty, &message);
                inference.record_type(program.value.body.value.result.location, ty.clone());
                diagnostics.push(inference.take_failure_diagnostic(
                    program.value.body.value.result.location,
                    message,
                    None,
                ));
                ty
            }
        };
    let module_requirement = inference
        .propagation_boundaries
        .pop()
        .expect("module propagation boundary exists");
    let result_type = inference
        .finish_propagation_boundary(result_type.clone(), None, module_requirement)
        .unwrap_or_else(|message| {
            let ty = inference.fresh_variable();
            inference.variables.record_conflict(&ty, &message);
            inference.record_type(program.value.body.value.result.location, ty.clone());
            diagnostics.push(Diagnostic::error(
                message,
                program.value.body.value.result.location,
            ));
            ty
        });
    match solve_declared_property_contracts(program, &checked_environment, &mut inference, sources)
    {
        Ok(contracts) => {
            inference.local_type_properties = declared_property_evidence(module_id, contracts)
        }
        Err(error) => diagnostics.push(
            error
                .diagnostic
                .map(|diagnostic| *diagnostic)
                .unwrap_or_else(|| Diagnostic::error(error.message, program.location)),
        ),
    }
    diagnostics.extend(
        inference
            .pattern_diagnostics
            .iter()
            .map(|(location, message)| Diagnostic::error(message.clone(), *location)),
    );
    if let Some((location, message)) = inference.unresolved_placeholder_since(0) {
        diagnostics.push(Diagnostic::error(message, location));
    }
    for result in [
        inference.finish_type_constraints(),
        inference.finish_interpolations(),
        inference.finish_enum_constructors(),
    ] {
        if let Err((location, message)) = result {
            diagnostics.push(Diagnostic::error(message, location));
        }
    }
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
            diagnostics.push(Diagnostic::error(
                format!(
                    "cannot infer monomorphic binding {name:?}: unresolved {}",
                    inference.normalize(&descriptor).display_name()
                ),
                location,
            ));
        }
    }
    inference.variables.canonicalize_slots();
    Ok(ProgramTypeOutcome {
        inference,
        environment: checked_environment,
        result: result_type,
        diagnostics,
    })
}

fn record_program_binding_conflict(
    inference: &mut GenericInference<'_>,
    environment: &mut dyn MutableTypeEnvironment,
    binding_types: &mut BTreeMap<String, TypeDescriptor>,
    binding: &crate::ast::Binding,
    error: FrontendError,
    diagnostics: &mut Vec<Diagnostic>,
) {
    close_program_binding_scopes(inference);
    let diagnostic = error
        .diagnostic
        .map(|diagnostic| *diagnostic)
        .unwrap_or_else(|| Diagnostic::error(error.message, binding.value.value.location));
    let conflicted = inference.fresh_variable();
    inference
        .variables
        .record_conflict(&conflicted, &diagnostic.message);
    inference.record_type(binding.value.value.location, conflicted.clone());
    environment.insert(binding.value.name.value.clone(), conflicted.clone());
    inference.bind_local(
        environment,
        binding.value.name.location,
        &binding.value.name.value,
        conflicted.clone(),
        None,
    );
    binding_types.insert(binding.value.name.value.clone(), conflicted);
    diagnostics.push(diagnostic);
}

fn close_program_binding_scopes(inference: &mut GenericInference<'_>) {
    // A top-level initializer has ended. Close lexical frames even when its
    // evidence conflicted; do not roll back any arena node or recorded fact.
    inference.scheme_scopes.truncate(1);
    inference.propagation_boundaries.truncate(1);
    inference.return_boundaries.truncate(1);
    inference.lexical_type_evidence.clear();
    inference.closure_inference_depth = 0;
    inference.type_syntax_depth = 0;
    inference.delayed_initializer_depth = 0;
    inference.recursive_body_inference_depth = 0;
}

#[cfg(test)]
mod program_outcome_tests {
    use super::*;

    #[test]
    fn keeps_independent_types_and_conflicts_in_one_solve() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("outcomes.telora",
            "def bad = fn(x: Int) { x + \"wrong\" }; def other = 1 + \"wrong\"; def good = 42; good");
        let parsed = parse_registered(&sources, source);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let program = parsed.program.unwrap();
        let hir = resolve_module_hir(&program, &BTreeSet::new(), HashSet::new());
        let prelude = BootstrapPrelude::new();
        let empty_map = HashMap::new();
        let empty_tree = BTreeMap::new();
        let named_types = BTreeMap::new();
        let annotations = HashMap::new();
        let trait_ids = BTreeMap::new();
        let dyn_namespaces = HashSet::new();
        let contracts = HashMap::new();
        let external_names = HashSet::new();
        let families = BTreeMap::new();
        let mut bindings = BTreeMap::new();
        let ProgramTypeOutcome {
            inference,
            result,
            diagnostics,
            ..
        } = solve_program_types(
            "outcomes.telora",
            &program,
            &sources,
            &TypeGraph::default(),
            ProgramTypeInputs {
                module_id: crate::ModuleId::ANONYMOUS,
                declaration_locations: &empty_map,
                binding_schemes: &prelude.schemes,
                hir: &hir,
                qualified_external_interfaces: &empty_tree,
                external_interfaces: &empty_tree,
                named_types: &named_types,
                local_annotations: &annotations,
                trait_implementations: &[],
                type_properties: &[],
                trait_ids: &trait_ids,
                display_trait: None,
                dyn_namespaces: &dyn_namespaces,
                static_environment: &prelude.types,
                definition_contracts: &contracts,
                contract_external_names: &external_names,
                contract_families: &families,
                builtin_tuple_available: true,
                query: None,
            },
            &mut bindings,
        )
        .unwrap();
        assert!(diagnostics.len() >= 2, "{diagnostics:?}");
        for name in ["bad", "other"] {
            assert!(
                inference
                    .variables
                    .ensure_consistent(&bindings[name])
                    .is_err(),
                "{name}"
            );
        }
        assert_eq!(inference.normalize(&bindings["good"]), TypeDescriptor::Int);
        assert_eq!(inference.normalize(&result), TypeDescriptor::Int);
        assert_eq!(inference.scheme_scopes.len(), 1);
        assert_eq!(inference.closure_inference_depth, 0);
    }
}
