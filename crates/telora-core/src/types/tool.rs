struct ToolEvaluator<'a> {
    observed_vm: Vm,
    silent_vm: Vm,
    main: &'a mut Heap,
    work: Heap,
    tool_types: TypeGraph,
    tool_type_values: crate::heap::TypeGraphMaterialization,
    registered_construction_checks: BTreeSet<PropertyKey>,
    construction_checks_complete: bool,
}

struct ToolInferenceContext<'a> {
    types: TypeGraph,
    hir: &'a HirProgram,
    interfaces: BTreeMap<String, ModuleInterface>,
    environment: HashMap<String, TypeDescriptor>,
    schemes: HashMap<String, TypeScheme>,
    named_types: BTreeMap<String, TypeDescriptor>,
    builtin_tuple_available: bool,
    declared_bodies: HashMap<crate::value::DeclaredTypeId, Arc<TypeDescriptor>>,
    trait_implementations: Vec<TraitImplementation>,
    type_properties: Vec<TypePropertyEvidence>,
    trait_ids: BTreeMap<String, crate::TraitId>,
    display_trait: Option<(crate::TraitId, String)>,
}

#[derive(Default)]
struct ToolExpressionEvidence {
    external_names: HashSet<String>,
    expression_types: HashMap<crate::Location, ToolTypeRoot>,
    value_constructors: HashMap<crate::Location, ValueConstructor>,
    calls: HashMap<crate::Location, Vec<ResolvedEvidence>>,
    runtime_types: BTreeMap<String, AnalysisTypeId>,
    parameters: HashMap<crate::Location, Vec<String>>,
    lexical_types: HashMap<TypeParameterId, String>,
    inferred_scopes: HashMap<crate::Location, Vec<LexicalTypeEvidence>>,
    families: HashMap<crate::Location, PropagationFamily>,
    not_families: HashMap<crate::Location, NotFamily>,
    members: HashMap<crate::Location, ResolvedEvidence>,
    interpolations: HashMap<crate::Location, ResolvedEvidence>,
}

// IDs always refer to the module's shared tool type graph. Intermediate
// tool records can be open even after a successful inference pass; keep those
// explicit until all consumers support an open graph snapshot.
#[derive(Clone, Copy)]
enum ToolTypeRoot {
    Graph(AnalysisTypeId),
    OpenFunction { arity: usize, owner: Option<AnalysisTypeId> },
    Unresolved,
}

impl ToolTypeRoot {
    #[cfg(test)]
    fn runtime_value(&self, graph: &TypeGraph, evaluator: &mut ToolEvaluator<'_>) -> Result<Val, FrontendError> {
        match self {
            Self::Graph(id) => evaluator.work.type_graph_value(Some(evaluator.main), graph, *id)
                .map_err(|error| frontend_error("<tool-stage>", error.to_string())),
            _ => Err(frontend_error("<tool-stage>", "open expression shape is not a runtime type")),
        }
    }

    fn bound_arity(&self, graph: &TypeGraph) -> usize {
        let mut parameters = Vec::new();
        match self {
            Self::OpenFunction { .. } | Self::Unresolved => {},
            Self::Graph(root) => {
                let mut visited = vec![false; graph.nodes().len()];
                let mut pending = vec![*root];
                while let Some(id) = pending.pop() {
                    if std::mem::replace(&mut visited[id.index()], true) { continue; }
                    match graph.node(id) {
                        TypeNode::Bound(parameter) => parameters.push(*parameter),
                        TypeNode::Ref(child) | TypeNode::Array(child) | TypeNode::Newtype(child)
                        | TypeNode::Dict(child) | TypeNode::TypeOf(child)
                        | TypeNode::Tagged { payload: child, .. } => pending.push(*child),
                        TypeNode::Declared { id, body, .. } => {
                            // Phantom arguments still contribute to family arity.
                            for argument in id.arguments() { collect_bound_parameters(argument, &mut parameters); }
                            pending.push(*body);
                        }
                        TypeNode::Tuple(items) | TypeNode::PendingAlternatives(items) => pending.extend(items),
                        TypeNode::Struct(fields) => pending.extend(fields.values()),
                        TypeNode::Enum(variants) => pending.extend(variants.values().flatten()),
                        TypeNode::Function { parameters, result } => {
                            pending.extend(parameters);
                            pending.push(*result);
                        }
                        _ => {}
                    }
                }
            }
        }
        parameters.iter().map(|parameter| parameter.index() as usize + 1).max().unwrap_or(0)
    }

    #[cfg(test)]
    fn import(graph: &mut TypeGraph, descriptor: &TypeDescriptor) -> Self {
        match graph.intern_resolved_descriptor(descriptor) {
            Some(id) => Self::Graph(id),
            None => Self::open_shape(graph, descriptor),
        }
    }

    fn open_shape(graph: &mut TypeGraph, descriptor: &TypeDescriptor) -> Self {
        match descriptor {
            TypeDescriptor::Function { parameters, result } => Self::OpenFunction {
                arity: parameters.len(),
                owner: matches!(result.as_ref(), TypeDescriptor::Declared(_))
                    .then(|| graph.intern_resolved_descriptor(result)).flatten(),
            },
            _ => Self::Unresolved,
        }
    }

    fn descriptor(&self, graph: &TypeGraph) -> Result<TypeDescriptor, String> {
        match self {
            Self::Graph(id) => graph.descriptor(*id),
            _ => Err("open expression shape has no solved type descriptor".into()),
        }
    }

    fn arity(&self, graph: &TypeGraph) -> Option<usize> {
        match self {
            Self::Graph(id) => match graph.node(*id) {
                TypeNode::Function { parameters, .. } => Some(parameters.len()),
                _ => None,
            },
            Self::OpenFunction { arity, .. } => Some(*arity),
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
            Self::OpenFunction { owner, .. } if constructor => owner.map(Self::Graph),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tool_type_root_tests {
    use super::*;

    #[test]
    fn open_function_shape_reads_slots_without_rebuilding_descriptors() {
        let mut variables = InferenceVariables::default();
        let unknown = variables.fresh();
        let int = variables.structure_edge(TypeDescriptor::Int);
        let owner = variables.structure_node(InferenceConstructor::Declared {
            head: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 989),
            name: "KnownOwner".into(),
        }, &[int]);
        let function = variables.structure_node(InferenceConstructor::Function, &[unknown, owner]);
        let proxy = variables.fresh();
        variables.set(proxy, TypeDescriptor::Inference(function));
        let mut graph = TypeGraph::default();
        let mut publication = InferencePublication::new(&variables);
        let shape = publication.publish_tool_root(&mut graph, proxy,
            |_| panic!("open function shape must not rebuild a descriptor"));
        assert_eq!(shape.arity(&graph), Some(1));
        assert!(shape.descriptor(&graph).is_err());
        let resolved_owner = shape.owner(&graph, true).expect("independently solved owner");
        assert!(matches!(resolved_owner, ToolTypeRoot::Graph(_)));
        assert_eq!(graph.nodes().len(), 2);
    }

    #[test]
    fn graph_metadata_batches_share_results_and_discard_failed_provisional_slots() {
        let mut graph = TypeGraph::default();
        let int = graph.intern_node(TypeNode::Int);
        let shared = graph.intern_node(TypeNode::Array(int));
        let pair = graph.intern_node(TypeNode::Tuple(vec![shared, shared]));
        let string = graph.intern_node(TypeNode::String);
        let invalid = graph.push(TypeNode::Pending);
        graph.finish_reserved_node(invalid, TypeNode::Array(invalid));
        let invalid_owner = graph.intern_declared_body(
            crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 992),
            "Invalid".into(), invalid);
        let mut main = Heap::main();
        let mut evaluator = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut main);
        let mut materialized = crate::heap::TypeGraphMaterialization::default();
        let values = evaluator.work.type_graph_values_in(Some(evaluator.main), &graph, [
            shared, string, pair, shared,
        ], &mut materialized).unwrap();
        assert_eq!(values[0].value(), values[3].value());
        for (value, expected) in values.iter().zip([
            TypeDescriptor::Array(Box::new(TypeDescriptor::Int)), TypeDescriptor::String,
            TypeDescriptor::Tuple(vec![TypeDescriptor::Array(Box::new(TypeDescriptor::Int)); 2]),
            TypeDescriptor::Array(Box::new(TypeDescriptor::Int)),
        ]) {
            let (decoded, id) = evaluator.decode_type_graph(*value, "Type").unwrap();
            assert_eq!(decoded.descriptor(id).unwrap(), expected);
        }
        let reused = evaluator.work.type_graph_values_in(Some(evaluator.main), &graph,
            [pair], &mut materialized).unwrap();
        assert_eq!(reused[0].value(), values[2].value());
        assert!(evaluator.work.type_graph_values_in(Some(evaluator.main), &graph,
            [shared, invalid], &mut materialized).is_err());
        for _ in 0..2 {
            assert!(evaluator.work.type_graph_values_in(Some(evaluator.main), &graph,
                [invalid_owner], &mut materialized).is_err());
        }
        let next = evaluator.work.type_graph_values_in(Some(evaluator.main), &graph,
            [pair], &mut materialized).unwrap();
        let (decoded, id) = evaluator.decode_type_graph(next[0], "Type").unwrap();
        assert_eq!(decoded.descriptor(id).unwrap(), graph.descriptor(pair).unwrap());
        assert!(evaluator.work.type_graph_values_in(Some(evaluator.main), &graph, [], &mut materialized).unwrap().is_empty());
    }

    #[test]
    fn graph_metadata_matches_descriptor_metadata_and_preserves_phantom_bounds() {
        let phantom = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::applied(crate::ModuleId::ANONYMOUS, 918,
                &[TypeDescriptor::Bound(TypeParameterId(3))]),
            name: "Phantom".into(), body: Arc::new(TypeDescriptor::Newtype(Box::new(TypeDescriptor::Int))),
        });
        let descriptor = TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Tuple(vec![TypeDescriptor::Int, TypeDescriptor::Int]),
                TypeDescriptor::Dict(Box::new(TypeDescriptor::String)), phantom],
            result: Box::new(TypeDescriptor::Enum(BTreeMap::from([
                ("Empty".into(), None),
                ("Values".into(), Some(Box::new(TypeDescriptor::Array(Box::new(TypeDescriptor::Bytes))))),
            ]))),
        };
        let mut graph = TypeGraph::default();
        let root = ToolTypeRoot::Graph(graph.intern_descriptor(&descriptor));
        assert_eq!(root.bound_arity(&graph), 4);
        let mut main = Heap::main();
        let mut evaluator = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut main);
        let value = root.runtime_value(&graph, &mut evaluator).unwrap();
        let (decoded, id) = evaluator.decode_type_graph(value, "Type").unwrap();
        let actual = decoded.descriptor(id).unwrap();
        let mut legacy_main = Heap::main();
        let mut legacy = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut legacy_main);
        let value = legacy.descriptor(&descriptor).unwrap();
        let (decoded, id) = legacy.decode_type_graph(value, "Type").unwrap();
        assert_eq!(actual, decoded.descriptor(id).unwrap());
    }

    #[test]
    fn graph_metadata_accepts_a_structural_root_crossing_a_nominal_cycle() {
        let analysis = analyze_source("metadata-cycle.telora",
            "type Node = struct {children: Array(Node)}; export def id: Fn(Node) -> Node = fn(x) {x};").unwrap();
        let graph = &analysis.types;
        let owner = analysis.declared_types["Node"];
        let TypeNode::Declared { body, .. } = graph.node(owner) else { panic!("owner"); };
        let TypeNode::Struct(fields) = graph.node(*body) else { panic!("body"); };
        let root = ToolTypeRoot::Graph(fields["children"]);
        let mut main = Heap::main();
        let mut evaluator = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut main);
        let value = root.runtime_value(graph, &mut evaluator).unwrap();
        let (decoded, id) = evaluator.decode_type_graph(value, "Type").unwrap();
        assert_eq!(decoded.descriptor(id).unwrap(), graph.descriptor(fields["children"]).unwrap());
        let mut invalid = TypeGraph::default();
        let slot = invalid.push(TypeNode::Pending);
        invalid.finish_reserved_node(slot, TypeNode::Array(slot));
        assert!(ToolTypeRoot::Graph(slot).runtime_value(&invalid, &mut evaluator).is_err());
    }

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
        fn assert_copy<T: Copy>() {}
        assert_copy::<ToolTypeRoot>();
        let mut variables = InferenceVariables::default();
        let slot = variables.fresh();
        let descriptor = TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Inference(slot)],
            result: Box::new(TypeDescriptor::Inference(slot)),
        };
        let mut graph = TypeGraph::default();
        let root = ToolTypeRoot::import(&mut graph, &descriptor);
        assert!(matches!(root, ToolTypeRoot::OpenFunction { arity: 1, owner: None }));
        assert_eq!(root.arity(&graph), Some(1));
        assert!(root.owner(&graph, true).is_none());
        assert!(root.descriptor(&graph).is_err());
        assert_eq!(graph.nodes().len(), 0);

        let owner = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 988),
            name: "KnownOwner".into(), body: Arc::new(TypeDescriptor::Newtype(Box::new(TypeDescriptor::Int))),
        });
        let constructor = ToolTypeRoot::import(&mut graph, &TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Inference(slot)], result: Box::new(owner.clone()),
        });
        assert_eq!(constructor.arity(&graph), Some(1));
        assert!(constructor.descriptor(&graph).is_err());
        assert!(constructor.owner(&graph, false).is_none());
        let resolved_owner = constructor.owner(&graph, true).expect("known owner retained independently");
        assert_eq!(resolved_owner.descriptor(&graph).unwrap(), owner);
    }
}

impl<'a> ToolInferenceContext<'a> {
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
        types: TypeGraph,
        hir: &'a HirProgram,
        interfaces: BTreeMap<String, ModuleInterface>,
        environment: HashMap<String, TypeDescriptor>,
        schemes: HashMap<String, TypeScheme>,
        named_types: BTreeMap<String, TypeDescriptor>,
        builtin_tuple_available: bool,
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
            types,
            hir, interfaces, environment, schemes, named_types, builtin_tuple_available,
            declared_bodies: HashMap::new(), trait_implementations,
            type_properties, trait_ids, display_trait,
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
        }
    }

}

impl<'a> ToolEvaluator<'a> {
    fn new(debug_sink: Arc<dyn DebugSink>, main: &'a mut Heap) -> Self {
        let work = Heap::work_for(main);
        Self {
            observed_vm: Vm::new().with_debug_sink(debug_sink),
            silent_vm: Vm::new().with_debug_sink(Arc::new(DiscardDebugSink)),
            main,
            work,
            tool_types: TypeGraph::default(),
            tool_type_values: crate::heap::TypeGraphMaterialization::default(),
            registered_construction_checks: BTreeSet::new(),
            construction_checks_complete: false,
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

    #[cfg(test)]
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
}

#[allow(clippy::too_many_arguments)]
fn build_recursive_type_family(
    source_name: &str,
    module_id: crate::ModuleId,
    declaration: u32,
    binding: &Binding,
    base_bindings: &dyn ToolBindings,
    parameters: &[TypeParameter],
    rebuild_at_runtime: bool,
    evaluator: &mut ToolEvaluator<'_>,
    solved: (&TypeGraph, &SolvedRecursiveType),
) -> Result<RecursiveTypeFamilyBuild, FrontendError> {
    let mut evaluation_bindings = ScopedToolBindings::new(base_bindings);
    for parameter in parameters {
        evaluation_bindings.insert(
            parameter.name.clone(),
            evaluator.descriptor(&TypeDescriptor::Bound(parameter.id))?,
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
        evaluator.reserve_recursive_type_family(&constructor, parameters)?;
    evaluation_bindings.insert(binding.value.name.value.clone(), self_family);
    let (graph, solved) = solved;
    let body = materialize_type_body(solved.body, graph, binding, source_name,
        &evaluation_bindings, evaluator)?.0;
    evaluator
        .work
        .seal_type_ref(symbolic_root, body)
        .map_err(|error| frontend_error(source_name, error.to_string()))?;
    let (family_value, template, root) =
        evaluator.create_type_family(symbolic_root, parameters.len(), None)?;
    let family = TypeFamilyTemplate {
        template,
        root,
        rebuild_at_runtime,
        constructor: Some(constructor),
    };
    Ok(RecursiveTypeFamilyBuild {
        family_value,
        family,
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
