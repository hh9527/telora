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
    solved: &SolvedRecursiveType, graph: &TypeGraph,
    binding: &Binding, source_name: &str, store: &mut TypeStore,
) -> Result<TypeDescriptor, FrontendError> {
    let invalid = |message| frontend_error(source_name, format!(
        "type {} produced invalid metadata: {message}", binding.value.name.value));
    let root = solved.owner;
    let descriptor = graph.descriptor(root).map_err(invalid)?;
    graph.canonicalize(root, store).map_err(|message| frontend_error(source_name, message))?;
    Ok(descriptor)
}

// Establish the nominal graph identity before runtime materialization.
#[allow(clippy::too_many_arguments)]
fn prepare_static_declaration(
    root: AnalysisTypeId, graph: &mut TypeGraph, binding: &Binding, module_id: crate::ModuleId,
    slots: &HashMap<crate::Location, u32>, source_name: &str, type_store: &mut TypeStore,
) -> Result<AnalysisTypeId, FrontendError> {
    let root = if binding.value.declared_initializer.is_some() {
        graph.intern_declared_body(
            crate::value::DeclaredTypeId::concrete(module_id, slots[&binding.value.name.location]),
            binding.value.name.value.clone(), root)
    } else { root };
    graph.canonicalize(root, type_store).map_err(|message| frontend_error(source_name, message))?;
    Ok(root)
}

// Metadata is currently still consumed by legacy family/value preparation.
// A solved body enters that boundary directly; it is never executed or decoded.
#[allow(clippy::too_many_arguments)]
fn materialize_type_body(
    root: AnalysisTypeId, graph: &TypeGraph, binding: &Binding,
    source_name: &str, values: &dyn ToolBindings, evaluator: &mut ToolEvaluator<'_>,
) -> Result<(Val, TypeDescriptor), FrontendError> {
    let descriptor = graph.descriptor(root).map_err(|message| frontend_error(source_name, message))?;
    Ok((evaluator.descriptor_with_origins(&descriptor, &binding.value.value, values)?, descriptor))
}

// Native identities originate in the builtin inventory, never in a heap value.
fn native_type_contract(name: &str, interfaces: &BTreeMap<String, ModuleInterface>) -> Result<TypeDescriptor, String> {
    let scheme = interfaces.get(name).and_then(ModuleInterface::binding_scheme)
        .ok_or_else(|| format!("native type {name} requires an explicit static type interface"))?;
    if scheme.parameters.is_empty() && scheme.constraints.is_empty()
        && let TypeDescriptor::TypeOf(instance) = &scheme.body
        && matches!(instance.as_ref(), TypeDescriptor::Opaque(_))
    {
        return Ok(instance.as_ref().clone());
    }
    Err(format!("native type {name} requires a concrete opaque type interface"))
}

#[cfg(test)]
mod native_contract_tests {
    use super::*;

    #[test]
    fn native_contracts_require_static_opaque_identity_without_runtime_resources() {
        let native = crate::NativeType::bind(crate::value::NativeTypeId {
            module: crate::value::NativeModuleId(1024), local: 7,
        }, "host:fixture#Item");
        let expected = TypeDescriptor::Opaque(native);
        let mut interfaces = BTreeMap::new();
        assert!(native_type_contract("Item", &interfaces).unwrap_err().contains("explicit static type interface"));
        interfaces.insert("Item".into(), ModuleInterface {
            value_binding: Some("Item".into()),
            exports: BTreeMap::from([("Item".into(), TypeScheme {
                parameters: Vec::new(), constraints: Vec::new(),
                body: TypeDescriptor::TypeOf(Box::new(expected.clone())),
            })]),
            ..Default::default()
        });
        assert_eq!(native_type_contract("Item", &interfaces).unwrap(), expected);
        interfaces.get_mut("Item").unwrap().exports.get_mut("Item").unwrap().body =
            TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Int));
        assert!(native_type_contract("Item", &interfaces).unwrap_err().contains("concrete opaque type interface"));
    }
}

// Namespace/value identity is an interface fact, including an empty namespace.
fn imported_binding_contract(name: &str, interfaces: &BTreeMap<String, ModuleInterface>) -> Option<TypeScheme> {
    interfaces.get(name).and_then(ModuleInterface::binding_scheme).cloned()
        .or_else(|| interfaces.values().flat_map(|interface| &interface.trait_implementations)
            .find(|implementation| implementation.dictionary == name)
            .map(|implementation| implementation.dictionary_scheme.clone()))
        .or_else(|| interfaces.values().flat_map(|interface| &interface.type_properties)
            .find(|property| property.root == name).map(|property| TypeScheme {
                parameters: Vec::new(), constraints: Vec::new(), body: property.property.clone(),
            }))
}

// A malformed selected binding cannot be treated as a namespace or guessed from
// its runtime value.
fn imported_interface_descriptor(interface: &ModuleInterface) -> Option<TypeDescriptor> {
    if interface.value_binding.is_some() {
        return interface.binding_scheme().map(|scheme| scheme.body.clone());
    }
    Some(interface_descriptor(interface))
}

#[cfg(test)]
mod import_contract_tests {
    use super::*;

    #[test]
    fn import_shape_comes_from_namespace_or_selected_contract_without_values() {
        let mut interface = ModuleInterface::default();
        assert_eq!(imported_interface_descriptor(&interface), Some(TypeDescriptor::Struct(BTreeMap::new())));
        interface.exports.insert("answer".into(), TypeScheme {
            parameters: Vec::new(), constraints: Vec::new(), body: TypeDescriptor::Int,
        });
        assert_eq!(imported_interface_descriptor(&interface), Some(TypeDescriptor::Struct(BTreeMap::from([("answer".into(), TypeDescriptor::Int)]))));
        interface.value_binding = Some("answer".into());
        assert_eq!(imported_interface_descriptor(&interface), Some(TypeDescriptor::Int));
        interface.value_binding = Some("missing".into());
        assert_eq!(imported_interface_descriptor(&interface), None);
    }
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

#[cfg(test)]
mod pure_tool_solver_tests {
    use super::*;

    #[test]
    fn scalar_tool_expressions_require_solved_evidence_without_constructors() {
        for (text, valid) in [("1 + 2", true), ("1 / 0", true), ("1 + \"wrong\"", false)] {
            let mut sources = SourceDatabase::default();
            let source = sources.add("scalar-tool-types", text);
            let program = parse_registered(&sources, source).program.unwrap();
            let prelude = BootstrapPrelude::new();
            let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
            let mut context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
                prelude.schemes, BTreeMap::new(), true);
            let expression = &program.value.body.value.result;
            for expected in [None, Some(&TypeDescriptor::Int)] {
                let result = solve_tool_expression_types(expression, expected, None, &sources, &mut context);
                assert_eq!(result.is_ok(), valid, "{text}: {:?}", result.as_ref().err());
                if let Ok(evidence) = result {
                    assert_eq!(evidence.expression_types[&expression.location]
                        .descriptor(&context.types).unwrap(), TypeDescriptor::Int);
                }
            }
        }
    }

    #[test]
    fn tool_function_types_are_solved_without_runtime_resources() {
        let expected = TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Int], result: Box::new(TypeDescriptor::Int),
        };
        for (text, valid) in [("fn(x) { x + 1 }", true), ("fn(x) { identity@[Int](x) }", true),
            ("fn(x) { 1 / 0 }", true), ("fn(x) { x + \"wrong\" }", false)] {
            let mut sources = SourceDatabase::default();
            let source = sources.add("pure-tool-types", text);
            let parsed = parse_registered(&sources, source);
            let program = parsed.program.unwrap();
            let mut prelude = BootstrapPrelude::new();
            let parameter = TypeParameterId(0);
            let body = TypeDescriptor::Function { parameters: vec![TypeDescriptor::Bound(parameter)],
                result: Box::new(TypeDescriptor::Bound(parameter)) };
            prelude.types.insert("identity".into(), body.clone());
            prelude.schemes.insert("identity".into(), TypeScheme {
                parameters: vec![TypeParameter { id: parameter, name: "T".into(), location: program.location }],
                constraints: vec![TypeConstraint { parameter, capability: TypeCapability::RuntimeType,
                    location: program.location }], body,
            });
            let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
            let mut context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
                prelude.schemes, BTreeMap::new(), true);
            let expression = &program.value.body.value.result;
            let result = solve_tool_expression_types(expression, Some(&expected), None, &sources, &mut context);
            assert_eq!(result.is_ok(), valid, "{text}: {:?}", result.as_ref().err());
            if let Ok(evidence) = result {
                let root = &evidence.expression_types[&expression.location];
                assert_eq!(root.arity(&context.types), Some(1));
                assert_eq!(root.descriptor(&context.types).unwrap(), expected);
                if text.contains("identity") {
                    assert!(!evidence.runtime_types.is_empty());
                    for id in evidence.runtime_types.values() {
                        assert_eq!(context.types.node(*id), &TypeNode::Int);
                    }
                }
            }
        }
    }
}

fn tool_external_names(
    expression: &Expr,
    inputs: &HirProgram,
    context: &ToolInferenceContext<'_>,
) -> HashSet<String> {
    let names = inputs.references().iter()
        .filter(|reference| !matches!(reference.resolution, HirResolution::Definition(_)))
        .map(|reference| reference.name.as_str()).collect::<HashSet<_>>();
    let mut external_names = names.iter().filter(|name| context.environment.contains_key(**name)
        || context.schemes.contains_key(**name) || context.interfaces.contains_key(**name))
        .map(|name| (*name).to_owned()).collect::<HashSet<_>>();
    for reference in inputs.references() {
        if let Some(HirResolution::Definition(id)) = context.hir.reference_at(reference.location, &reference.name)
            .map(|reference| reference.resolution)
            && context.hir.definition(id).is_some_and(|definition| definition.top_level) {
            external_names.insert(reference.name.clone());
        }
    }
    // Unit constructors in patterns are not ordinary expression references.
    external_names.extend(context.interfaces.iter().filter(|(name, interface)|
        interface.value_binding.as_deref() == Some(name.as_str())
            && interface.member_constructors.contains_key(*name)).map(|(name, _)| name.clone()));
    for definition in context.hir.definitions() {
        if let Some(value) = definition.value.and_then(|id| context.hir.expression(id))
            && value.location.source == expression.location.source
            && value.location.start <= expression.location.start && expression.location.end <= value.location.end {
            external_names.extend(definition.type_parameters.iter().filter(|parameter| names.contains(parameter.name.as_str()))
                .map(|parameter| parameter.name.clone()));
        }
    }
    external_names
}

// Static preparation has no access to runtime values, heaps or an evaluator.
fn solve_tool_expression_types(
    expression: &Expr,
    expected: Option<&TypeDescriptor>,
    query: Option<crate::query::QueryContext>,
    sources: &SourceDatabase,
    context: &mut ToolInferenceContext<'_>,
) -> Result<ToolExpressionEvidence, String> {
    let inputs = HirProgram::resolve_expression(expression, Vec::new());
    let external_names = tool_external_names(expression, &inputs, context);
    let names = inputs.references().iter()
        .filter(|reference| !matches!(reference.resolution, HirResolution::Definition(_)))
        .map(|reference| reference.name.as_str()).collect::<HashSet<_>>();
    let mut annotations = HashMap::new();
    let mut types = std::mem::take(&mut context.types);
    let annotations_result = collect_tool_annotations(expression, context, sources, &mut annotations, &mut types);
    context.types = types;
    annotations_result.map_err(|error| error.to_string())?;
    let mut environment = HashMap::new();
    let mut lexical_schemes = Vec::new();
    // Reference types and imported schemes come from the static context. Runtime
    // metadata values cannot refine or override the solver's input environment.
    for name in names {
        if let Some(descriptor) = context.environment.get(name) {
            environment.insert(name.to_owned(), descriptor.clone());
        }
        if let Some(scheme) = context.interfaces.get(name)
            .and_then(ModuleInterface::binding_scheme)
        {
            lexical_schemes.push((name.to_owned(), scheme.clone()));
        }
        if let Some(interface) = context.interfaces.get(name) {
            environment.insert(name.to_owned(), interface.binding_scheme()
                .map_or_else(|| interface_descriptor(interface), |scheme| scheme.body.clone()));
        }
    }
    for descriptor in environment.values() {
        collect_declared_bodies(descriptor, &mut context.declared_bodies, &mut HashSet::new());
    }
    let annotation_inputs = InferenceAnnotationInputs::from_graph(&context.types, &annotations, sources)
        .map_err(|error| error.to_string())?;
    let mut inference = GenericInference::new(
        &context.schemes, &context.hir, &context.interfaces, &context.named_types,
        annotation_inputs, &context.trait_implementations, &context.type_properties,
        &context.trait_ids, context.display_trait.clone(),
        context.builtin_tuple_available,
        Some(&context.declared_bodies), query,
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
    let mut publication = InferencePublication::new(&inference.variables);
    let expression_types = inference.records.iter()
        .map(|(location, slot)| (*location, publication.publish_tool_root(&mut context.types, *slot,
            |slot| inference.normalize(&TypeDescriptor::Inference(slot))))).collect();
    let runtime_types = runtime_slots.into_iter()
        .map(|(name, slot)| publication.publish(&mut context.types, slot,
            |slot| inference.normalize(&TypeDescriptor::Inference(slot)))
            .map(|id| (name.clone(), id))
            .map_err(|_| format!("runtime type evidence {name:?} has no solved static type")))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    Ok(ToolExpressionEvidence { external_names, expression_types, value_constructors: inference.value_constructors,
        calls: inference.resolved_call_evidence, runtime_types,
        parameters, lexical_types, inferred_scopes: inference.inferred_runtime_scopes, families: inference.propagation_families,
        not_families: inference.not_families, members: inference.resolved_trait_members,
        interpolations: inference.resolved_interpolation_evidence })
}

#[allow(clippy::too_many_arguments)]
fn evaluate_prepared_tool_expression(
    source_name: &str,
    bindings: &dyn ToolBindings,
    evidence: &PreparedToolExpression,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
    observed: bool,
) -> Result<Val, FrontendError> {
    let PreparedToolExpression { expression, source, function, required, runtime_types } = evidence;
    let function = function.get_or_init(|| {
        let source = sources.get(*source);
        crate::compiler::compile_prepared_external_expression(
            &source.name, "<tool-stage>", expression, required, source,
        )
    }).as_ref().map_err(Clone::clone)?;
    if let Some(name) = required.iter().find(|name| bindings.get(name).is_none()
        && !runtime_types.iter().any(|(generated, _, _)| generated == *name)) {
        return Err(frontend_error(source_name,
            format!("prepared tool expression requires unavailable binding {name:?}")));
    }
    let mut bindings = ScopedToolBindings::new(bindings);
    let values = evaluator.work.type_graph_values_in(
        Some(evaluator.main), &evaluator.tool_types, runtime_types.iter().map(|(_, root, _)| *root),
        &mut evaluator.tool_type_values,
    ).map_err(|error| frontend_error("<tool-stage>", error.to_string()))?;
    for ((name, _, arity), value) in runtime_types.iter().zip(values) {
        let value = if *arity != 0 {
            evaluator.create_type_family(value, *arity, None)?.0
        } else { value };
        bindings.insert(name.clone(), value);
    }
    let externals = required.iter()
        .map(|name| {
            let value = *bindings.get(name).ok_or_else(|| frontend_error(source_name,
                format!("prepared tool expression requires unavailable binding {name:?}")))?;
            Ok((name.clone(), value))
        })
        .collect::<Result<HashMap<_, _>, FrontendError>>()?;
    let work = std::mem::replace(&mut evaluator.work, Heap::work_for(evaluator.main));
    let vm = if observed {
        &mut evaluator.observed_vm
    } else {
        &mut evaluator.silent_vm
    };
    let root =
        match vm.execute_in_existing_work(evaluator.main, &externals, function, work, account) {
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
