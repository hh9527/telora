struct DeclaredOwnerPlan {
    key: String,
    root: AnalysisTypeId,
    arity: usize,
}

// Resolve lexical owner evidence while inference is available. Execution only
// materializes these graph roots and links the already-selected arguments.
fn prepare_declared_value_owners(
    program: &Program,
    expressions: &HashMap<crate::Location, TypeDescriptor>,
    schemes: &HashMap<String, TypeScheme>,
    inference: &GenericInference<'_>,
    graph: &mut TypeGraph,
) -> (HashMap<crate::Location, ResolvedEvidence>, Vec<DeclaredOwnerPlan>) {
    let mut owners = HashMap::new();
    let mut plans = Vec::new();
    for (location, descriptor) in expressions {
        let descriptor = if inference.value_constructors.contains_key(location)
            && let TypeDescriptor::Function { result, .. } = descriptor
        {
            result.as_ref()
        } else { descriptor };
        if !matches!(descriptor, TypeDescriptor::Declared(_)) { continue; }
        let key = if type_identity_is_symbolic(descriptor) {
            format!("\0owner-family:{}:{}", location.start, location.end)
        } else { crate::compiler::declared_owner_link_key(*location) };
        let mut owner = ResolvedEvidence::root(key.clone());
        let mut parameters = Vec::new();
        collect_bound_parameters(descriptor, &mut parameters);
        parameters.sort_by_key(|parameter| parameter.0);
        parameters.dedup();
        let descriptor = if parameters.is_empty() { descriptor.clone() } else {
            let binding = program.value.body.value.bindings.iter().find(|binding| {
                binding.value.value.location.source == location.source
                    && binding.value.value.location.start <= location.start
                    && location.end <= binding.value.value.location.end
            });
            let scheme = binding.and_then(|binding| schemes.get(&binding.value.name.value));
            let mut replacements = HashMap::new();
            for (index, parameter) in parameters.iter().enumerate() {
                let inferred = inference.inferred_runtime_scopes.iter().filter(|(scope, _)| {
                    scope.source == location.source && scope.start <= location.start && location.end <= scope.end
                }).filter_map(|(scope, evidence)| evidence.iter().find(|evidence| evidence.target == TypeDescriptor::Bound(*parameter))
                    .map(|evidence| (scope.end - scope.start, evidence.name.clone())))
                    .min_by_key(|(length, _)| *length).map(|(_, name)| name);
                let explicit = scheme.and_then(|scheme| scheme.constraints.iter().position(|constraint| {
                    constraint.parameter == *parameter && constraint.capability == TypeCapability::RuntimeType
                })).map(|index| evidence_parameter_name(&binding.unwrap().value.name.value, index));
                let implementation = binding.filter(|binding| binding.value.kind == BindingKind::Impl)
                    .and_then(|binding| binding.value.type_parameters.get(parameter.0 as usize))
                    .map(|parameter| parameter.value.clone());
                let Some(name) = inferred.or(explicit).or(implementation) else { break; };
                owner.arguments.push(ResolvedEvidence::root(name));
                replacements.insert(*parameter, TypeDescriptor::Bound(TypeParameterId(index as u32)));
            }
            if owner.arguments.len() != parameters.len() { continue; }
            substitute_bound_parameters(descriptor, &replacements)
        };
        if contains_type_variable(&descriptor) { continue; }
        plans.push(DeclaredOwnerPlan {
            key, root: graph.intern_descriptor(&descriptor), arity: owner.arguments.len(),
        });
        owners.insert(*location, owner);
    }
    (owners, plans)
}
