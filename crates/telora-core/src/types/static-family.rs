// Templates are roots in the analysis graph. Runtime family closures are not
// inputs to application; their construction remains a separate migration step.
struct StaticTypeFamily {
    root: AnalysisTypeId,
    arity: usize,
    recursive_pending: bool,
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
    });
    let body = StaticContractScope { hir, environment, external_names, interfaces, parameters, families }
        .elaborate(&binding.value.value, graph);
    families.remove(name);
    if let Some(previous) = previous { families.insert(name.clone(), previous); }
    let body = body?;
    graph.fill_declared_body(owner, body);
    Some(SolvedRecursiveType { owner, body })
}

#[cfg(test)]
mod static_family_tests {
    use super::*;

    #[test]
    fn recursive_family_definition_requires_no_execution_fuel() {
        for source in [
            "type Tree(T) = struct {value: T, children: Array(Tree(T))}; type IntTree = Tree(Int); type StringTree = Tree(String);",
            "type Chain(T) = struct {children: Array(Chain(T))}; type IntTree = Chain(Int); type StringTree = Chain(String);",
        ] {
            let analysis = analyze_source_with_fuel("static-recursive-family", source, 0).unwrap();
            assert_ne!(analysis.binding_types["IntTree"], analysis.binding_types["StringTree"],
                "recursive applications retain argument identity, including phantom arguments");
        }
    }

    #[test]
    fn recursive_family_still_rejects_changed_self_arguments() {
        for source in [
            "type Tree(T) = struct {children: Array(Tree(Array(T)))};",
            "type Pair(A, B) = struct {children: Array(Pair(B, A))};",
        ] {
            let error = analyze_source("recursive-family-arguments", source).unwrap_err();
            assert!(error.message.contains("unchanged and in declaration order"), "{error}");
        }
    }

    #[test]
    fn contracts_apply_family_templates_without_runtime_inputs() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("contract", "type Pair(T) = (T, T); export def f: Fn(Pair(Int), Pair(String)) -> () = fn(a, b) { () };");
        let program = parse_registered(&sources, source).program.unwrap();
        let binding = &program.value.body.value.bindings[1];
        let environment = BootstrapPrelude::new().types;
        let hir = HirProgram::resolve(&program, environment.keys().cloned());
        let parameter = TypeDescriptor::Bound(TypeParameterId(0));
        let mut scheme = TypeScheme {
            parameters: vec![TypeParameter {
                id: TypeParameterId(0),
                name: "T".into(),
                location: binding.location,
            }],
            constraints: Vec::new(),
            body: TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::TypeOf(Box::new(parameter.clone()))],
                result: Box::new(TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Tuple(
                    vec![parameter.clone(), parameter],
                )))),
            },
        };
        let mut graph = TypeGraph::default();
        let families = BTreeMap::from([(
            "Pair".into(),
            StaticTypeFamily::from_scheme(&scheme, &mut graph).unwrap(),
        )]);
        let scope = StaticContractScope {
            hir: &hir,
            environment: &environment,
            parameters: &[],
            interfaces: &BTreeMap::new(),
            external_names: &HashSet::new(),
            families: &families,
        };
        let root = scope
            .elaborate(binding.value.annotation.as_ref().unwrap(), &mut graph)
            .unwrap();
        assert_eq!(
            graph.descriptor(root).unwrap(),
            TypeDescriptor::Function {
                parameters: vec![
                    TypeDescriptor::Tuple(vec![TypeDescriptor::Int, TypeDescriptor::Int]),
                    TypeDescriptor::Tuple(vec![TypeDescriptor::String, TypeDescriptor::String])
                ],
                result: Box::new(TypeDescriptor::Tuple(Vec::new())),
            }
        );
        scheme.constraints.push(TypeConstraint {
            parameter: TypeParameterId(0),
            capability: TypeCapability::RuntimeType,
            location: binding.location,
        });
        assert!(
            StaticTypeFamily::from_scheme(&scheme, &mut graph).is_none(),
            "constraints must not be silently discarded"
        );
    }

    #[test]
    fn applications_keep_binders_independent_and_share_equal_results() {
        let mut graph = TypeGraph::default();
        let a = graph.intern_node(TypeNode::Bound(TypeParameterId(0)));
        let b = graph.intern_node(TypeNode::Bound(TypeParameterId(1)));
        let pair = graph.intern_node(TypeNode::Tuple(vec![a, b, a]));
        let swapped = graph.apply_static_family(pair, &[b, a]);
        assert!(matches!(graph.node(swapped), TypeNode::Tuple(items) if items == &[b, a, b]));
        let int = graph.intern_node(TypeNode::Int);
        let string = graph.intern_node(TypeNode::String);
        let applied = graph.apply_static_family(pair, &[int, string]);
        assert!(
            matches!(graph.node(applied), TypeNode::Tuple(items) if items == &[int, string, int])
        );
        assert_eq!(graph.apply_static_family(pair, &[int, string]), applied);
        assert!(matches!(graph.node(pair), TypeNode::Tuple(items) if items == &[a, b, a]));
    }

    #[test]
    fn recursive_phantom_family_preserves_application_identity() {
        let mut graph = TypeGraph::default();
        let symbolic = crate::value::DeclaredTypeId::applied(
            crate::ModuleId::ANONYMOUS,
            47,
            &[TypeDescriptor::Bound(TypeParameterId(0))],
        );
        let stub = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: symbolic.clone(),
            name: "Chain".into(),
            body: Arc::new(TypeDescriptor::Never),
        });
        let template = graph.intern_descriptor(&TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: symbolic.clone(),
            name: "Chain".into(),
            body: Arc::new(TypeDescriptor::Struct(BTreeMap::from([(
                "next".into(),
                TypeDescriptor::Array(Box::new(stub)),
            )]))),
        }));
        let int = graph.intern_node(TypeNode::Int);
        let string = graph.intern_node(TypeNode::String);
        let int_chain = graph.apply_static_family(template, &[int]);
        let string_chain = graph.apply_static_family(template, &[string]);
        assert_ne!(
            int_chain, string_chain,
            "phantom arguments are part of nominal identity"
        );
        assert_eq!(graph.apply_static_family(template, &[int]), int_chain);
        let TypeNode::Declared { id, body, .. } = graph.node(int_chain) else {
            panic!("nominal root");
        };
        assert_eq!(id, &symbolic.reapply(&[TypeDescriptor::Int]));
        let TypeNode::Struct(fields) = graph.node(*body) else {
            panic!("struct body");
        };
        assert!(matches!(graph.node(fields["next"]), TypeNode::Array(next) if *next == int_chain));
        assert!(graph.descriptor(int_chain).is_ok());
    }

    #[test]
    fn application_refines_an_existing_nominal_stub() {
        let mut graph = TypeGraph::default();
        let identity = crate::value::DeclaredTypeId::applied(
            crate::ModuleId::ANONYMOUS,
            48,
            &[TypeDescriptor::Bound(TypeParameterId(0))],
        );
        let template = graph.intern_descriptor(&TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: identity.clone(),
            name: "Box".into(),
            body: Arc::new(TypeDescriptor::Struct(BTreeMap::from([(
                "item".into(),
                TypeDescriptor::Bound(TypeParameterId(0)),
            )]))),
        }));
        let stub = graph.intern_descriptor(&TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: identity.reapply(&[TypeDescriptor::Int]),
            name: "Box".into(),
            body: Arc::new(TypeDescriptor::Never),
        }));
        let int = graph.intern_node(TypeNode::Int);
        assert_eq!(graph.apply_static_family(template, &[int]), stub);
        let TypeNode::Declared { body, .. } = graph.node(stub) else {
            panic!("nominal root");
        };
        assert!(matches!(graph.node(*body), TypeNode::Struct(fields) if fields["item"] == int));
    }
}

impl StaticTypeFamily {
    fn from_scheme(scheme: &TypeScheme, graph: &mut TypeGraph) -> Option<Self> {
        if !scheme.constraints.is_empty() {
            return None;
        }
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
        })
    }
}

fn static_type_families(
    graph: &mut TypeGraph,
    local: &BTreeMap<String, TypeFamilyTemplate>,
    schemes: &HashMap<String, TypeScheme>,
    interfaces: &BTreeMap<String, ModuleInterface>,
) -> BTreeMap<String, StaticTypeFamily> {
    fn namespace(
        prefix: &str,
        interface: &ModuleInterface,
        graph: &mut TypeGraph,
        families: &mut BTreeMap<String, StaticTypeFamily>,
    ) {
        for name in interface.type_family_templates.keys() {
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
    for name in local.keys() {
        if let Some(family) = schemes
            .get(name)
            .and_then(|scheme| StaticTypeFamily::from_scheme(scheme, graph))
        {
            families.insert(name.clone(), family);
        }
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
