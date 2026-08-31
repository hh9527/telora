    #[test]
    fn bootstrap_prelude_keeps_public_projections_consistent() {
        let prelude = BootstrapPrelude::new();
        for name in prelude.schemes.keys() {
            assert!(prelude.types.contains_key(name), "missing type for {name}");
        }
    }
    #[test]
    fn exported_traits_keep_stable_constructor_identity() {
        let analysis = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               export { Display };"#,
        )
        .unwrap();
        let trait_id = analysis.trait_ids["Display"];
        assert_eq!(trait_id.module, crate::ModuleId::ANONYMOUS);
        assert_eq!(trait_id.local, crate::FIRST_DYNAMIC_MODULE_LOCAL);
        assert_eq!(analysis.module_interface.traits["Display"], trait_id);
        assert_eq!(
            analysis.module_interface.type_family_templates["Display"]
                .constructor()
                .unwrap()
                .id,
            crate::TypeConstructorId::from(trait_id)
        );
    }

    #[test]
    fn trait_registry_uses_ids_and_rejects_duplicate_or_fake_traits() {
        let analysis = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               type Endpoint = struct { host: String };
               impl Display for Endpoint { display: fn(value) { value.host } };
               export { Display, Endpoint };"#,
        )
        .unwrap();
        let implementation = &analysis.trait_implementations[0];
        assert_eq!(implementation.trait_id, analysis.trait_ids["Display"]);
        assert_eq!(implementation.id.module, crate::ModuleId::ANONYMOUS);
        assert_eq!(implementation.id.local, crate::FIRST_DYNAMIC_MODULE_LOCAL);
        assert!(
            matches!(implementation.target, TypeDescriptor::Declared(_)),
            "{:?}",
            implementation.target
        );

        let duplicate = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               type Endpoint = struct { host: String };
               impl Display for Endpoint { display: fn(value) { value.host } };
               impl Display for Endpoint { display: fn(value) { value.host } };"#,
        )
        .unwrap_err();
        assert!(duplicate.message.contains("duplicate trait implementation"));

        let fake = analyze_source(
            "traits.telora",
            r#"type Capability(T) = struct { apply: Fn(T) -> String };
               impl Capability for Int { apply: fn(value) { "ok" } };"#,
        )
        .unwrap_err();
        assert!(fake.message.contains("not a visible trait"));

        let wrong_member = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               type Endpoint = struct { host: String };
               impl Display for Endpoint { display: fn(value) { 42 } };"#,
        )
        .unwrap_err();
        assert!(wrong_member.message.contains("String"), "{wrong_member}");

        let overlap = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               trait Marker { mark: Fn(Self) -> String };
               impl(T: Marker) Display for T { display: fn(value) { "generic" } };
               impl Display for Int { display: fn(value) { "int" } };"#,
        )
        .unwrap_err();
        assert!(overlap.message.contains("overlapping trait implementations"));
    }

    #[test]
    fn generic_schemes_publish_canonical_trait_constraints() {
        let analysis = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               def identity: for(T: Display) Fn(T) -> T = fn(value) { value };
               export { Display, identity };"#,
        )
        .unwrap();
        let scheme = &analysis.module_interface.exports["identity"];
        assert_eq!(scheme.display_name(), "for(T: Display) Fn(T) -> T");
        assert!(matches!(
            &scheme.constraints[0].capability,
            TypeCapability::Trait { id, .. } if *id == analysis.trait_ids["Display"]
        ));

        let unknown = analyze_source(
            "traits.telora",
            "def identity: for(T: Missing) Fn(T) -> T = fn(value) { value };",
        )
        .unwrap_err();
        assert!(unknown.message.contains("unknown trait or constraint"));

        let duplicate = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               def identity: for(T: Display + Display) Fn(T) -> T = fn(value) { value };"#,
        )
        .unwrap_err();
        assert!(duplicate.message.contains("duplicate type parameter constraint"));

        let missing = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               def identity: for(T: Display) Fn(T) -> T = fn(value) { value };
               def output = identity(1);"#,
        )
        .unwrap_err();
        assert!(
            missing.message.contains("Int does not implement Display"),
            "{missing}"
        );

        let satisfied = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               impl Display for Int { display: fn(value) { "int" } };
               def identity: for(T: Display) Fn(T) -> T = fn(value) { value };
               def output = identity(1);"#,
        )
        .unwrap();
        assert_eq!(satisfied.display(satisfied.binding_types["output"]), "Int");
    }

    fn analyze_with_natives(
        source: &str,
        natives: &[(&'static str, usize)],
    ) -> Result<Analysis, FrontendError> {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("generic-native.telora", source);
        let parsed = parse_registered(&sources, source_id);
        let program = parsed.program.unwrap_or_else(|| {
            panic!(
                "generic native source parses: {source:?}: {:?}",
                parsed.diagnostics
            )
        });
        let mut tool_heap = Heap::main();
        let mut work = Heap::work_for(&tool_heap);
        let external_roots = natives
            .iter()
            .map(|(name, arity)| {
                let value = work.native_closure(
                    NativeFunction::new(name, *arity, native_validate),
                    Vec::<Val>::new().into_boxed_slice(),
                );
                publish_root(&mut tool_heap, &work, value)
                    .map(|value| ((*name).to_owned(), value))
                    .unwrap()
            })
            .collect();
        let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
        let mut type_store = TypeStore::default();
        analyze_program_with_bindings_observed(
            "generic-native.telora",
            crate::ModuleId::ANONYMOUS,
            ModuleAnalysisContext::Ordinary,
            &program,
            &mut QuotaAccount::new(Quota::with_fuel(100_000)),
            &external_roots,
            &HashSet::new(),
            &sources,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &debug_sink,
            &mut tool_heap,
            &mut type_store,
        )
    }

    fn analyze_with_host_binding(
        source: &str,
        native_arity: Option<usize>,
        dynamic: bool,
        interface: Option<TypeScheme>,
    ) -> Result<Analysis, FrontendError> {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("host-binding.telora", source);
        let parsed = parse_registered(&sources, source_id);
        let program = parsed.program.unwrap_or_else(|| {
            panic!(
                "host binding source parses: {source:?}: {:?}",
                parsed.diagnostics
            )
        });
        let mut tool_heap = Heap::main();
        let mut work = Heap::work_for(&tool_heap);
        let value = native_arity.map_or_else(
            || Val::unknown(crate::heap::DecodedValue::Int(1)),
            |arity| {
                work.native_closure(
                    NativeFunction::new("host", arity, native_validate),
                    Vec::<Val>::new().into_boxed_slice(),
                )
            },
        );
        let external_roots = BTreeMap::from([(
            "host".to_owned(),
            publish_root(&mut tool_heap, &work, value).unwrap(),
        )]);
        let dynamic_bindings = if dynamic {
            HashSet::from(["host".to_owned()])
        } else {
            HashSet::new()
        };
        let external_interfaces = interface
            .map(|scheme| {
                BTreeMap::from([(
                    "host".to_owned(),
                    ModuleInterface {
                        exports: BTreeMap::from([("host".to_owned(), scheme)]),
                        concrete_types: BTreeMap::new(),
                        traits: BTreeMap::new(),
                        trait_implementations: Vec::new(),
                        type_properties: Vec::new(),
                        display_trait: None,
                        type_family_templates: BTreeMap::new(),
                    },
                )])
            })
            .unwrap_or_default();
        let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
        let mut type_store = TypeStore::default();
        analyze_program_with_bindings_observed(
            "host-binding.telora",
            crate::ModuleId::ANONYMOUS,
            ModuleAnalysisContext::Ordinary,
            &program,
            &mut QuotaAccount::new(Quota::with_fuel(100_000)),
            &external_roots,
            &dynamic_bindings,
            &sources,
            &BTreeMap::new(),
            &external_interfaces,
            &debug_sink,
            &mut tool_heap,
            &mut type_store,
        )
    }

    #[test]
    fn host_bindings_distinguish_erased_dynamic_and_declared_interfaces() {
        let erased = analyze_with_host_binding("host(1)", Some(1), false, None).unwrap();
        assert_eq!(
            erased.display(erased.binding_types["host"]),
            "Fn(Any) -> Any"
        );
        assert_eq!(erased.display(erased.result_type), "Any");

        let mut interface_sources = SourceDatabase::default();
        let interface_source = interface_sources.add("host-interface", "");
        let interface_location = crate::Location::from_usize(interface_source, 0..0).unwrap();
        let parameter = TypeParameterId(37);
        let declared = analyze_with_host_binding(
            "host(1)",
            Some(1),
            false,
            Some(TypeScheme {
                parameters: vec![TypeParameter {
                    id: parameter,
                    name: "Value".into(),
                    location: interface_location,
                }],
                constraints: Vec::new(),
                body: TypeDescriptor::Function {
                    parameters: vec![TypeDescriptor::Bound(parameter)],
                    result: Box::new(TypeDescriptor::Bound(parameter)),
                },
            }),
        )
        .unwrap();
        assert_eq!(declared.display(declared.result_type), "Int");
        assert_eq!(
            declared.module_interface.exports.get("host"),
            None,
            "a consumed Host interface is not implicitly re-exported"
        );

        let dynamic = analyze_with_host_binding("host", None, true, None).unwrap();
        assert_eq!(dynamic.display(dynamic.binding_types["host"]), "Any");
        assert_eq!(dynamic.display(dynamic.result_type), "Any");

        let chained =
            analyze_with_natives("if 'False { 1 } else if 'True { \"x\" } else { 2.0 }", &[])
                .unwrap();
        let explicit_nested = analyze_with_natives(
            "if 'False { 1 } else { if 'True { \"x\" } else { 2.0 } }",
            &[],
        )
        .unwrap();
        assert_eq!(
            chained.display(chained.result_type),
            explicit_nested.display(explicit_nested.result_type)
        );
    }
