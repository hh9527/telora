struct ToolEvaluator<'a> {
    observed_vm: Vm,
    silent_vm: Vm,
    main: &'a mut Heap,
    work: Heap,
    inference_context: Option<ToolInferenceContext>,
    inference_depth: usize,
    registered_construction_checks: BTreeSet<PropertyKey>,
    construction_checks_complete: bool,
}

struct ToolInferenceContext {
    hir: Arc<HirProgram>,
    interfaces: BTreeMap<String, ModuleInterface>,
    environment: HashMap<String, TypeDescriptor>,
    schemes: HashMap<String, TypeScheme>,
    named_types: BTreeMap<String, TypeDescriptor>,
    builtin_tuple_available: bool,
    dyn_namespaces: HashSet<String>,
    declared_bodies: HashMap<crate::value::DeclaredTypeId, Arc<TypeDescriptor>>,
    trait_implementations: Vec<TraitImplementation>,
    type_properties: Vec<TypePropertyEvidence>,
    trait_ids: BTreeMap<String, crate::TraitId>,
    display_trait: Option<(crate::TraitId, String)>,
    supports_constructors: bool,
}

fn imported_dyn_namespaces(bindings: &[Binding]) -> HashSet<String> {
    bindings.iter().filter(|binding| {
        binding.value.kind == BindingKind::Import && binding.value.imported_name.is_none()
            && matches!(&binding.value.value.value, ExprKind::String(path) if path == "std/dyn")
    }).map(|binding| binding.value.name.value.clone()).collect()
}

#[derive(Default)]
struct ToolExpressionEvidence {
    types: TypeGraph,
    expression_types: HashMap<crate::Location, ToolTypeRoot>,
    value_constructors: HashMap<crate::Location, ValueConstructor>,
    calls: HashMap<crate::Location, Vec<ResolvedEvidence>>,
    runtime_types: BTreeMap<String, ToolTypeRoot>,
    parameters: HashMap<crate::Location, Vec<String>>,
    lexical_types: HashMap<TypeParameterId, String>,
    inferred_scopes: HashMap<crate::Location, Vec<LexicalTypeEvidence>>,
    families: HashMap<crate::Location, PropagationFamily>,
    not_families: HashMap<crate::Location, NotFamily>,
    members: HashMap<crate::Location, ResolvedEvidence>,
    interpolations: HashMap<crate::Location, ResolvedEvidence>,
}

// IDs always refer to the graph owned by the enclosing evidence. Intermediate
// tool records can be open even after a successful inference pass; keep those
// explicit until all consumers support an open graph snapshot.
enum ToolTypeRoot {
    Graph(AnalysisTypeId),
    Compatibility(TypeDescriptor),
}

impl ToolTypeRoot {
    fn import(graph: &mut TypeGraph, descriptor: &TypeDescriptor) -> Self {
        match graph.intern_resolved_descriptor(descriptor) {
            Some(id) => Self::Graph(id),
            None => Self::Compatibility(descriptor.clone()),
        }
    }

    fn descriptor<'a>(&'a self, graph: &TypeGraph) -> Result<std::borrow::Cow<'a, TypeDescriptor>, String> {
        match self {
            Self::Graph(id) => graph.descriptor(*id).map(std::borrow::Cow::Owned),
            Self::Compatibility(descriptor) => Ok(std::borrow::Cow::Borrowed(descriptor)),
        }
    }

    fn arity(&self, graph: &TypeGraph) -> Option<usize> {
        match self {
            Self::Graph(id) => match graph.node(*id) {
                TypeNode::Function { parameters, .. } => Some(parameters.len()),
                _ => None,
            },
            Self::Compatibility(TypeDescriptor::Function { parameters, .. }) => Some(parameters.len()),
            _ => None,
        }
    }

    fn owner(&self, graph: &TypeGraph, constructor: bool) -> Option<Self> {
        match self {
            Self::Graph(id) => {
                let id = match graph.node(*id) {
                    TypeNode::Function { result, .. } if constructor => *result,
                    _ => *id,
                };
                matches!(graph.node(id), TypeNode::Declared { .. }).then_some(Self::Graph(id))
            }
            Self::Compatibility(descriptor) => {
                let descriptor = match descriptor {
                    TypeDescriptor::Function { result, .. } if constructor => result.as_ref(),
                    _ => descriptor,
                };
                matches!(descriptor, TypeDescriptor::Declared(_))
                    .then(|| Self::Compatibility(descriptor.clone()))
            }
        }
    }
}

#[cfg(test)]
mod tool_type_root_tests {
    use super::*;

    #[test]
    fn evidence_owns_shared_types_after_solver_is_dropped() {
        let mut graph = TypeGraph::default();
        let (function, owner) = {
            let mut variables = InferenceVariables::default();
            let int = variables.structure_edge(TypeDescriptor::Int);
            let body = variables.structure_node(InferenceConstructor::Array, &[int]);
            let owner = variables.structure_node(InferenceConstructor::Declared {
                head: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 987),
                name: "Items".into(),
            }, &[body]);
            let function = variables.structure_node(InferenceConstructor::Function, &[owner, owner]);
            let mut publication = InferencePublication::new(&variables);
            let function = publication.publish(&mut graph, function, |_| panic!("no materialization needed")).ok().unwrap();
            let owner = publication.publish(&mut graph, owner, |_| panic!("shared root must be reused")).ok().unwrap();
            (ToolTypeRoot::Graph(function), owner)
        };
        assert_eq!(function.arity(&graph), Some(1));
        assert!(function.owner(&graph, false).is_none());
        assert!(matches!(function.owner(&graph, true), Some(ToolTypeRoot::Graph(id)) if id == owner));
        let TypeNode::Function { parameters, result } = graph.node(match function {
            ToolTypeRoot::Graph(id) => id,
            _ => unreachable!(),
        }) else { panic!("function node") };
        assert_eq!(parameters, &[owner]);
        assert_eq!(*result, owner);
        assert_eq!(graph.nodes().len(), 4);
    }

    #[test]
    fn open_function_evidence_keeps_arity_without_claiming_a_final_type() {
        let mut variables = InferenceVariables::default();
        let slot = variables.fresh();
        let descriptor = TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Inference(slot)],
            result: Box::new(TypeDescriptor::Inference(slot)),
        };
        let mut graph = TypeGraph::default();
        let root = ToolTypeRoot::import(&mut graph, &descriptor);
        assert!(matches!(root, ToolTypeRoot::Compatibility(_)));
        assert_eq!(root.arity(&graph), Some(1));
        assert!(root.owner(&graph, true).is_none());
        assert_eq!(root.descriptor(&graph).unwrap().as_ref(), &descriptor);
        assert_eq!(graph.nodes().len(), 0);
    }
}

impl ToolInferenceContext {
    fn scope_environment_inputs(
        &mut self,
        expression: &Expr,
        environment: &dyn TypeEnvironment,
    ) -> Vec<(String, Option<TypeDescriptor>)> {
        let inputs = HirProgram::resolve_expression(expression, Vec::new());
        let names = inputs.references().iter()
            .filter(|reference| !matches!(reference.resolution, HirResolution::Definition(_)))
            .map(|reference| reference.name.as_str()).collect::<BTreeSet<_>>();
        names.into_iter().map(|name| {
            let previous = match environment.get(name) {
                Some(descriptor) => self.environment.insert(name.to_owned(), descriptor.clone()),
                None => self.environment.remove(name),
            };
            (name.to_owned(), previous)
        }).collect()
    }

    fn restore_environment_inputs(&mut self, previous: Vec<(String, Option<TypeDescriptor>)>) {
        for (name, descriptor) in previous {
            match descriptor {
                Some(descriptor) => { self.environment.insert(name, descriptor); }
                None => { self.environment.remove(&name); }
            }
        }
    }

    fn new(
        hir: impl Into<Arc<HirProgram>>,
        interfaces: BTreeMap<String, ModuleInterface>,
        environment: HashMap<String, TypeDescriptor>,
        schemes: HashMap<String, TypeScheme>,
        named_types: BTreeMap<String, TypeDescriptor>,
        builtin_tuple_available: bool,
        dyn_namespaces: HashSet<String>,
    ) -> Self {
        let trait_implementations = interfaces.values()
            .flat_map(|interface| interface.trait_implementations.iter().cloned()).collect();
        let type_properties = interfaces.values()
            .flat_map(|interface| interface.type_properties.iter().cloned()).collect();
        let trait_ids = interfaces.values()
            .flat_map(|interface| interface.traits.clone()).collect();
        let display_trait = interfaces.values().find_map(|interface| interface.display_trait)
            .map(|id| (id, "std/fmt.Display".to_owned()));
        let mut context = Self {
            hir: hir.into(), interfaces, environment, schemes, named_types, builtin_tuple_available,
            dyn_namespaces, declared_bodies: HashMap::new(), trait_implementations,
            type_properties, trait_ids, display_trait, supports_constructors: false,
        };
        context.prepare_declarations();
        context
    }

    fn prepare_declarations(&mut self) {
        self.declared_bodies.clear();
        for descriptor in self.named_types.values()
            .chain(self.schemes.values().map(|scheme| &scheme.body))
            .chain(self.interfaces.values().flat_map(|interface| interface.concrete_types.values()))
            .chain(self.interfaces.values().flat_map(|interface| interface.exports.values().map(|scheme| &scheme.body)))
        {
            collect_declared_bodies(descriptor, &mut self.declared_bodies, &mut HashSet::new());
            self.supports_constructors |= tool_constructor_type(descriptor);
        }
    }

    fn publish_binding(
        &mut self,
        name: &str,
        descriptor: Option<&TypeDescriptor>,
        scheme: Option<&TypeScheme>,
        named_type: Option<&TypeDescriptor>,
    ) {
        if let Some(descriptor) = descriptor {
            self.environment.insert(name.into(), descriptor.clone());
            collect_declared_bodies(descriptor, &mut self.declared_bodies, &mut HashSet::new());
            self.supports_constructors |= tool_constructor_type(descriptor);
        } else {
            self.environment.remove(name);
        }
        if let Some(scheme) = scheme {
            self.schemes.insert(name.into(), scheme.clone());
            collect_declared_bodies(&scheme.body, &mut self.declared_bodies, &mut HashSet::new());
            self.supports_constructors |= tool_constructor_type(&scheme.body);
        } else {
            self.schemes.remove(name);
        }
        if let Some(descriptor) = named_type {
            self.named_types.insert(name.into(), descriptor.clone());
        }
    }

    fn publish_type(&mut self, name: &str, descriptor: &TypeDescriptor, scheme: Option<&TypeScheme>) {
        let (witness, body) = if let Some(scheme) = scheme {
            self.schemes.insert(name.into(), scheme.clone());
            let body = match &scheme.body {
                TypeDescriptor::Function { result, .. } => match result.as_ref() {
                    TypeDescriptor::TypeOf(body) => body.as_ref(),
                    _ => descriptor,
                },
                _ => descriptor,
            };
            (scheme.body.clone(), body.clone())
        } else {
            (TypeDescriptor::TypeOf(Box::new(descriptor.clone())), descriptor.clone())
        };
        self.publish_binding(name, Some(&witness), scheme, Some(&body));
    }
}

fn tool_constructor_type(descriptor: &TypeDescriptor) -> bool {
    let owner = constructor_instance_type(descriptor).unwrap_or(descriptor);
    let body = match owner {
        TypeDescriptor::Declared(declared) => declared.body.as_ref(),
        ty => ty,
    };
    matches!(body, TypeDescriptor::Newtype(_) | TypeDescriptor::Enum(_))
}

impl<'a> ToolEvaluator<'a> {
    fn new(debug_sink: Arc<dyn DebugSink>, main: &'a mut Heap) -> Self {
        let work = Heap::work_for(main);
        Self {
            observed_vm: Vm::new().with_debug_sink(debug_sink),
            silent_vm: Vm::new().with_debug_sink(Arc::new(DiscardDebugSink)),
            main,
            work,
            inference_context: None,
            inference_depth: 0,
            registered_construction_checks: BTreeSet::new(),
            construction_checks_complete: false,
        }
    }

    fn refresh_inference_context(
        &mut self,
        environment: &HashMap<String, TypeDescriptor>,
        schemes: &HashMap<String, TypeScheme>,
        declared_types: &BTreeMap<String, TypeDescriptor>,
    ) {
        if let Some(context) = &mut self.inference_context {
            context.environment.clone_from(environment);
            context.schemes.clone_from(schemes);
            context.named_types = context.interfaces.values()
                .flat_map(|interface| interface.concrete_types.clone())
                .chain(declared_types.clone()).collect();
            context.prepare_declarations();
        }
    }

    fn publish_inference_binding(
        &mut self,
        name: &str,
        environment: &HashMap<String, TypeDescriptor>,
        schemes: &HashMap<String, TypeScheme>,
        declared_types: &BTreeMap<String, TypeDescriptor>,
    ) {
        if let Some(context) = &mut self.inference_context {
            context.publish_binding(name, environment.get(name), schemes.get(name), declared_types.get(name));
        }
    }

    fn descriptor(&mut self, descriptor: &TypeDescriptor) -> Result<Val, FrontendError> {
        self.work
            .type_descriptor_value(Some(self.main), descriptor)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn install_bootstrap(&mut self) -> Result<BTreeMap<String, Val>, FrontendError> {
        let mut values = BTreeMap::new();
        for (name, descriptor) in [
            ("Type", TypeDescriptor::Type),
            ("Dyn", TypeDescriptor::Dyn),
            ("Never", TypeDescriptor::Never),
            ("Unit", TypeDescriptor::Tuple(Vec::new())),
            ("\0telora_unit_type", TypeDescriptor::Tuple(Vec::new())),
            ("Int", TypeDescriptor::Int),
            ("Float", TypeDescriptor::Float),
            ("String", TypeDescriptor::String),
            ("Bytes", TypeDescriptor::Bytes),
            ("PropertyTarget", property_target_descriptor()),
        ] {
            values.insert(name.into(), self.descriptor(&descriptor)?);
        }
        values.insert(
            "Bool".into(),
            self.work
                .normalized_bool_type_value(Some(self.main))
                .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?,
        );
        for function in [
            NativeFunction::core_model(CoreModelFunction::Struct),
            NativeFunction::core_model(CoreModelFunction::Newtype),
            NativeFunction::core_model(CoreModelFunction::Enum),
            NativeFunction::core_builtin_type(CoreBuiltinTypeFunction::Option),
            NativeFunction::core_builtin_type(CoreBuiltinTypeFunction::Result),
            NativeFunction::core_builtin_type(CoreBuiltinTypeFunction::FoldControl),
            NativeFunction::new("Array", 1, native_array_type),
            NativeFunction::new("Dict", 1, native_dict_type),
            NativeFunction::new("TypeOf", 1, native_type_of_type),
            NativeFunction::new("Unchecked", 1, native_unchecked_type),
            NativeFunction::new("Tuple", 1, native_tuple_type),
            NativeFunction::new("Func", 2, native_function_type),
            NativeFunction::new("\0telora_tuple_type", 1, native_tuple_type),
            NativeFunction::new("\0telora_function_type", 2, native_function_type),
            NativeFunction::checked_cast(native_checked_cast),
        ] {
            values.insert(
                function.name().into(),
                self.work
                    .native_closure(function, Vec::<Val>::new().into_boxed_slice()),
            );
        }
        let pack = NativeFunction::core_dyn(CoreDynFunction::Pack);
        values.insert(
            "\0telora_pack_dyn".into(),
            self.work
                .native_closure(pack, Vec::<Val>::new().into_boxed_slice()),
        );
        Ok(values)
    }

    fn persist_table(
        &mut self,
        entries: impl IntoIterator<Item = (String, Val)>,
    ) -> Result<PersistentValue, FrontendError> {
        let root = self
            .work
            .module(entries)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?;
        publish_root(self.main, &self.work, root)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn decode_type(&self, value: Val, path: &str) -> Result<TypeDescriptor, String> {
        decode_type_ref(ValueRef::work(value, &self.work, self.main), path)
    }

    fn declared_type_id(&self, value: Val) -> Result<TypeId, FrontendError> {
        HeapView {
            current: &self.work,
            background: Some(self.main),
        }
        .declared_type_id(value)
        .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn canonical_type_id(&self, descriptor: &TypeDescriptor) -> Result<TypeId, FrontendError> {
        self.work
            .canonical_descriptor_type_id(descriptor)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn property_attr_type(&self) -> Option<TypeId> {
        self.work
            .property_attr_type()
            .or_else(|| self.main.property_attr_type())
    }

    fn establish_property_attr_type(&mut self, type_id: TypeId) -> Result<(), FrontendError> {
        self.work
            .establish_property_attr_type(type_id)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn property_capabilities(&self, target: TypeId) -> Result<u32, crate::heap::HeapError> {
        let marker = self
            .property_attr_type()
            .ok_or(crate::heap::HeapError::new("PropertyAttr is not established"))?;
        let view = HeapView {
            current: &self.work,
            background: Some(self.main),
        };
        let value = view
            .type_property(target, marker)
            .ok_or(crate::heap::HeapError::new(
                "decorator result type is not marked with @property",
            ))?;
        let crate::heap::DecodedValue::Dict(handle) = value.value() else {
            return Err(crate::heap::HeapError::new(
                "PropertyAttr value is not a record",
            ));
        };
        let bits = view
            .dict_get_text(handle, "bits")?
            .ok_or(crate::heap::HeapError::new(
                "PropertyAttr value has no bits field",
            ))?;
        let crate::heap::DecodedValue::Int(bits) = bits.value() else {
            return Err(crate::heap::HeapError::new("PropertyAttr.bits is not Int"));
        };
        u32::try_from(bits)
            .map_err(|_| crate::heap::HeapError::new("PropertyAttr.bits is outside u32"))
    }

    fn property_attr_value(&mut self, property_type: TypeId, bits: u32) -> Val {
        self.work.property_attr_value(property_type, bits)
    }

    fn previous_property_value(&mut self, previous: Option<Val>) -> Val {
        self.work.option_value(previous)
    }

    fn stage_property(&mut self, key: PropertyKey, value: Val) -> Result<(), FrontendError> {
        self.work
            .stage_property(key, value)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn publish_type_properties(
        &mut self,
        property_attr_type: Option<TypeId>,
        properties: &[(PropertyKey, Val)],
    ) -> Result<(), FrontendError> {
        publish_type_properties(
            self.main,
            &self.work,
            property_attr_type,
            properties,
        )
        .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn persistent_type_property(
        &self,
        target: TypeId,
        property: TypeId,
    ) -> Option<PersistentValue> {
        self.main.persistent_type_property(target, property)
    }

    fn decode_type_graph(
        &self,
        value: Val,
        path: &str,
    ) -> Result<(TypeGraph, AnalysisTypeId), String> {
        let mut graph = TypeGraph::default();
        let root = graph.decode_persistent(
            ValueRef::work(value, &self.work, self.main),
            path,
            &mut HashMap::new(),
        )?;
        Ok((graph, root))
    }

    fn create_type_family(
        &mut self,
        metadata: Val,
        arity: usize,
        constructor: Option<&NominalTypeConstructor>,
    ) -> Result<(Val, PersistentValue, PersistentValue), FrontendError> {
        let arity_value = i64::try_from(arity)
            .map_err(|_| frontend_error("<tool-stage>", "type-family arity exceeds Int"))?;
        let template = publish_root(self.main, &self.work, metadata)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?;
        let module = constructor.map_or(-1, |constructor| i64::from(constructor.id.module.raw()));
        let local = constructor.map_or(0, |constructor| i64::from(constructor.id.local));
        let name = constructor.map_or("", |constructor| constructor.name.as_str());
        let work_name = Val::unknown(self.work.string(Some(self.main), name));
        let main_name = Val::unknown(self.main.string(None, name));
        let family = self.work.native_closure(
            NativeFunction::new("type-family.apply", arity, native_apply_type_family),
            vec![
                metadata,
                self.work.int(arity_value),
                self.work.int(module),
                self.work.int(local),
                work_name,
            ],
        );
        let persistent_family = self.main.native_closure(
            NativeFunction::new("type-family.apply", arity, native_apply_type_family),
            vec![
                template.runtime(),
                self.main.int(arity_value),
                self.main.int(module),
                self.main.int(local),
                main_name,
            ],
        );
        let root = self
            .main
            .persistent(persistent_family)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?;
        Ok((family, template, root))
    }

    fn reserve_recursive_type_family(
        &mut self,
        constructor: &NominalTypeConstructor,
        parameters: &[TypeParameter],
    ) -> Result<(Val, Val), FrontendError> {
        let arguments = parameters
            .iter()
            .map(|parameter| TypeDescriptor::Bound(parameter.id))
            .collect::<Vec<_>>();
        let id = crate::value::DeclaredTypeId::applied(
            constructor.id.module,
            constructor.id.local,
            &arguments,
        );
        let placeholder = self.descriptor(&TypeDescriptor::Named(constructor.name.clone()))?;
        let root = self
            .work
            .reserve_symbolic_type_ref(id, constructor.name.as_str(), placeholder)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?;
        let family = self.work.native_closure(
            NativeFunction::new(
                "recursive-type-family.apply",
                parameters.len(),
                native_apply_recursive_type_family,
            ),
            vec![root],
        );
        Ok((root, family))
    }
}

struct RecursiveTypeFamilyBuild {
    family_value: Val,
    family: TypeFamilyTemplate,
    scheme: TypeScheme,
}

#[allow(clippy::too_many_arguments)]
fn build_recursive_type_family(
    source_name: &str,
    module_id: crate::ModuleId,
    declaration: u32,
    binding: &Binding,
    base_bindings: &dyn ToolBindings,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
    solved: Option<(&TypeGraph, &SolvedRecursiveType)>,
) -> Result<RecursiveTypeFamilyBuild, FrontendError> {
    let mut evaluation_bindings = ScopedToolBindings::new(base_bindings);
    let mut parameters = Vec::new();
    let mut parameter_names = HashSet::new();
    for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
        if !parameter_names.insert(parameter.value.as_str()) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!("duplicate type parameter {:?}", parameter.value),
                    parameter.location,
                ),
            ));
        }
        let id = TypeParameterId(
            u32::try_from(index)
                .map_err(|_| frontend_error(source_name, "type family has too many parameters"))?,
        );
        parameters.push(TypeParameter {
            id,
            name: parameter.value.clone(),
            location: parameter.location,
        });
        evaluation_bindings.insert(
            parameter.value.clone(),
            evaluator.descriptor(&TypeDescriptor::Bound(id))?,
        );
    }
    let constructor = NominalTypeConstructor {
        id: crate::TypeConstructorId {
            module: module_id,
            local: declaration,
        },
        name: binding.value.name.value.clone(),
    };
    let (symbolic_root, self_family) =
        evaluator.reserve_recursive_type_family(&constructor, &parameters)?;
    evaluation_bindings.insert(binding.value.name.value.clone(), self_family);
    let body = if let Some((graph, solved)) = solved {
        validate_declared_graph(source_name, binding, graph, solved.body)?;
        materialize_type_body(Some(solved.body), graph, binding, source_name,
            &evaluation_bindings, account, sources, evaluator)?.0
    } else {
        let body = evaluate_tool_expression(source_name, &binding.value.value,
            &evaluation_bindings, account, sources, evaluator)?;
        validate_declared_metadata(source_name, binding, body, evaluator)?;
        body
    };
    evaluator
        .work
        .seal_type_ref(symbolic_root, body)
        .map_err(|error| frontend_error(source_name, error.to_string()))?;
    let decoded;
    let (graph, root) = if let Some((graph, solved)) = solved {
        (graph, solved.owner)
    } else {
        decoded = evaluator
            .decode_type_graph(symbolic_root, "Type")
            .map_err(|message| {
                frontend_error(
                    source_name,
                    format!(
                        "type family {} produced invalid metadata: {message}",
                        binding.value.name.value
                    ),
                )
            })?;
        (&decoded.0, decoded.1)
    };
    let descriptor = graph.descriptor(root).map_err(|message| {
        frontend_error(
            source_name,
            format!(
                "type family {} produced invalid metadata: {message}",
                binding.value.name.value
            ),
        )
    })?;
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
    let (family_value, template, root) =
        evaluator.create_type_family(symbolic_root, parameters.len(), None)?;
    let family = TypeFamilyTemplate {
        parameters: parameters.clone(),
        template,
        root,
        rebuild_at_runtime: contains_named_type(&descriptor),
        constructor: Some(constructor),
    };
    let scheme = TypeScheme {
        parameters,
        constraints: Vec::new(),
        body: TypeDescriptor::Function {
            parameters: family
                .parameters
                .iter()
                .map(|parameter| {
                    TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(parameter.id)))
                })
                .collect(),
            result: Box::new(TypeDescriptor::TypeOf(Box::new(descriptor.clone()))),
        },
    };
    Ok(RecursiveTypeFamilyBuild {
        family_value,
        family,
        scheme,
    })
}

fn native_apply_recursive_type_family(
    context: &mut CallContext<'_, '_>,
) -> Result<(), NativeError> {
    for index in 0..context.argument_count() {
        let argument = native_type_argument_descriptor(context, context.argument(index)?, index)?;
        if argument
            != TypeDescriptor::Bound(TypeParameterId(
                u32::try_from(index)
                    .map_err(|_| NativeError::new("type-family parameter index exceeds u32"))?,
            ))
        {
            return Err(NativeError::new(
                "recursive type-family application must use its bound parameters unchanged and in declaration order",
            ));
        }
    }
    context.copy(context.result(), context.upvalue(0)?)?;
    context.mark_at_call_site(context.result())
}

fn native_apply_type_family(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let template = context.upvalue(0)?;
    let arity = context
        .value(context.upvalue(1)?)?
        .as_int()
        .and_then(|arity| usize::try_from(arity).ok())
        .ok_or_else(|| NativeError::new("invalid type-family arity"))?;
    let mut argument_registers = Vec::with_capacity(arity);
    let mut argument_descriptors = Vec::with_capacity(arity);
    for index in 0..arity {
        let register = context.argument(index)?;
        let argument = native_type_argument_descriptor(context, register, index)?;
        argument_registers.push(register);
        argument_descriptors.push(argument);
    }
    let result = context.result();
    context.instantiate_type_family(
        result,
        template,
        &argument_registers,
        &argument_descriptors,
    )?;
    let module = context
        .value(context.upvalue(2)?)?
        .as_int()
        .ok_or_else(|| NativeError::new("invalid type-constructor module ID"))?;
    if module >= 0 {
        let module = u32::try_from(module)
            .map(crate::ModuleId::from_raw)
            .map_err(|_| NativeError::new("invalid type-constructor module ID"))?;
        let local = context
            .value(context.upvalue(3)?)?
            .as_int()
            .and_then(|local| u32::try_from(local).ok())
            .ok_or_else(|| NativeError::new("invalid type-constructor local ID"))?;
        let name = context
            .value(context.upvalue(4)?)?
            .as_str()
            .ok_or_else(|| NativeError::new("invalid type-constructor name"))?
            .to_string();
        let id = crate::value::DeclaredTypeId::applied(module, local, &argument_descriptors);
        context.make_declared_type_application(result, id, name, result, &argument_registers)?;
    }
    context.mark_at_call_site(result)
}

pub(crate) fn native_declare_type_family(
    context: &mut CallContext<'_, '_>,
) -> Result<(), NativeError> {
    let body = context.argument(0)?;
    let module = context
        .value(context.argument(1)?)?
        .as_int()
        .and_then(|module| u32::try_from(module).ok())
        .map(crate::ModuleId::from_raw)
        .ok_or_else(|| NativeError::new("invalid type-constructor module ID"))?;
    let local = context
        .value(context.argument(2)?)?
        .as_int()
        .and_then(|local| u32::try_from(local).ok())
        .ok_or_else(|| NativeError::new("invalid type-constructor local ID"))?;
    let name = context
        .value(context.argument(3)?)?
        .as_str()
        .ok_or_else(|| NativeError::new("invalid type-constructor name"))?
        .to_string();
    let arity = context.argument_count().saturating_sub(4);
    let mut argument_registers = Vec::with_capacity(arity);
    let mut argument_descriptors = Vec::with_capacity(arity);
    for index in 0..arity {
        let register = context.argument(index + 4)?;
        let argument = native_type_argument_descriptor(context, register, index)?;
        argument_registers.push(register);
        argument_descriptors.push(argument);
    }
    let id = crate::value::DeclaredTypeId::applied(module, local, &argument_descriptors);
    context.make_declared_type_application(
        context.result(),
        id,
        name,
        body,
        &argument_registers,
    )?;
    context.mark_at_call_site(context.result())
}

fn native_type_argument_descriptor(
    context: &CallContext<'_, '_>,
    register: RegisterId,
    index: usize,
) -> Result<TypeDescriptor, NativeError> {
    decode_native_type(context.value(register)?).map_err(|error| {
        NativeError::new(format!(
            "type-family argument {} is not valid TypeMetadata: {}",
            index + 1,
            error.message
        ))
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_nested_annotation_types(
    source_name: &str,
    expression: &Expr,
    bindings: &dyn ToolBindings,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    debug_sink: &mut ToolEvaluator,
    annotations: &mut HashMap<crate::Location, TypeDescriptor>,
) -> Result<(), FrontendError> {
    match &expression.value {
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    collect_nested_annotation_types(
                        source_name,
                        expression,
                        bindings,
                        account,
                        sources,
                        debug_sink,
                        annotations,
                    )?;
                }
            }
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                collect_nested_annotation_types(
                    source_name,
                    item,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::TypeMetadata(operand) => {
            let mut target = operand.as_ref();
            while let ExprKind::TypeSyntax(inner) = &target.value { target = inner; }
            let metadata = if let ExprKind::Variable(name) = &target.value
                && let Some(value) = bindings.get(&name.value)
            { *value } else {
                evaluate_tool_expression(source_name, operand, bindings, account, sources, debug_sink)?
            };
            let descriptor = debug_sink.decode_type(metadata, "Type").map_err(|message| {
                FrontendError::from_diagnostic(sources, Diagnostic::error(message, expression.location))
            })?;
            annotations.insert(expression.location, descriptor);
        }
        ExprKind::TypeSyntax(operand) | ExprKind::Spread(operand) => collect_nested_annotation_types(
            source_name,
            operand,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Dict(fields) => {
            for field in fields {
                collect_nested_annotation_types(
                    source_name,
                    &field.value.value,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Block(block) => {
            collect_block_annotation_types(
                source_name,
                block,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
        }
        ExprKind::Closure {
            parameters,
            result_annotation,
            body,
        } => {
            for annotation in parameters
                .iter()
                .filter_map(|parameter| parameter.annotation.as_ref())
                .chain(result_annotation.as_deref())
            {
                let metadata = evaluate_tool_expression(
                    source_name,
                    annotation,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                )?;
                let descriptor = debug_sink
                    .decode_type(metadata, "Type")
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!("closure annotation is invalid: {message}"),
                                annotation.location,
                            ),
                        )
                    })?;
                annotations.insert(annotation.location, descriptor);
            }
            collect_block_annotation_types(
                source_name,
                body,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
        }
        ExprKind::Unary { operand, .. }
        | ExprKind::FieldProjection { receiver: operand, .. }
        | ExprKind::Propagate { operand }
        | ExprKind::Field {
            receiver: operand, ..
        }
        | ExprKind::TupleProjection {
            receiver: operand, ..
        } => {
            collect_nested_annotation_types(
                source_name,
                operand,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
        }
        ExprKind::Return { value } => collect_nested_annotation_types(
            source_name,
            value,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Panic { message } => collect_nested_annotation_types(
            source_name,
            message,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Raise { message, subjects, .. } => {
            for value in std::iter::once(message.as_ref()).chain(subjects.iter()) {
                collect_nested_annotation_types(source_name, value, bindings, account,
                    sources, debug_sink, annotations)?;
            }
        },
        ExprKind::Debug { value, .. } => collect_nested_annotation_types(
            source_name,
            value,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::TypeAscription { value, target } | ExprKind::CheckedCast { value, target } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            let metadata = evaluate_tool_expression(
                source_name,
                target,
                bindings,
                account,
                sources,
                debug_sink,
            )?;
            let descriptor = debug_sink
                .decode_type(metadata, "Type")
                .map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!("type target is invalid: {message}"),
                            target.location,
                        ),
                    )
                })?;
            annotations.insert(target.location, descriptor);
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            for expression in [namespace.as_ref(), value.as_ref()] {
                collect_nested_annotation_types(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
            let metadata = evaluate_tool_expression(
                source_name,
                target,
                bindings,
                account,
                sources,
                debug_sink,
            )?;
            let descriptor = debug_sink
                .decode_type(metadata, "Type")
                .map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!("Dyn projection target is invalid: {message}"),
                            target.location,
                        ),
                    )
                })?;
            annotations.insert(target.location, descriptor);
        }
        ExprKind::Binary { left, right, .. } => {
            for expression in [left.as_ref(), right.as_ref()] {
                collect_nested_annotation_types(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Index { receiver, index } => {
            for expression in [receiver.as_ref(), index.as_ref()] {
                collect_nested_annotation_types(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Call { callee, arguments } => {
            collect_nested_annotation_types(
                source_name,
                callee,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for argument in arguments {
                collect_nested_annotation_types(
                    source_name,
                    argument,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::TypeApply { callee, arguments } => {
            collect_nested_annotation_types(
                source_name,
                callee,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for argument in arguments {
                let TypeArgumentKind::Explicit(expression) = &argument.value else {
                    continue;
                };
                let metadata = evaluate_tool_expression(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                )?;
                let descriptor = debug_sink
                    .decode_type(metadata, "Type")
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!("type argument is invalid: {message}"),
                                expression.location,
                            ),
                        )
                    })?;
                annotations.insert(expression.location, descriptor);
            }
        }
        ExprKind::Interpreter { operand, .. } => collect_nested_annotation_types(
            source_name,
            operand,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_nested_annotation_types(
                source_name,
                condition,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for block in [then_branch, else_branch] {
                collect_block_annotation_types(
                    source_name,
                    block,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for block in [then_branch, else_branch] {
                collect_block_annotation_types(
                    source_name,
                    block,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for block in [else_branch, body] {
                collect_block_annotation_types(
                    source_name,
                    block,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Match { value, arms } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for arm in arms {
                if let Some(guard) = &arm.value.guard {
                    collect_nested_annotation_types(
                        source_name,
                        guard,
                        bindings,
                        account,
                        sources,
                        debug_sink,
                        annotations,
                    )?;
                }
                collect_nested_annotation_types(
                    source_name,
                    &arm.value.value,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_)
        | ExprKind::Variable(_) => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn collect_block_annotation_types(
    source_name: &str,
    block: &Block,
    bindings: &dyn ToolBindings,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    debug_sink: &mut ToolEvaluator,
    annotations: &mut HashMap<crate::Location, TypeDescriptor>,
) -> Result<(), FrontendError> {
    for binding in &block.value.bindings {
        if let Some(annotation) = &binding.value.annotation {
            let metadata = evaluate_tool_expression(
                source_name,
                annotation,
                bindings,
                account,
                sources,
                debug_sink,
            )?;
            let descriptor = debug_sink
                .decode_type(metadata, "Type")
                .map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!(
                                "annotation on {} is invalid: {message}",
                                binding.value.name.value
                            ),
                            annotation.location,
                        ),
                    )
                })?;
            annotations.insert(annotation.location, descriptor);
        }
        collect_nested_annotation_types(
            source_name,
            &binding.value.value,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?;
    }
    collect_nested_annotation_types(
        source_name,
        &block.value.result,
        bindings,
        account,
        sources,
        debug_sink,
        annotations,
    )
}
