    #[test]
    fn inferred_callable_schemes_publish_separately_from_call_instances() {
        let source = "let apply = fn(callback, value) { callback(value) };\
                      let result = apply(fn(value) { value + 1 }, 41);\
                      {apply: apply, result: result}";
        let analysis = analyze_with_natives(source, &[]).unwrap();
        assert_eq!(
            analysis.module_interface.exports["apply"].display_name(),
            "for(A, B) Fn(Fn(A) -> B, A) -> B"
        );
        assert_eq!(analysis.display(analysis.binding_types["result"]), "Int");

        let call_start = source.find("apply(fn").unwrap();
        let call = analysis
            .hir
            .expressions()
            .iter()
            .filter(|expression| expression.location.range().start == call_start)
            .max_by_key(|expression| expression.location.range().end)
            .unwrap();
        assert_eq!(analysis.display(analysis.expression_types[&call.id]), "Int");
    }
    #[test]
    fn branch_joins_are_canonical_pure_and_order_independent() {
        for source in ["if Bool.True { 1 } else { \"x\" }", "if Bool.True { \"x\" } else { 1 }"] {
            assert!(analyze_with_natives(source, &[]).err().unwrap().to_string().contains("no common type"));
        }

        let metadata = analyze_with_natives("if Bool.True { Int } else { String }", &[]).unwrap();
        let reversed = analyze_with_natives("if Bool.True { String } else { Int }", &[]).unwrap();
        assert_eq!(metadata.display(metadata.result_type), "Type");
        assert_eq!(reversed.display(reversed.result_type), "Type");

        let nested = analyze_with_natives(
            "if Bool.True { if Bool.False { 1 } else { \"x\" } } else { 1 }",
            &[],
        )
        .err().unwrap();
        assert!(nested.to_string().contains("no common type"));

        let delayed = analyze_with_natives(
            "def choose: Fn(Bool, Int) -> Int = fn(flag, value) {\
                 if flag { value } else { 1 }\
             }; let selected = choose(Bool.True, 2); choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            delayed.display(delayed.result_type),
            "Fn(enum {False, True}, Int) -> Int"
        );

        let concrete =
            analyze_with_natives("let value: Int = 1; if Bool.True { value } else { 1 }", &[]).unwrap();
        assert_eq!(concrete.display(concrete.result_type), "Int");
    }

    #[test]
    fn adversarial_branch_joins_are_pure_symmetric_and_canonical() {
        for (left, right, expected) in [
            (
                "let value: String = \"a\"; if Bool.True { value } else { \"x\" }",
                "let value: String = \"a\"; if Bool.True { \"x\" } else { value }",
                "String",
            ),
            (
                "if Bool.True { Int } else { Array(String) }",
                "if Bool.True { Array(String) } else { Int }",
                "Type",
            ),
        ] {
            let left = analyze_with_natives(left, &[]).unwrap();
            let right = analyze_with_natives(right, &[]).unwrap();
            assert_eq!(left.display(left.result_type), expected);
            assert_eq!(right.display(right.result_type), expected);
        }

        let no_leak = analyze_with_natives(
            "let select = fn(flag, value) { if flag { value } else { 1 } };\
             (select(Bool.True, \"x\"), select(Bool.False, 2.0))",
            &[],
        )
        .err().unwrap();
        assert!(no_leak.to_string().contains("no common type"));
    }

    #[test]
    fn slot_backed_empty_containers_freshen_evidence_without_rebinding_never() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let named_types = BTreeMap::new();
        let annotations = HashMap::new();
        let trait_ids = BTreeMap::new();
        let dyn_namespaces = HashSet::new();
        let mut inference = GenericInference::new(
            &schemes, &hir, &interfaces, &named_types, &annotations,
            &[], &[], &trait_ids, None, &dyn_namespaces, true, None, None,
        );
        let never = inference.variables.structure_node(InferenceConstructor::Never, &[]);
        let array = inference.variables.structure_node(InferenceConstructor::Array, &[never]);
        let actual = TypeDescriptor::Inference(array);
        assert!(inference.variables.contains_runtime_never_leaf(&actual));
        assert!(inference.variables.descriptor_views.iter().all(|view| view.get().is_none()));
        let expected = inference.fresh_variable();
        inference.check(&actual, &expected).unwrap();
        let TypeDescriptor::Array(item) = inference.normalize(&expected) else { panic!("expected array evidence"); };
        assert!(matches!(*item, TypeDescriptor::Inference(_)));
        inference.unify(&item, &TypeDescriptor::Int).unwrap();
        assert_eq!(inference.normalize(&expected), TypeDescriptor::Array(Box::new(TypeDescriptor::Int)));
        assert_eq!(inference.normalize(&actual), TypeDescriptor::Array(Box::new(TypeDescriptor::Never)));
    }

    #[test]
    fn known_named_slots_use_the_same_compatibility_rules_as_inline_names() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let named_types = BTreeMap::new();
        let annotations = HashMap::new();
        let trait_ids = BTreeMap::new();
        let dyn_namespaces = HashSet::new();
        let mut inference = GenericInference::new(
            &schemes, &hir, &interfaces, &named_types, &annotations,
            &[], &[], &trait_ids, None, &dyn_namespaces, true, None, None,
        );
        // Recursive references may have an identity without a local body.
        let named = TypeDescriptor::Named("Tree".into());
        let slot = TypeDescriptor::Inference(inference.variables.structure_edge(named.clone()));
        inference.check(&named, &named).unwrap();
        inference.check(&slot, &named).unwrap();
        inference.check(&named, &slot).unwrap();
        inference.check(&slot, &slot).unwrap();
    }

    #[test]
    fn aggregate_expressions_produce_slot_edges_directly() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let named_types = BTreeMap::new();
        let annotations = HashMap::new();
        let trait_ids = BTreeMap::new();
        let dyn_namespaces = HashSet::new();
        let mut inference = GenericInference::new(
            &schemes, &hir, &interfaces, &named_types, &annotations,
            &[], &[], &trait_ids, None, &dyn_namespaces, true, None, None,
        );
        let mut sources = SourceDatabase::default();
        let source = sources.add("aggregate.telora", "([1, 2], [3])");
        let program = parse_registered(&sources, source).program.unwrap();
        let expression = &program.value.body.value.result;
        let ty = inference.infer(expression, &HashMap::new(), None).unwrap();
        let TypeDescriptor::Inference(tuple) = ty else { panic!("tuple result must be a slot"); };
        assert_eq!(inference.records[&expression.location], ty);
        let arguments = inference.variables.arguments(inference.variables.known(tuple).unwrap());
        assert_eq!(arguments.len(), 2);
        for argument in arguments {
            assert!(matches!(inference.variables.constructor(inference.variables.known(*argument).unwrap()),
                InferenceConstructor::Array));
        }
        assert_eq!(inference.normalize(&ty), TypeDescriptor::Tuple(vec![
            TypeDescriptor::Array(Box::new(TypeDescriptor::Int)),
            TypeDescriptor::Array(Box::new(TypeDescriptor::Int)),
        ]));
        let source = sources.add("record.telora", "{z: [1], a: (2, 3)}");
        let program = parse_registered(&sources, source).program.unwrap();
        let ty = inference.infer(&program.value.body.value.result, &HashMap::new(), None).unwrap();
        let TypeDescriptor::Inference(record) = ty else { panic!("record result must be a slot"); };
        let id = inference.variables.known(record).unwrap();
        let InferenceConstructor::Struct(names) = inference.variables.constructor(id) else {
            panic!("expected record constructor");
        };
        assert_eq!(names.as_ref(), &["a".to_owned(), "z".to_owned()]);
        let children = inference.variables.arguments(id);
        assert!(matches!(inference.variables.constructor(inference.variables.known(children[0]).unwrap()),
            InferenceConstructor::Tuple));
        assert!(matches!(inference.variables.constructor(inference.variables.known(children[1]).unwrap()),
            InferenceConstructor::Array));
    }

    #[test]
    fn deep_slot_unification_uses_graph_edges_without_descriptor_views() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let named_types = BTreeMap::new();
        let annotations = HashMap::new();
        let trait_ids = BTreeMap::new();
        let dyn_namespaces = HashSet::new();
        let mut inference = GenericInference::new(
            &schemes, &hir, &interfaces, &named_types, &annotations,
            &[], &[], &trait_ids, None, &dyn_namespaces, true, None, None,
        );
        let item1 = inference.variables.fresh();
        let item2 = inference.variables.fresh();
        let (mut left, mut right) = (item1, item2);
        for _ in 0..16384 {
            left = inference.variables.structure_edge(TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(left))));
            right = inference.variables.structure_edge(TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(right))));
        }
        inference.unify(&TypeDescriptor::Inference(left), &TypeDescriptor::Inference(right)).unwrap();
        assert_eq!(inference.variables.root(left), inference.variables.root(right));
        assert_eq!(inference.variables.root(item1), inference.variables.root(item2));
        assert!(inference.variables.descriptor_views.iter().all(|view| view.get().is_none()));
        assert!(inference.occurs(item1, &TypeDescriptor::Inference(left)));
        assert!(inference.variables.descriptor_views.iter().all(|view| view.get().is_none()));
        inference.variables.record_conflict(&TypeDescriptor::Inference(item1), "deep conflict");
        assert!(inference.variables.ensure_consistent(&TypeDescriptor::Inference(left)).is_err());
        assert!(inference.variables.ensure_consistent(&TypeDescriptor::Inference(right)).is_err());
    }

    #[test]
    fn generic_native_schemes_are_data_and_occurs_checks_reject_infinite_types() {
        let analysis = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A; {identity: identity}",
            &[("identity", 1)],
        )
        .unwrap();
        let scheme = &analysis.module_interface.exports["identity"];
        assert_eq!(scheme.parameters[0].name, "A");
        assert!(matches!(
            &scheme.body,
            TypeDescriptor::Function { parameters, result }
                if parameters == &[TypeDescriptor::Bound(TypeParameterId(0))]
                    && **result == TypeDescriptor::Bound(TypeParameterId(0))
        ));

        let schemes = HashMap::new();
        let interfaces = BTreeMap::new();
        let annotations = HashMap::new();
        let dyn_namespaces = HashSet::new();
        let named_types = BTreeMap::new();
        let trait_ids = BTreeMap::new();
        let hir = HirProgram::default();
        let mut inference = GenericInference::new(
            &schemes,
            &hir,
            &interfaces,
            &named_types,
            &annotations,
            &[],
            &[],
            &trait_ids,
            None,
            &dyn_namespaces,
            true,
            None,
            None,
        );
        let variable = inference.fresh_variable();
        let inner = inference.fresh_variable();
        inference.unify(&variable, &inner).unwrap();
        assert_eq!(inference.normalize(&inner), variable,
            "captured variables must retain their outer allocation scope");
        assert!(
            inference
                .unify(
                    &variable,
                    &TypeDescriptor::Array(Box::new(variable.clone()))
                )
                .unwrap_err()
                .contains("infinite type")
        );
        let array = inference.fresh_variable();
        let element = inference.fresh_variable();
        let alias = inference.fresh_variable();
        inference.unify(&array, &TypeDescriptor::Array(Box::new(element.clone()))).unwrap();
        inference.unify(&element, &alias).unwrap();
        inference.unify(&alias, &TypeDescriptor::String).unwrap();
        assert_eq!(inference.normalize(&array), TypeDescriptor::Array(Box::new(TypeDescriptor::String)));
        assert_eq!(inference.normalize(&element), TypeDescriptor::String);
        let body_item = inference.variables.fresh();
        let nominal = inference.variables.structure_edge(TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 42),
            name: "CachedBody".into(),
            body: Arc::new(TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(body_item)))),
        }));
        let normalize_nominal = |inference: &GenericInference<'_>| {
            let TypeDescriptor::Declared(declared) = inference.normalize(&TypeDescriptor::Inference(nominal)) else {
                panic!("nominal descriptor expected");
            };
            declared.body
        };
        let before = normalize_nominal(&inference);
        assert!(contains_type_variable(&before));
        assert!(Arc::ptr_eq(&before, &normalize_nominal(&inference)));
        inference.bind_inference_variable(body_item, &TypeDescriptor::Int).unwrap();
        let after = normalize_nominal(&inference);
        assert_eq!(*after, TypeDescriptor::Array(Box::new(TypeDescriptor::Int)));
        assert!(!Arc::ptr_eq(&before, &after));
        assert!(Arc::ptr_eq(&after, &normalize_nominal(&inference)));
        let cyclic = inference.fresh_variable();
        let item = inference.fresh_variable();
        let item_alias = inference.fresh_variable();
        inference.unify(&cyclic, &TypeDescriptor::Array(Box::new(item.clone()))).unwrap();
        inference.unify(&item, &item_alias).unwrap();
        let conflict = inference.unify(&item_alias, &cyclic).unwrap_err();
        assert!(conflict.contains("infinite type"));
        assert_eq!(inference.unify(&item, &TypeDescriptor::Int).unwrap_err(), conflict);

        let bottom = inference.fresh_variable();
        let integer = inference.fresh_variable();
        inference.unify(&bottom, &TypeDescriptor::Never).unwrap();
        inference.unify(&integer, &TypeDescriptor::Int).unwrap();
        inference.unify(&bottom, &integer).unwrap();
        assert_eq!(inference.normalize(&bottom), TypeDescriptor::Never);
        assert_eq!(inference.normalize(&integer), TypeDescriptor::Int);
    }

    #[test]
    fn published_schemes_reject_solver_and_unbound_parameter_identities() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("scheme.telora", "");
        let location = crate::Location::from_usize(source, 0..0).unwrap();
        let valid = TypeScheme {
            parameters: vec![TypeParameter {
                id: TypeParameterId(0),
                name: "A".into(),
                location,
            }],
            constraints: Vec::new(),
            body: TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Bound(TypeParameterId(0))],
                result: Box::new(TypeDescriptor::Bound(TypeParameterId(0))),
            },
        };
        assert!(validate_publishable_scheme(&valid).is_ok());

        let unresolved = TypeScheme {
            parameters: Vec::new(),
            constraints: Vec::new(),
            body: TypeDescriptor::Inference(InferenceVariableId(0)),
        };
        assert!(
            validate_publishable_scheme(&unresolved)
                .unwrap_err()
                .contains("unresolved")
        );

        let unbound = TypeScheme {
            parameters: Vec::new(),
            constraints: Vec::new(),
            body: TypeDescriptor::Bound(TypeParameterId(7)),
        };
        assert!(
            validate_publishable_scheme(&unbound)
                .unwrap_err()
                .contains("unbound parameter T7")
        );

        let unbound_constraint = TypeScheme {
            parameters: Vec::new(),
            constraints: vec![TypeConstraint {
                parameter: TypeParameterId(7),
                capability: TypeCapability::Property(TypeDescriptor::Int),
                location,
            }],
            body: TypeDescriptor::Int,
        };
        assert!(
            validate_publishable_scheme(&unbound_constraint)
                .unwrap_err()
                .contains("constraint references unbound parameter T7")
        );
    }

    #[test]
    #[should_panic(expected = "solver descriptors must be resolved before interning")]
    fn strict_type_graph_interning_rejects_solver_descriptors() {
        TypeGraph::default().intern_descriptor(&TypeDescriptor::Inference(InferenceVariableId(0)));
    }

    #[test]
    fn type_graph_preserves_parameters_and_omits_unresolved_evidence() {
        let mut types = TypeGraph::default();
        let unresolved = types.intern_resolved_descriptor(&TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Bound(TypeParameterId(0))],
            result: Box::new(TypeDescriptor::Inference(InferenceVariableId(0))),
        });
        assert!(unresolved.is_none());
        let descriptor = TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Bound(TypeParameterId(0))],
            result: Box::new(TypeDescriptor::Bound(TypeParameterId(0))),
        };
        let resolved = types.intern_resolved_descriptor(&descriptor).unwrap();
        assert_eq!(types.descriptor(resolved).unwrap(), descriptor);
    }

    #[test]
    fn metadata_round_trips() {
        fn round_trip(descriptor: &TypeDescriptor) {
            let mut heap = Heap::work();
            let value = heap.type_descriptor_value(None, descriptor).unwrap();
            let world = crate::DataWorld::new(heap, value);
            assert_eq!(decode_type_ref(world.value(), "Type").unwrap(), *descriptor);
        }

        let descriptor = TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Struct(BTreeMap::from([
                ("age".into(), TypeDescriptor::Int),
                ("name".into(), TypeDescriptor::String),
            ]))],
            result: Box::new(TypeDescriptor::Enum(BTreeMap::from([
                ("None".into(), None),
                ("Some".into(), Some(Box::new(TypeDescriptor::String))),
            ]))),
        };
        round_trip(&descriptor);

        let bound = TypeDescriptor::Array(Box::new(TypeDescriptor::Bound(TypeParameterId(7))));
        round_trip(&bound);

        let metatype = TypeDescriptor::Type;
        round_trip(&metatype);

        let never = TypeDescriptor::Never;
        round_trip(&never);

        round_trip(&normalized_bool_descriptor());

        let witness = TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Array(Box::new(
            TypeDescriptor::Int,
        ))));
        round_trip(&witness);
    }
