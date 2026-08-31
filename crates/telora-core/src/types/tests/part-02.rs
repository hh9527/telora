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
        let left = analyze_with_natives("if 'True { 1 } else { \"x\" }", &[]).unwrap();
        let right = analyze_with_natives("if 'True { \"x\" } else { 1 }", &[]).unwrap();
        assert_eq!(
            left.display(left.result_type),
            right.display(right.result_type)
        );
        assert_eq!(left.display(left.result_type), "Int | String");

        let metadata = analyze_with_natives("if 'True { Int } else { String }", &[]).unwrap();
        let reversed = analyze_with_natives("if 'True { String } else { Int }", &[]).unwrap();
        assert_eq!(metadata.display(metadata.result_type), "Type");
        assert_eq!(reversed.display(reversed.result_type), "Type");

        let nested = analyze_with_natives(
            "if 'True { if 'False { 1 } else { \"x\" } } else { 1 }",
            &[],
        )
        .unwrap();
        assert_eq!(nested.display(nested.result_type), "Int | String");

        let delayed = analyze_with_natives(
            "def choose = fn(flag, value) {\
                 if flag { value } else { 1 }\
             }; let selected = choose('True, 2); choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            delayed.display(delayed.result_type),
            "Fn(enum {False, True}, Any) -> Any | Int"
        );

        let dynamic =
            analyze_with_natives("let value: Any = 1; if 'True { value } else { 1 }", &[]).unwrap();
        assert_eq!(dynamic.display(dynamic.result_type), "Any");
    }

    #[test]
    fn adversarial_branch_joins_are_pure_symmetric_and_canonical() {
        for (left, right, expected) in [
            (
                "if 'True { if 'False { 1 } else { \"x\" } } else { 1.0 }",
                "if 'True { 1.0 } else { if 'False { \"x\" } else { 1 } }",
                "Float | Int | String",
            ),
            (
                "let dynamic: Any = 1; if 'True { dynamic } else { \"x\" }",
                "let dynamic: Any = 1; if 'True { \"x\" } else { dynamic }",
                "Any",
            ),
            (
                "if 'True { Int } else { Array(String) }",
                "if 'True { Array(String) } else { Int }",
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
             (select('True, \"x\"), select('False, 2.0))",
            &[],
        )
        .unwrap();
        assert_eq!(
            no_leak.display(no_leak.result_type),
            "(Int | String, Float | Int)"
        );
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
        );
        let variable = TypeDescriptor::Inference(InferenceVariableId(0));
        assert!(
            inference
                .unify(
                    &variable,
                    &TypeDescriptor::Array(Box::new(variable.clone()))
                )
                .unwrap_err()
                .contains("infinite type")
        );
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
    #[should_panic(expected = "solver descriptors must be explicitly erased before interning")]
    fn strict_type_graph_interning_rejects_solver_descriptors() {
        TypeGraph::default().intern_descriptor(&TypeDescriptor::Inference(InferenceVariableId(0)));
    }

    #[test]
    fn explicit_runtime_erasure_is_the_only_solver_to_any_path() {
        let mut types = TypeGraph::default();
        let erased = types.intern_erased_descriptor(&TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Bound(TypeParameterId(0))],
            result: Box::new(TypeDescriptor::Inference(InferenceVariableId(0))),
        });
        assert_eq!(types.display(erased), "Fn(Any) -> Any");
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

        round_trip(&TypeDescriptor::AtomValue);

        let witness = TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Array(Box::new(
            TypeDescriptor::Int,
        ))));
        round_trip(&witness);
    }
