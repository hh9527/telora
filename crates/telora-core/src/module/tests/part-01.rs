    #[test]
    fn module_skeleton_assigns_separate_stable_function_and_type_slots() {
        let blueprint = module_blueprint(
            "decl call: Fn(Int) -> Int; def call = fn(value) { value }; type Box(T) = struct { value: T }; def other = fn(value) { value }; type State = struct { value: Int }; 0",
        )
        .unwrap();
        let funcs = blueprint
            .slots
            .iter()
            .filter(|slot| slot.kind == StaticSlotKind::Func)
            .collect::<Vec<_>>();
        let types = blueprint
            .slots
            .iter()
            .filter(|slot| slot.kind == StaticSlotKind::TypeConstructor)
            .collect::<Vec<_>>();

        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].name, "call");
        assert_eq!(funcs[0].local, crate::FIRST_DYNAMIC_MODULE_LOCAL);
        assert_eq!((funcs[0].declarations, funcs[0].definitions), (1, 1));
        assert_eq!(funcs[1].name, "other");
        assert_eq!(funcs[1].local, crate::FIRST_DYNAMIC_MODULE_LOCAL + 1);

        assert_eq!(types.len(), 2);
        assert_eq!(types[0].name, "Box");
        assert_eq!(types[0].local, crate::FIRST_DYNAMIC_MODULE_LOCAL);
        assert_eq!(types[1].name, "State");
        assert_eq!(types[1].local, crate::FIRST_DYNAMIC_MODULE_LOCAL + 1);
    }
    #[test]
    fn module_skeleton_rejects_incomplete_or_duplicate_function_slots() {
        let missing = module_blueprint("decl call: Fn(Int) -> Int; 0").unwrap_err();
        assert!(missing.contains("has no definition"));

        let duplicate = module_blueprint(
            "decl call: Fn(Int) -> Int; def call = fn(value) { value }; def call = fn(value) { value }; 0",
        )
        .unwrap_err();
        assert!(duplicate.contains("cannot shadow"));
    }

    #[test]
    fn module_skeleton_allows_only_let_to_shadow_a_definition() {
        module_blueprint("def call = fn(value) { value }; let call = 1; call")
            .expect("let may shadow a definition");

        let direct = module_blueprint(
            "let call = fn(value) { value }; def call = fn(value) { value }; call",
        )
        .unwrap_err();
        assert!(direct.contains("cannot shadow"));

        let through_let = module_blueprint(
            "decl call: Fn(Int) -> Int; let call = fn(value) { value }; def call = fn(value) { value }; call",
        )
        .unwrap_err();
        assert!(through_let.contains("cannot shadow"));
    }

    #[test]
    fn module_skeleton_rejects_explicit_import_name_collisions() {
        for binding in [
            "type Item = struct {value: Int};",
            "def Item = 1;",
            "decl Item: Fn() -> Int;",
            "native Item: Fn() -> Int;",
            "native type Item @7;",
        ] {
            let source =
                format!("import \"./provider\" {{ Item }}; {binding} export {{Item}};");
            let mut sources = SourceDatabase::default();
            let source_id = sources.add("@test/conflict.telora", source);
            let parsed = parse_registered(&sources, source_id);
            let program = parsed.program.unwrap_or_else(|| {
                panic!("conflict fixture did not parse: {source_id:?}: {binding}")
            });
            let diagnostics = module_binding_diagnostics(&program);
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic
                    .message
                    .contains("conflicts with an earlier explicit binding")),
                "{diagnostics:?}"
            );
            assert_eq!(diagnostics[0].labels.len(), 2);
        }
    }

    fn named_output(value: &crate::ExecutionWorld) -> crate::ValueRef<'_> {
        value.value().dict_get("output").expect("output export")
    }
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("telora-module-test-{unique}"));
        fs::create_dir(&path).unwrap();
        path
    }

    fn recovery_engine() -> Engine {
        Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(1_000_000),
            session_quota: Quota::with_fuel(1_000_000),
            data_limits: DataLimits::default(),
        })
    }

    #[derive(Default)]
    struct CapturingDebugSink {
        events: Mutex<Vec<crate::DebugEvent>>,
    }

    impl crate::DebugSink for CapturingDebugSink {
        fn emit(&self, event: crate::DebugEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[test]
    fn core_native_module_ids_are_reserved_unique_and_order_independent() {
        let specs = module_specs();
        let identities = specs
            .iter()
            .map(|spec| (spec.name, spec.native_id))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(identities.len(), specs.len());
        assert!(
            identities
                .values()
                .all(|id| *id > 0 && *id <= crate::value::RESERVED_NATIVE_MODULE_MAX)
        );
        assert_eq!(
            identities.values().copied().collect::<HashSet<_>>().len(),
            specs.len()
        );
        assert_eq!(identities.get(crate::core::EXEC_MODULE), Some(&21));
        assert!(!identities.values().any(|id| *id == 12));
        let reordered = specs
            .iter()
            .rev()
            .map(|spec| (spec.name, spec.native_id))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(identities, reordered);
    }

    #[test]
    fn native_type_slots_are_explicit_unique_and_order_independent() {
        fn declarations(
            source: &str,
        ) -> Result<BTreeMap<u32, (String, crate::NativeType)>, ModuleError> {
            let mut sources = SourceDatabase::default();
            let source_id = sources.add("<fixture>", source);
            let program = parse_registered(&sources, source_id).program.unwrap();
            declared_native_types(
                &program,
                crate::value::NativeModuleId(1024),
                "host:fixture",
                &sources,
            )
        }

        let forward = declarations("native type First @7; native type Second @2; First").unwrap();
        let reversed = declarations("native type Second @2; native type First @7; First").unwrap();
        assert_eq!(forward.get(&2).unwrap().1, reversed.get(&2).unwrap().1);
        assert_eq!(forward.get(&7).unwrap().1, reversed.get(&7).unwrap().1);

        let duplicate =
            declarations("native type First @7; native type Second @7; First").unwrap_err();
        assert!(
            duplicate
                .to_string()
                .contains("duplicate native type slot @7")
        );

        let overflow = declarations("native type Huge @4294967296; Huge").unwrap_err();
        assert!(overflow.to_string().contains("must fit the u32 range"));
    }

    #[test]
    fn contextual_debug_observes_values_with_authored_context() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"def identity: Fn(Any) -> Any = fn(value) { value };
               def data = { text: "line\nnext", items: [1, 'Ok, (2,)] };
               def observed = dbg!(data, "loaded\nvalue");
               def seen_identity = dbg!(identity);
               def seen_value = dbg!(observed);
               def whole_float = dbg!(3.0);
               def negative_zero = dbg!(-0.0);
               export def output = if seen_identity == identity { seen_value } else { data };"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        let engine = Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
            data_limits: DataLimits::default(),
        })
        .with_debug_sink(sink.clone());
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        assert_eq!(
            named_output(&engine.execute(&module).unwrap()).to_string(),
            "{items: [1, 'Ok, (2)], text: \"line\\nnext\"}"
        );
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].message.as_deref(), Some("loaded\nvalue"));
        assert_eq!(events[0].name, "data");
        assert_eq!(events[0].module, "standalone/main");
        assert_eq!(events[0].line, 3);
        assert_eq!(
            events[0].repr,
            "{items: [1, 'Ok, (2)], text: \"line\\nnext\"}"
        );
        assert_eq!(events[1].name, "identity");
        assert!(events[1].repr.starts_with("<fn-ref "));
        assert_eq!(events[2].name, "observed");
        assert_eq!(events[2].repr, events[0].repr);
        assert_eq!(events[3].name, "3.0");
        assert_eq!(events[3].repr, "3.0");
        assert_eq!(events[4].name, "-0.0");
        assert_eq!(events[4].repr, "-0.0");
        drop(events);

        fs::write(
            directory.join("bad-message.telora"),
            r#"def message = "dynamic"; export def output = dbg!(42, message);"#,
        )
        .unwrap();
        let bad = engine
            .load_module(directory.join("bad-message.telora"), BTreeMap::new())
            .unwrap_err();
        assert!(bad.to_string().contains("String literal"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn runtime_debug_does_not_emit_during_bootstrap_analysis() {
        let directory = fixture_dir();
        let sink = Arc::new(CapturingDebugSink::default());
        let engine = Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
            data_limits: DataLimits::default(),
        })
        .with_debug_sink(sink.clone());

        for (name, type_binding) in [
            ("without-type.telora", ""),
            ("with-type.telora", "type Number = Int;"),
        ] {
            let path = directory.join(name);
            fs::write(
                &path,
                format!(
                    "{type_binding}\ndef value = 1;\ndef observed = dbg!(value);\nexport def output = \"ok\";"
                ),
            )
            .unwrap();
            let before = sink.events.lock().unwrap().len();
            let module = engine.load_module(path, BTreeMap::new()).unwrap();
            assert_eq!(sink.events.lock().unwrap().len(), before, "{name}");
            assert_eq!(
                named_output(&engine.execute(&module).unwrap()).to_string(),
                "\"ok\""
            );
            assert_eq!(sink.events.lock().unwrap().len(), before + 1, "{name}");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn contextual_debug_is_outside_telora_fuel_and_allocation() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"export def output = dbg!(42, "answer");"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        let engine = Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
            data_limits: DataLimits::default(),
        })
        .with_debug_sink(sink.clone());
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        let initial_events = sink.events.lock().unwrap().len();
        let mut exact = QuotaAccount::new(Quota::new(0, 1_000, u64::MAX));
        let arena = Vm::new()
            .with_debug_sink(sink.clone())
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut exact,
            )
            .unwrap();
        let output = crate::ExecutionWorld::new(Arc::clone(&module.runtime.main.heap), arena);
        assert_eq!(named_output(&output).to_string(), "42");
        assert_eq!(sink.events.lock().unwrap().len(), initial_events + 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn metadata_only_helpers_are_erased_but_runtime_helpers_are_retained() {
        let directory = fixture_dir();
        fs::write(
            directory.join("erased.telora"),
            r#"def observe: Fn(Any) -> Any = fn(value) { dbg!(value, "metadata") };
               type Observed = observe(Int);
               0"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        let erased = load_module_with_quota_and_debug_sink(
            directory.join("erased.telora"),
            BTreeMap::new(),
            Quota::with_fuel(100_000),
            sink.clone(),
        )
        .unwrap();
        assert_eq!(sink.events.lock().unwrap().len(), 1);
        assert_eq!(
            erased
                .execute_with_quota(Quota::new(0, 1_000, 0))
                .unwrap()
                .to_string(),
            "0"
        );
        assert_eq!(sink.events.lock().unwrap().len(), 1);

        fs::write(
            directory.join("retained.telora"),
            r#"def observe: Fn(Any) -> Any = fn(value) { dbg!(value, "observed") };
               type Observed = observe(Int);
               observe(1)"#,
        )
        .unwrap();
        let retained = load_module_with_quota_and_debug_sink(
            directory.join("retained.telora"),
            BTreeMap::new(),
            Quota::with_fuel(100_000),
            sink.clone(),
        )
        .unwrap();
        assert_eq!(sink.events.lock().unwrap().len(), 2);
        retained
            .execute_with_quota_and_debug_sink(Quota::with_fuel(2), sink.clone())
            .unwrap();
        assert_eq!(sink.events.lock().unwrap().len(), 3);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bootstrap_shadow_does_not_consume_the_module_initialization_quota() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"type Observed = dbg!(Int);
               0"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        load_module_with_quota_and_debug_sink(
            directory.join("main.telora"),
            BTreeMap::new(),
            Quota::new(1, 1_000, u64::MAX),
            sink.clone(),
        )
        .unwrap();
        assert_eq!(
            sink.events.lock().unwrap().len(),
            1,
            "only authoritative MetadataInit is observable and charged"
        );
        fs::remove_dir_all(directory).unwrap();
    }
