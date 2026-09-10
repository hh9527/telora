// Templates are roots in the analysis graph. Runtime family closures are not
// inputs to application; their construction remains a separate migration step.
// This table elaborates shapes only. The original TypeSchemes retain constraints,
// which final inference checks for both type bodies and declaration contracts.
struct StaticTypeFamily {
    root: AnalysisTypeId,
    arity: usize,
    recursive_pending: bool,
    has_constraints: bool,
}

fn static_type_family_scheme(parameters: Vec<TypeParameter>, body: TypeDescriptor) -> TypeScheme {
    TypeScheme {
        body: TypeDescriptor::Function {
            parameters: parameters.iter().map(|parameter|
                TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(parameter.id)))).collect(),
            result: Box::new(TypeDescriptor::TypeOf(Box::new(body))),
        },
        parameters,
        constraints: Vec::new(),
    }
}

fn prepare_recursive_family_scheme(
    binding: &Binding,
    parameters: Vec<TypeParameter>,
    graph: &TypeGraph,
    solved: &SolvedRecursiveType,
    sources: &SourceDatabase,
) -> Result<(TypeScheme, bool), FrontendError> {
    let source_name = &sources.get(binding.location.source).name;
    validate_declared_graph(source_name, binding, graph, solved.body)?;
    let descriptor = graph.descriptor(solved.owner)
        .map_err(|message| frontend_error(source_name, message))?;
    let mut bounds = Vec::new();
    collect_bound_parameters(&descriptor, &mut bounds);
    if let Some(foreign) = bounds.iter().find(|bound| !parameters.iter().any(|parameter| parameter.id == **bound)) {
        return Err(FrontendError::from_diagnostic(sources, Diagnostic::error(
            format!("type family {} produced foreign bound parameter T{}", binding.value.name.value, foreign.0),
            binding.value.value.location)));
    }
    let rebuild_at_runtime = contains_named_type(&descriptor);
    Ok((static_type_family_scheme(parameters, descriptor), rebuild_at_runtime))
}

#[allow(clippy::too_many_arguments)]
fn elaborate_recursive_family(
    binding: &Binding, module_id: crate::ModuleId, declaration: u32,
    parameters: &[TypeParameter], hir: &HirProgram, environment: &HashMap<String, TypeDescriptor>,
    external_names: &HashSet<&str>, interfaces: &BTreeMap<String, ModuleInterface>,
    families: &mut BTreeMap<String, StaticTypeFamily>, graph: &mut TypeGraph,
) -> Option<SolvedRecursiveType> {
    if binding.value.type_parameter_bounds.iter().any(|bounds| !bounds.is_empty()) {
        return None;
    }
    let name = &binding.value.name.value;
    let arguments = parameters.iter().map(|parameter| TypeDescriptor::Bound(parameter.id)).collect::<Vec<_>>();
    let owner = graph.intern_descriptor(&TypeDescriptor::Declared(DeclaredTypeDescriptor {
        id: crate::value::DeclaredTypeId::applied(module_id, declaration, &arguments),
        name: name.clone(), body: Arc::new(TypeDescriptor::Never),
    }));
    let previous = families.insert(name.clone(), StaticTypeFamily {
        root: owner, arity: parameters.len(), recursive_pending: true,
        has_constraints: false,
    });
    let body = StaticContractScope { hir, environment, external_names, interfaces, parameters, families }
        .elaborate(&binding.value.value, graph);
    families.remove(name);
    if let Some(previous) = previous { families.insert(name.clone(), previous); }
    let body = body?;
    graph.fill_declared_body(owner, body);
    Some(SolvedRecursiveType { owner, body })
}


impl StaticTypeFamily {
    fn from_scheme(scheme: &TypeScheme, graph: &mut TypeGraph) -> Option<Self> {
        let TypeDescriptor::Function { parameters, result } = &scheme.body else {
            return None;
        };
        let TypeDescriptor::TypeOf(body) = result.as_ref() else {
            return None;
        };
        if parameters.len() != scheme.parameters.len()
            || contains_type_variable(body)
            || contains_named_type(body)
            || !parameters.iter().enumerate().all(|(index, parameter)| {
                scheme.parameters[index].id == TypeParameterId(index as u32)
                    && matches!(parameter, TypeDescriptor::TypeOf(item)
                        if **item == TypeDescriptor::Bound(TypeParameterId(index as u32)))
            })
        {
            return None;
        }
        let mut bounds = Vec::new();
        collect_bound_parameters(body, &mut bounds);
        if bounds
            .iter()
            .any(|bound| bound.0 as usize >= parameters.len())
        {
            return None;
        }
        Some(Self {
            root: graph.intern_descriptor(body),
            arity: parameters.len(),
            recursive_pending: false,
            has_constraints: !scheme.constraints.is_empty(),
        })
    }
}

fn static_type_families(
    graph: &mut TypeGraph,
    interfaces: &BTreeMap<String, ModuleInterface>,
) -> BTreeMap<String, StaticTypeFamily> {
    fn namespace(
        prefix: &str,
        interface: &ModuleInterface,
        graph: &mut TypeGraph,
        families: &mut BTreeMap<String, StaticTypeFamily>,
    ) {
        for name in &interface.type_declarations {
            if let Some(family) = interface
                .exports
                .get(name)
                .and_then(|scheme| StaticTypeFamily::from_scheme(scheme, graph))
            {
                let key = if interface.value_binding.as_ref() == Some(name) {
                    prefix.to_owned()
                } else {
                    format!("{prefix}.{name}")
                };
                families.insert(key, family);
            }
        }
        if interface.value_binding.is_none() {
            for (name, child) in &interface.namespaces {
                namespace(&format!("{prefix}.{name}"), child, graph, families);
            }
        }
    }
    let mut families = BTreeMap::new();
    for (name, interface) in interfaces {
        namespace(name, interface, graph, &mut families);
    }
    families
}

impl TypeGraph {
    fn apply_static_family(
        &mut self,
        root: AnalysisTypeId,
        arguments: &[AnalysisTypeId],
    ) -> AnalysisTypeId {
        fn visit(
            graph: &mut TypeGraph,
            root: AnalysisTypeId,
            arguments: &[AnalysisTypeId],
            descriptors: &[TypeDescriptor],
            mapped: &mut HashMap<AnalysisTypeId, AnalysisTypeId>,
        ) -> AnalysisTypeId {
            if let Some(result) = mapped.get(&root) {
                return *result;
            }
            let node = match graph.node(root).clone() {
                TypeNode::Bound(parameter) => return arguments[parameter.0 as usize],
                TypeNode::Ref(target) => {
                    return visit(graph, target, arguments, descriptors, mapped);
                }
                TypeNode::Declared { id, name, body } => {
                    let applied = apply_declared_type_arguments(&id, descriptors);
                    if applied.constructor() == unchecked_type_constructor() {
                        let result = graph.intern_descriptor(&unchecked_descriptor(
                            applied.arguments()[0].clone(),
                        ));
                        mapped.insert(root, result);
                        return result;
                    }
                    if let Some(existing) = graph.declared.get(&applied).copied() {
                        mapped.insert(root, existing);
                        if let TypeNode::Declared { body: previous, .. } = graph.node(existing)
                            && matches!(graph.node(*previous), TypeNode::Never)
                            && !matches!(graph.node(body), TypeNode::Never)
                        {
                            let body = visit(graph, body, arguments, descriptors, mapped);
                            graph.nodes[existing.index()] = TypeNode::Declared {
                                id: applied,
                                name,
                                body,
                            };
                        }
                        return existing;
                    }
                    // Reserve nominal identity before following the body so
                    // recursive edges point to this same application node.
                    let result = graph.push(TypeNode::Pending);
                    graph.declared.insert(applied.clone(), result);
                    mapped.insert(root, result);
                    let body = visit(graph, body, arguments, descriptors, mapped);
                    graph.nodes[result.index()] = TypeNode::Declared {
                        id: applied,
                        name,
                        body,
                    };
                    graph.record_interned_node(result);
                    return result;
                }
                TypeNode::Array(item) => {
                    TypeNode::Array(visit(graph, item, arguments, descriptors, mapped))
                }
                TypeNode::Dict(item) => {
                    TypeNode::Dict(visit(graph, item, arguments, descriptors, mapped))
                }
                TypeNode::TypeOf(item) => {
                    TypeNode::TypeOf(visit(graph, item, arguments, descriptors, mapped))
                }
                TypeNode::Newtype(item) => {
                    TypeNode::Newtype(visit(graph, item, arguments, descriptors, mapped))
                }
                TypeNode::Tagged { tag, payload } => TypeNode::Tagged {
                    tag,
                    payload: visit(graph, payload, arguments, descriptors, mapped),
                },
                TypeNode::Tuple(items) => TypeNode::Tuple(
                    items
                        .into_iter()
                        .map(|item| visit(graph, item, arguments, descriptors, mapped))
                        .collect(),
                ),
                TypeNode::Struct(fields) => TypeNode::Struct(
                    fields
                        .into_iter()
                        .map(|(name, item)| {
                            (name, visit(graph, item, arguments, descriptors, mapped))
                        })
                        .collect(),
                ),
                TypeNode::Enum(fields) => TypeNode::Enum(
                    fields
                        .into_iter()
                        .map(|(name, item)| {
                            (
                                name,
                                item.map(|item| visit(graph, item, arguments, descriptors, mapped)),
                            )
                        })
                        .collect(),
                ),
                TypeNode::PendingAlternatives(items) => TypeNode::PendingAlternatives(
                    items
                        .into_iter()
                        .map(|item| visit(graph, item, arguments, descriptors, mapped))
                        .collect(),
                ),
                TypeNode::Function { parameters, result } => TypeNode::Function {
                    parameters: parameters
                        .into_iter()
                        .map(|item| visit(graph, item, arguments, descriptors, mapped))
                        .collect(),
                    result: visit(graph, result, arguments, descriptors, mapped),
                },
                node => node,
            };
            let result = graph.intern_node(node);
            mapped.insert(root, result);
            result
        }
        // Nominal identities still use descriptor arguments at the existing
        // identity adapter. Structural substitution itself follows graph IDs.
        let descriptors = arguments
            .iter()
            .map(|argument| {
                self.descriptor(*argument)
                    .expect("static family arguments are resolved types")
            })
            .collect::<Vec<_>>();
        visit(self, root, arguments, &descriptors, &mut HashMap::new())
    }
}

fn static_contract_parameters(
    binding: &Binding,
    sources: &SourceDatabase,
) -> Result<Vec<TypeParameter>, FrontendError> {
    let mut names = HashSet::new();
    binding
        .value
        .type_parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            if !names.insert(&parameter.value) {
                return Err(FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!("duplicate type parameter {:?}", parameter.value),
                        parameter.location,
                    ),
                ));
            }
            Ok(TypeParameter {
                id: TypeParameterId(index as u32),
                name: parameter.value.clone(),
                location: parameter.location,
            })
        })
        .collect()
}
