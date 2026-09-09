struct SolvedRecursiveType {
    owner: AnalysisTypeId,
    body: AnalysisTypeId,
}

// Reserve every nominal identity before elaborating any recursive body. All
// references then target the same rows as those bodies are filled in.
#[allow(clippy::too_many_arguments)]
fn elaborate_recursive_bodies(
    bindings: &[&Binding], module_id: crate::ModuleId,
    slots: &HashMap<crate::Location, u32>, environment: &mut HashMap<String, TypeDescriptor>,
    hir: &HirProgram, external_names: &HashSet<&str>, interfaces: &BTreeMap<String, ModuleInterface>,
    families: &BTreeMap<String, StaticTypeFamily>, graph: &mut TypeGraph,
) -> Option<Vec<SolvedRecursiveType>> {
    let mut owners = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let descriptor = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(module_id, slots[&binding.value.name.location]),
            name: binding.value.name.value.clone(), body: Arc::new(TypeDescriptor::Never),
        });
        owners.push(graph.intern_descriptor(&descriptor));
        environment.insert(binding.value.name.value.clone(), TypeDescriptor::TypeOf(Box::new(descriptor)));
    }
    let scope = StaticContractScope { hir, environment, external_names, interfaces, parameters: &[], families };
    let roots = bindings.iter().map(|binding| scope.elaborate(&binding.value.value, graph))
        .collect::<Option<Vec<_>>>()?;
    Some(owners.into_iter().zip(roots).map(|(owner, body)| {
        graph.fill_declared_body(owner, body);
        SolvedRecursiveType { owner, body }
    }).collect())
}

#[allow(clippy::too_many_arguments)]
fn recursive_declaration_descriptor(
    solved: Option<&SolvedRecursiveType>, graph: &TypeGraph, value: Val,
    binding: &Binding, source_name: &str, evaluator: &ToolEvaluator<'_>, store: &mut TypeStore,
) -> Result<TypeDescriptor, FrontendError> {
    let invalid = |message| frontend_error(source_name, format!(
        "type {} produced invalid metadata: {message}", binding.value.name.value));
    let decoded;
    let (graph, root) = if let Some(solved) = solved {
        (graph, solved.owner)
    } else {
        decoded = evaluator.decode_type_graph(value, "Type").map_err(invalid)?;
        (&decoded.0, decoded.1)
    };
    let descriptor = graph.descriptor(root).map_err(invalid)?;
    graph.canonicalize(root, store).map_err(|message| frontend_error(source_name, message))?;
    Ok(descriptor)
}

// Metadata is currently still consumed by legacy family/value preparation.
#[allow(clippy::too_many_arguments)]
fn materialize_static_declaration(
    root: AnalysisTypeId, graph: &mut TypeGraph, binding: &Binding, module_id: crate::ModuleId,
    slots: &HashMap<crate::Location, u32>, source_name: &str, type_store: &mut TypeStore,
    evaluator: &mut ToolEvaluator<'_>, values: &dyn ToolBindings,
) -> Result<(Val, TypeDescriptor), FrontendError> {
    let root = if binding.value.declared_initializer.is_some() {
        let body = graph.descriptor(root).map_err(|message| frontend_error(source_name, message))?;
        graph.intern_descriptor(&TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(module_id, slots[&binding.value.name.location]),
            name: binding.value.name.value.clone(), body: Arc::new(body),
        }))
    } else { root };
    graph.canonicalize(root, type_store).map_err(|message| frontend_error(source_name, message))?;
    let descriptor = graph.descriptor(root).map_err(|message| frontend_error(source_name, message))?;
    Ok((evaluator.descriptor_with_origins(&descriptor, &binding.value.value, values)?, descriptor))
}

// Metadata is currently still consumed by legacy family/value preparation.
// A solved body enters that boundary directly; it is never executed or decoded.
#[allow(clippy::too_many_arguments)]
fn materialize_type_body(
    root: Option<AnalysisTypeId>, graph: &TypeGraph, binding: &Binding,
    source_name: &str, values: &dyn ToolBindings, account: &mut QuotaAccount,
    sources: &SourceDatabase, evaluator: &mut ToolEvaluator<'_>,
) -> Result<(Val, TypeDescriptor), FrontendError> {
    if let Some(root) = root {
        let descriptor = graph.descriptor(root).map_err(|message| frontend_error(source_name, message))?;
        return Ok((evaluator.descriptor_with_origins(&descriptor, &binding.value.value, values)?, descriptor));
    }
    let value = evaluate_tool_expression(source_name, &binding.value.value, values, account, sources, evaluator)?;
    let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
        FrontendError::from_diagnostic(sources, Diagnostic::error(
            format!("type family {} produced invalid metadata: {message}", binding.value.name.value),
            binding.value.value.location))
    })?;
    Ok((value, descriptor))
}

fn imported_static_descriptor(
    value: ValueRef<'_>,
    interface: Option<&ModuleInterface>,
) -> Option<TypeDescriptor> {
    let Some(interface) = interface else {
        return infer_value_ref(value);
    };
    if interface.exports.is_empty() && interface.namespaces.is_empty() && value.kind() != ValueKind::Module {
        return infer_value_ref(value);
    }
    if let Some(scheme) = interface.binding_scheme() {
        return Some(scheme.body.clone());
    }
    Some(interface_descriptor(interface))
}

fn interface_descriptor(interface: &ModuleInterface) -> TypeDescriptor {
    TypeDescriptor::Struct(
        interface
            .exports
            .iter()
            .map(|(name, scheme)| (name.clone(), scheme.body.clone()))
            .chain(interface.namespaces.iter().map(|(name, namespace)| (name.clone(), interface_descriptor(namespace))))
            .collect(),
    )
}

fn validate_interpreter_contract(
    type_parameters: &[crate::ast::Identifier],
    contract: Option<&TypeDescriptor>,
) -> Result<(), String> {
    if contract.is_none() {
        return Err(
            "interpreter requires an explicit for(A, ...) Fn(TypeOf(A), ...) -> Fn(...) -> R definition contract"
                .into(),
        );
    }
    if type_parameters.is_empty() {
        return Err("interpreter requires at least one quantified type parameter".into());
    }
    let Some(TypeDescriptor::Function {
        parameters: outer_parameters,
        result: outer_result,
    }) = contract
    else {
        return Err(
            "interpreter contract must return an inner Function from explicit TypeOf witnesses"
                .into(),
        );
    };

    let mut witnesses = HashMap::new();
    for (index, witness) in outer_parameters.iter().enumerate() {
        let TypeDescriptor::TypeOf(parameter) = witness else {
            return Err(format!(
                "interpreter witness parameter {} must have type TypeOf(A)",
                index + 1
            ));
        };
        let TypeDescriptor::Bound(parameter) = parameter.as_ref() else {
            return Err(format!(
                "interpreter witness parameter {} must name a quantified type parameter",
                index + 1
            ));
        };
        let Some(name) = type_parameters.get(parameter.0 as usize) else {
            return Err("interpreter witness refers to an unknown type parameter".into());
        };
        if witnesses.insert(*parameter, index).is_some() {
            return Err(format!(
                "interpreter type parameter {} has more than one TypeOf witness",
                name.value
            ));
        }
    }
    for (index, parameter) in type_parameters.iter().enumerate() {
        if !witnesses.contains_key(&TypeParameterId(index as u32)) {
            return Err(format!(
                "interpreter type parameter {} has no TypeOf witness",
                parameter.value
            ));
        }
    }

    let TypeDescriptor::Function {
        parameters: inner_parameters,
        result,
    } = outer_result.as_ref()
    else {
        return Err("interpreter TypeOf witnesses must return an inner Function".into());
    };
    let interpreted = witnesses.keys().copied().collect::<HashSet<_>>();
    for (index, parameter) in inner_parameters.iter().enumerate() {
        if let TypeDescriptor::Bound(bound) = parameter
            && interpreted.contains(bound)
        {
            continue;
        }
        let mut mentioned = Vec::new();
        collect_bound_parameters(parameter, &mut mentioned);
        if let Some(bound) = mentioned
            .into_iter()
            .find(|bound| interpreted.contains(bound))
        {
            let name = &type_parameters[bound.0 as usize].value;
            return Err(format!(
                "interpreter inner parameter {} contains type parameter {}; only a direct {} parameter can be lifted",
                index + 1,
                name,
                name
            ));
        }
    }
    let mut result_parameters = Vec::new();
    collect_bound_parameters(result, &mut result_parameters);
    if let Some(bound) = result_parameters
        .into_iter()
        .find(|bound| interpreted.contains(bound))
    {
        return Err(format!(
            "interpreter result contains type parameter {}; lifted interpreters cannot return interpreted values",
            type_parameters[bound.0 as usize].value
        ));
    }
    Ok(())
}

pub(crate) fn infer_value_ref(value: ValueRef<'_>) -> Option<TypeDescriptor> {
    infer_value_ref_with(value, &mut HashSet::new())
}

fn infer_value_ref_with(
    value: ValueRef<'_>,
    visiting_type_slots: &mut HashSet<Handle>,
) -> Option<TypeDescriptor> {
    if let Some(handle) = value.hidden_type_slot_handle() {
        if !visiting_type_slots.insert(handle) {
            return None;
        }
        let inferred = value
            .resolve_hidden_type_slot()
            .ok()
            .and_then(|resolved| infer_value_ref_with(resolved, visiting_type_slots));
        visiting_type_slots.remove(&handle);
        return inferred;
    }
    if let Some((owner, _)) = value.declared_value_parts() {
        return decode_type_ref(owner, "declared value owner").ok();
    }
    Some(match value.kind() {
        ValueKind::Int => TypeDescriptor::Int,
        ValueKind::Float => TypeDescriptor::Float,
        ValueKind::String => TypeDescriptor::String,
        ValueKind::Bytes => TypeDescriptor::Bytes,
        ValueKind::Type => TypeDescriptor::TypeOf(Box::new(
            decode_type_ref(value, "Type").ok()?,
        )),
        ValueKind::Opaque => value
            .opaque_native_type()
            .cloned()
            .map(TypeDescriptor::Opaque)?,
        ValueKind::Atom | ValueKind::Tagged => return None,
        ValueKind::Array => {
            let items = (0..value.sequence_len().unwrap_or_default())
                .filter_map(|index| value.sequence_get(index))
                .map(|item| infer_value_ref_with(item, visiting_type_slots))
                .collect::<Option<Vec<_>>>()?;
            let item = if items.is_empty() { TypeDescriptor::Never } else { common_type(items)? };
            TypeDescriptor::Array(Box::new(item))
        }
        ValueKind::Tuple => TypeDescriptor::Tuple(
            (0..value.sequence_len().unwrap_or_default())
                .filter_map(|index| value.sequence_get(index))
                .map(|item| infer_value_ref_with(item, visiting_type_slots))
                .collect::<Option<Vec<_>>>()?,
        ),
        ValueKind::Dict => TypeDescriptor::Struct(
            value
                .dict_fields()
                .unwrap_or_default()
                .into_iter()
                .map(|name| {
                    Some((name.to_owned(), infer_value_ref_with(value.dict_get(name)?, visiting_type_slots)?))
                })
                .collect::<Option<BTreeMap<_, _>>>()?,
        ),
        ValueKind::Func => return None,
        ValueKind::Dyn => TypeDescriptor::Dyn,
        ValueKind::Module => TypeDescriptor::Struct(
            value
                .module_fields()
                .unwrap_or_default()
                .into_iter()
                .map(|name| {
                    Some((name.to_owned(), infer_value_ref_with(value.module_get(name)?, visiting_type_slots)?))
                })
                .collect::<Option<BTreeMap<_, _>>>()?,
        ),
    })
}

fn declare_metadata_value(
    source_name: &str,
    module_id: crate::ModuleId,
    binding: &Binding,
    slots: &HashMap<crate::Location, u32>,
    value: Val,
    evaluator: &mut ToolEvaluator,
) -> Result<Val, FrontendError> {
    if binding.value.declared_initializer.is_none() {
        return Ok(value);
    }
    validate_declared_metadata(source_name, binding, value, evaluator)?;
    let slot = slots
        .get(&binding.value.name.location)
        .copied()
        .expect("direct declared initializer has a declaration slot");
    evaluator
        .work
        .declare_type(value, module_id, slot, binding.value.name.value.as_str())
        .map_err(|error| {
            frontend_error(
                source_name,
                format!("declared type construction failed: {error}"),
            )
        })
}

fn validate_declared_metadata(
    source_name: &str,
    binding: &Binding,
    value: Val,
    evaluator: &ToolEvaluator,
) -> Result<(), FrontendError> {
    let mut graph = TypeGraph::default();
    let root = graph
        .decode_persistent(
            ValueRef::work(value, &evaluator.work, evaluator.main),
            "Type",
            &mut HashMap::new(),
        )
        .map_err(|message| {
            frontend_error(
                source_name,
                format!(
                    "declared type {} produced invalid metadata: {message}",
                    binding.value.name.value
                ),
            )
        })?;
    validate_declared_graph(source_name, binding, &graph, root)
}

fn validate_declared_graph(
    source_name: &str, binding: &Binding, graph: &TypeGraph, root: AnalysisTypeId,
) -> Result<(), FrontendError> {
    let kind = binding.value.declared_initializer
        .expect("declared metadata validation requires a declared initializer");
    let valid = graph.root_model_kind(root) == Some(kind);
    if !valid {
        return Err(frontend_error(
            source_name,
            format!(
                "declared type {} initializer changed its root model kind",
                binding.value.name.value
            ),
        ));
    }
    Ok(())
}

fn is_declared_literal_construction(
    expression: &Expr,
    expected: &TypeDescriptor,
) -> bool {
    match (expected, &expression.value) {
        (TypeDescriptor::Declared(declared), _) => {
            declared_body_accepts_expression(&declared.body, expression)
        }
        (TypeDescriptor::Array(item), ExprKind::Array(items))
            if matches!(item.as_ref(), TypeDescriptor::Declared(_)) =>
        {
            items.iter().all(|item_expression| {
                !matches!(item_expression.value, ExprKind::Spread(_))
                    && is_declared_literal_construction(item_expression, item)
            })
        }
        _ => false,
    }
}

fn declared_body_accepts_expression(body: &TypeDescriptor, expression: &Expr) -> bool {
    match (body, &expression.value) {
        (TypeDescriptor::Struct(_), ExprKind::Dict(_))
        | (TypeDescriptor::Enum(_), ExprKind::Atom(_)) => true,
        (TypeDescriptor::Enum(_), ExprKind::Call { callee, .. }) => {
            matches!(callee.value, ExprKind::Atom(_))
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn infer_tool_expression_evidence(
    source_name: &str,
    expression: &Expr,
    bindings: &dyn ToolBindings,
    expected: Option<&TypeDescriptor>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
) -> Result<ToolExpressionEvidence, String> {
    let context = evaluator.inference_context.as_ref().ok_or("tool inference context is unavailable")?;
    if !matches!(expected, Some(TypeDescriptor::Function { .. })) && !context.supports_constructors {
        return Ok(ToolExpressionEvidence::default());
    }
    let mut annotations = HashMap::new();
    evaluator.inference_depth += 1;
    let annotation_result = collect_nested_annotation_types(
        source_name, expression, bindings, account, sources, evaluator, &mut annotations,
    );
    evaluator.inference_depth -= 1;
    annotation_result.map_err(|error| error.to_string())?;
    let context = evaluator.inference_context.as_ref().ok_or("tool inference context is unavailable")?;
    let inputs = HirProgram::resolve_expression(expression, Vec::new());
    let names = inputs.references().iter()
        .filter(|reference| !matches!(reference.resolution, HirResolution::Definition(_)))
        .map(|reference| reference.name.as_str()).collect::<HashSet<_>>();
    let mut environment = HashMap::new();
    let mut lexical_schemes = Vec::new();
    // Bound type parameters supplied by the metadata scheduler are lexical
    // evidence, just like the parameters supplied to the final inference pass.
    for name in names {
        if let Some(descriptor) = context.environment.get(name) {
            environment.insert(name.to_owned(), descriptor.clone());
        }
        if let Some(scheme) = context.interfaces.get(name)
            .and_then(ModuleInterface::binding_scheme)
        {
            lexical_schemes.push((name.to_owned(), scheme.clone()));
        }
        let Some(value) = bindings.get(name) else { continue; };
        if let Ok(descriptor) = evaluator.decode_type(*value, "Type") {
            environment.insert(name.to_owned(), TypeDescriptor::TypeOf(Box::new(descriptor)));
        } else if let Some(interface) = context.interfaces.get(name)
            && let Some(descriptor) = imported_static_descriptor(
                ValueRef::work(*value, &evaluator.work, evaluator.main), Some(interface),
            )
        {
            environment.insert(name.to_owned(), descriptor);
        }
    }
    let context = evaluator.inference_context.as_mut().expect("tool inference context exists");
    for descriptor in annotations.values().chain(environment.values()) {
        collect_declared_bodies(descriptor, &mut context.declared_bodies, &mut HashSet::new());
    }
    let mut inference = GenericInference::new(
        &context.schemes, &context.hir, &context.interfaces, &context.named_types,
        &annotations, &context.trait_implementations, &context.type_properties,
        &context.trait_ids, context.display_trait.clone(),
        &context.dyn_namespaces, context.builtin_tuple_available,
        Some(&context.declared_bodies), account.query_context(),
    );
    for (name, scheme) in lexical_schemes {
        inference.set_local_scheme(name, Some(scheme));
    }
    // The caller decides whether incomplete metadata may defer a failed
    // inference pass; typed tool expressions require successful evidence.
    let mut parameters = HashMap::new();
    let mut lexical_types = HashMap::new();
    let root_scheme = context.hir.definitions().iter().find(|definition| {
        definition.value.and_then(|value| context.hir.expression(value)).is_some_and(|value| value.location == expression.location)
    }).and_then(|definition| inference.scheme(&definition.name));
    if let Some(scheme) = root_scheme.as_ref().filter(|scheme| Some(&scheme.body) == expected && !scheme.parameters.is_empty())
    {
        inference.push_lexical_evidence("<tool>", scheme);
        parameters.insert(expression.location, scheme.constraints.iter().enumerate()
            .map(|(index, constraint)| {
                let name = evidence_parameter_name("<tool>", index);
                if constraint.capability == TypeCapability::RuntimeType {
                    lexical_types.insert(constraint.parameter, name.clone());
                }
                name
            }).collect());
    }
    inference.infer(expression, &environment, expected)?;
    inference.finish_type_constraints().map_err(|(_, message)| message)?;
    inference.finish_interpolations().map_err(|(_, message)| message)?;
    parameters.extend(inference.inferred_runtime_scopes.iter().map(|(location, evidence)| {
        (*location, evidence.iter().map(|evidence| evidence.name.clone()).collect())
    }));
    let runtime_slots = inference.runtime_type_evidence.iter().map(|(name, descriptor)| {
        (name.clone(), inference.variables.structure_edge(descriptor.clone()))
    }).collect::<Vec<_>>();
    let mut types = TypeGraph::default();
    let mut publication = InferencePublication::new(&inference.variables);
    let mut publish = |slot| match publication.publish(&mut types, slot,
        |slot| inference.normalize(&TypeDescriptor::Inference(slot))) {
        Ok(id) => ToolTypeRoot::Graph(id),
        Err(_) => ToolTypeRoot::Compatibility(inference.normalize(&TypeDescriptor::Inference(slot))),
    };
    let expression_types = inference.records.iter()
        .map(|(location, slot)| (*location, publish(*slot))).collect();
    let runtime_types = runtime_slots.into_iter()
        .map(|(name, slot)| (name, publish(slot))).collect();
    Ok(ToolExpressionEvidence { types, expression_types, value_constructors: inference.value_constructors,
        calls: inference.resolved_call_evidence, runtime_types,
        parameters, lexical_types, inferred_scopes: inference.inferred_runtime_scopes, families: inference.propagation_families,
        not_families: inference.not_families, members: inference.resolved_trait_members,
        interpolations: inference.resolved_interpolation_evidence })
}

fn evaluate_legacy_contract(
    source_name: &str,
    name: &str,
    contract: &Expr,
    values: &dyn ToolBindings,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<TypeDescriptor, FrontendError> {
    let metadata = evaluate_tool_expression(source_name, contract, values, account, sources, evaluator)?;
    evaluator.decode_type(metadata, "Type").map_err(|message| frontend_error(source_name,
        format!("declaration {name} has invalid contract metadata: {message}")))
}

fn evaluate_tool_expression(
    source_name: &str,
    expression: &Expr,
    bindings: &dyn ToolBindings,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
) -> Result<Val, FrontendError> {
    evaluate_tool_expression_with_debug(
        source_name,
        expression,
        bindings,
        None,
        Some(&TypeDescriptor::Type),
        account,
        sources,
        evaluator,
        true,
    )
}

fn evaluate_typed_tool_expression_silent(
    source_name: &str,
    expression: &Expr,
    bindings: &dyn ToolBindings,
    expression_descriptors: &HashMap<crate::Location, TypeDescriptor>,
    expected: Option<&TypeDescriptor>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
) -> Result<Val, FrontendError> {
    evaluate_tool_expression_with_debug(
        source_name,
        expression,
        bindings,
        Some(expression_descriptors),
        expected,
        account,
        sources,
        evaluator,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_tool_expression_with_debug(
    source_name: &str,
    expression: &Expr,
    bindings: &dyn ToolBindings,
    expression_descriptors: Option<&HashMap<crate::Location, TypeDescriptor>>,
    expected: Option<&TypeDescriptor>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
    observed: bool,
) -> Result<Val, FrontendError> {
    let evidence = match infer_tool_expression_evidence(
        source_name, expression, bindings, expected, account, sources, evaluator,
    ) {
        Ok(evidence) => Some(evidence),
        Err(message) if expression_descriptors.is_some() => {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(message, expression.location),
            ));
        }
        Err(_) => None,
    };
    let mut prepared = evidence.unwrap_or_default();
    for (location, descriptor) in expression_descriptors.into_iter().flat_map(|descriptors| descriptors.iter())
            .filter(|(location, _)| location.source == expression.location.source
                && expression.location.start <= location.start
                && location.end <= expression.location.end)
    {
        prepared.expression_types.entry(*location)
            .or_insert_with(|| ToolTypeRoot::import(&mut prepared.types, descriptor));
    }
    evaluate_prepared_tool_expression(
        source_name, expression, bindings, prepared, account, sources, evaluator, observed,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_prepared_tool_expression(
    source_name: &str,
    expression: &Expr,
    bindings: &dyn ToolBindings,
    evidence: ToolExpressionEvidence,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
    observed: bool,
) -> Result<Val, FrontendError> {
    let ToolExpressionEvidence { types, expression_types, value_constructors, calls, runtime_types,
        parameters, lexical_types, inferred_scopes, families, not_families, members, interpolations } = evidence;
    let arities = expression_types.iter().filter_map(|(location, root)|
        root.arity(&types).map(|arity| (*location, arity))).collect();
    let mut lowered = expression.clone();
    crate::elaboration::elaborate_tool_expression(&mut lowered, &calls, &arities, &parameters,
        &families, &not_families, &members, &interpolations);
    let mut bindings = ScopedToolBindings::new(bindings);
    for (name, root) in runtime_types {
        let descriptor = root.descriptor(&types).map_err(|message| frontend_error(source_name, message))?;
        let value = evaluator.descriptor(&descriptor)?;
        let mut parameters = Vec::new();
        collect_bound_parameters(&descriptor, &mut parameters);
        let value = if name.starts_with("\0type_argument:") && !parameters.is_empty() {
            let arity = parameters.iter().map(|parameter| parameter.0 as usize + 1).max().unwrap();
            evaluator.create_type_family(value, arity, None)?.0
        } else { value };
        bindings.insert(name, value);
    }
    let mut declared_value_owners = HashMap::new();
    for (location, root) in &expression_types {
        let Some(owner_root) = root.owner(&types, value_constructors.contains_key(location)) else { continue; };
        if location.source == expression.location.source
            && expression.location.start <= location.start
            && location.end <= expression.location.end
        {
            let descriptor = owner_root.descriptor(&types).map_err(|message| frontend_error(source_name, message))?;
            let descriptor = descriptor.as_ref();
            let key = if type_identity_is_symbolic(descriptor) {
                format!("\0owner-family:{}:{}", location.start, location.end)
            } else { crate::compiler::declared_owner_link_key(*location) };
            let mut owner = ResolvedEvidence::root(key.clone());
            let mut parameters = Vec::new();
            collect_bound_parameters(descriptor, &mut parameters);
            parameters.sort_by_key(|parameter| parameter.0);
            parameters.dedup();
            let mut replacements = HashMap::new();
            for (index, parameter) in parameters.iter().enumerate() {
                let inferred = inferred_scopes.iter().filter(|(scope, _)| {
                    scope.source == location.source && scope.start <= location.start && location.end <= scope.end
                }).filter_map(|(scope, evidence)| evidence.iter().find(|evidence| evidence.target == TypeDescriptor::Bound(*parameter))
                    .map(|evidence| (scope.end - scope.start, &evidence.name)))
                    .min_by_key(|(length, _)| *length).map(|(_, name)| name);
                let Some(name) = inferred.or_else(|| lexical_types.get(parameter)) else { break; };
                owner.arguments.push(ResolvedEvidence::root(name.clone()));
                replacements.insert(*parameter, TypeDescriptor::Bound(TypeParameterId(index as u32)));
            }
            if owner.arguments.len() != parameters.len() || contains_type_variable(descriptor) { continue; }
            let descriptor = substitute_bound_parameters(descriptor, &replacements);
            let value = evaluator.descriptor(&descriptor)?;
            let value = if owner.arguments.is_empty() { value } else {
                evaluator.create_type_family(value, owner.arguments.len(), None)?.0
            };
            bindings.insert(key, value);
            declared_value_owners.insert(*location, owner);
        }
    }
    let (function, required) = compile_expression_with_external_bindings(
        source_name,
        "<tool-stage>",
        &lowered,
        |name| bindings.get(name).is_some(),
        declared_value_owners,
        value_constructors,
        sources.get(expression.location.source),
    )?;
    let externals = required.into_iter()
        .map(|name| {
            let value = *bindings.get(&name).expect("compiled external binding exists");
            (name, value)
        })
        .collect::<HashMap<_, _>>();
    let work = std::mem::replace(&mut evaluator.work, Heap::work_for(evaluator.main));
    let vm = if observed && evaluator.inference_depth == 0 {
        &mut evaluator.observed_vm
    } else {
        &mut evaluator.silent_vm
    };
    let root =
        match vm.execute_in_existing_work(evaluator.main, &externals, &function, work, account) {
            Ok((work, root)) => {
                evaluator.work = work;
                root
            }
            Err((work, error)) => {
                evaluator.work = work;
                return Err(frontend_error(
                    source_name,
                    format!(
                        "tool-stage evaluation failed: {}",
                        error.with_sources(sources)
                    ),
                ));
            }
        };
    Ok(root)
}
