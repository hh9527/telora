    #[test]
    fn solved_module_plan_publishes_family_and_value_contracts_without_heap() {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("static-plan.telora",
            "type Box(T) = struct { value: T }; def answer = 1 / 0; export { Box, answer };");
        let program = parse_registered(&sources, source_id).program.unwrap();
        let solved = solve_module_plan(
            "static-plan.telora", crate::ModuleId::ANONYMOUS,
            ModuleAnalysisContext::Ordinary, &program,
            resolve_module_hir(&program, &BTreeSet::new(), HashSet::new()),
            &BTreeSet::new(), &sources,
            &BTreeMap::new(), &BTreeMap::new(), &[], None, &mut TypeStore::default(),
        ).unwrap();
        assert_eq!(solved.module_interface.exports["answer"].body, TypeDescriptor::Int);
        assert_eq!(solved.module_interface.type_family_constructors["Box"].id.module,
            crate::ModuleId::ANONYMOUS);
        assert!(!solved.expression_types.is_empty());
        assert!(!solved.declaration_plans.is_empty());
    }

    #[test]
    fn static_type_error_leaves_main_heap_unallocated() {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("static-error.telora", "def answer: Int = \"wrong\";");
        let program = parse_registered(&sources, source_id).program.unwrap();
        let mut heap = Heap::main();
        let allocations = heap.allocation_count();
        let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
        let error = analyze_program_with_bindings_observed(
            "static-error.telora", crate::ModuleId::ANONYMOUS,
            ModuleAnalysisContext::Ordinary, &program,
            resolve_module_hir_with_interfaces(&program, std::iter::empty(), &BTreeMap::new()),
            &mut QuotaAccount::new(Quota::with_fuel(100_000)),
            &BTreeMap::new(), &HashSet::new(), &sources,
            &BTreeMap::new(), &BTreeMap::new(), &debug_sink,
            &mut heap, &mut TypeStore::default(), &[],
        ).err().expect("invalid annotation must fail statically");
        assert!(error.to_string().contains("cannot unify String with Int"), "{error}");
        assert_eq!(heap.allocation_count(), allocations,
            "static rejection must leave the main heap unallocated");
    }

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
            analysis.module_interface.type_family_constructors["Display"]
                .id,
            crate::TypeConstructorId::from(trait_id)
        );
    }

    #[test]
    fn trait_member_signature_solves_self_before_selecting_implementation() {
        let analysis = analyze_source(
            "deferred-trait.telora",
            r#"trait Combine { combine: Fn(Self, Self) -> Self };
               impl Combine for Int { combine: fn(a, b) { a + b } };
               def add_one = fn(x) { Combine.combine(x, 1) };
               def output = add_one(41);
               export { add_one, output };"#,
        ).unwrap();
        assert_eq!(analysis.display(analysis.binding_types["output"]), "Int");
        assert_eq!(analysis.display(analysis.binding_types["add_one"]), "Fn(Int) -> Int");

        let missing = analyze_source(
            "missing-deferred-trait.telora",
            r#"trait Combine { combine: Fn(Self, Self) -> Self };
               def add_one = fn(x) { Combine.combine(x, 1) };"#,
        ).unwrap_err();
        assert!(missing.message.contains("Int does not implement Combine"), "{missing}");
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
        let external_roots: BTreeMap<_, _> = natives
            .iter()
            .map(|(name, arity)| {
                let value = work.native_closure(
                    NativeFunction::new(name, *arity, native_checked_cast),
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
            resolve_module_hir_with_interfaces(&program, external_roots.keys().cloned(), &BTreeMap::new()),
            &mut QuotaAccount::new(Quota::with_fuel(100_000)),
            &external_roots,
            &HashSet::new(),
            &sources,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &debug_sink,
            &mut tool_heap,
            &mut type_store,
            &[],
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
                    NativeFunction::new("host", arity, native_checked_cast),
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
                        value_binding: Some("host".into()),
                        type_declarations: BTreeSet::new(),
                        member_constructors: BTreeMap::new(),
                        namespaces: BTreeMap::new(),
                        exports: BTreeMap::from([("host".to_owned(), scheme)]),
                        concrete_types: BTreeMap::new(),
                        traits: BTreeMap::new(),
                        trait_implementations: Vec::new(),
                        type_properties: Vec::new(),
                        display_trait: None,
                        type_family_constructors: BTreeMap::new(),
                    },
                )])
            })
            .unwrap_or_default();
        let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
        let mut type_store = TypeStore::default();
        let mut hir = resolve_module_hir_with_interfaces(&program, external_roots.keys().cloned(), &external_interfaces);
        hir.set_import_origins(&BTreeMap::from([("host".to_owned(), crate::hir::HirImportOrigin::Export {
            module: crate::ModuleId::ANONYMOUS, index: 0,
        })]));
        analyze_program_with_bindings_observed(
            "host-binding.telora",
            crate::ModuleId::ANONYMOUS,
            ModuleAnalysisContext::Ordinary,
            &program,
            hir,
            &mut QuotaAccount::new(Quota::with_fuel(100_000)),
            &external_roots,
            &dynamic_bindings,
            &sources,
            &BTreeMap::new(),
            &external_interfaces,
            &debug_sink,
            &mut tool_heap,
            &mut type_store,
            &[],
        )
    }

    #[test]
    fn host_bindings_require_interfaces_for_functions_and_retain_value_types() {
        let missing = analyze_with_host_binding("host(1)", Some(1), false, None).err().unwrap();
        assert!(missing.message.contains("requires an explicit type interface"));

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
        let reference = declared.hir.references().iter().find(|reference| reference.name == "host").unwrap();
        assert_eq!(declared.hir.reference_import_origin(reference.id), Some(crate::hir::HirImportOrigin::Export {
            module: crate::ModuleId::ANONYMOUS, index: 0,
        }), "analysis must consume the supplied HIR without rebuilding and losing its source identities");
        assert_eq!(
            declared.module_interface.exports.get("host"),
            None,
            "a consumed Host interface is not implicitly re-exported"
        );

        let missing_data = analyze_with_host_binding("host", None, true, None).unwrap_err();
        assert!(missing_data.message.contains("requires an explicit type interface"));
        let dynamic = analyze_with_host_binding("host", None, true, Some(TypeScheme {
            parameters: Vec::new(), constraints: Vec::new(), body: TypeDescriptor::Int,
        })).unwrap();
        assert_eq!(dynamic.display(dynamic.binding_types["host"]), "Int");
        assert_eq!(dynamic.display(dynamic.result_type), "Int");

        let chained =
            analyze_with_natives("if Bool.False { 1 } else if Bool.True { \"x\" } else { 2.0 }", &[])
                .err().unwrap();
        let explicit_nested = analyze_with_natives(
            "if Bool.False { 1 } else { if Bool.True { \"x\" } else { 2.0 } }",
            &[],
        )
        .err().unwrap();
        assert!(chained.to_string().contains("no common type"));
        assert!(explicit_nested.to_string().contains("no common type"));
    }
