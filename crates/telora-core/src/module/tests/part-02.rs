    #[test]
    fn imports_recursive_function_roots_from_the_persistent_world() {
        let directory = fixture_dir();
        fs::write(
            directory.join("countdown.telora"),
            "def countdown: Fn(Int) -> Int = fn(n) { if n < 1 { 0 } else { countdown(n - 1) } }; countdown",
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            "import \"./countdown\" as countdown; countdown(4)",
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.execute(100_000).unwrap().to_string(), "0");
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn recursive_function_roots_capture_imported_bindings() {
        let directory = fixture_dir();
        fs::write(directory.join("base.telora"), "2").unwrap();
        fs::write(
            directory.join("countdown.telora"),
            "import \"./base\" as base; def countdown: Fn(Int) -> Int = fn(n) { if n < 1 { base } else { countdown(n - 1) } }; countdown",
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            "import \"./countdown\" as countdown; countdown(4)",
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.execute(100_000).unwrap().to_string(), "2");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn annotation_error_labels_json_data_and_telora_type_declaration() {
        let directory = fixture_dir();
        fs::write(directory.join("user.json"), r#"{"name":"Ada","age":"old"}"#).unwrap();
        fs::write(
            directory.join("main.telora"),
            "import \"./user.json\" { data as user };\n\
             import \"std/codec\" as codec;\n\
             import \"std/result\" as result;\n\
             type User = struct {name: String, age: Int};\n\
             let checked = codec.decode(User, user) |> result.unwrap;\n\
             checked",
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("user.json:1:21"), "{message}");
        assert!(message.contains("standalone/main:4:"), "{message}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn module_execution_uses_evaluation_fuel_semantics() {
        let directory = fixture_dir();
        fs::write(directory.join("straight.telora"), "40 + 2").unwrap();
        let straight = load_module(directory.join("straight.telora"), BTreeMap::new(), 0).unwrap();
        assert_eq!(straight.execute(0).unwrap().to_string(), "42");

        fs::write(
            directory.join("call.telora"),
            "let identity = fn(value) { value }; identity(42)",
        )
        .unwrap();
        let call = load_module(directory.join("call.telora"), BTreeMap::new(), 0).unwrap();
        assert_eq!(
            call.execute(0).unwrap_err().kind,
            crate::RuntimeErrorKind::FuelExhausted
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn engine_applies_module_and_session_quotas_at_separate_boundaries() {
        let directory = fixture_dir();
        fs::write(
            directory.join("typed.telora"),
            "type First = Array(Int); type Second = Array(Int); export def output = 0;",
        )
        .unwrap();
        let module_limited = Engine::new(EngineConfig {
            module_quota: Quota::new(1, 1_000, u64::MAX),
            session_quota: Quota::new(100, 1_000, u64::MAX),
            data_limits: DataLimits::default(),
        });
        let error = module_limited
            .load_module(directory.join("typed.telora"), BTreeMap::new())
            .unwrap_err();
        assert!(error.message().contains("fuel"));

        fs::write(directory.join("value.telora"), "export def output = [1];").unwrap();
        let session_limited = Engine::new(EngineConfig {
            module_quota: Quota::new(100, 1_000, u64::MAX),
            session_quota: Quota::new(100, 1_000, 0),
            data_limits: DataLimits::default(),
        });
        let module = session_limited
            .load_module(directory.join("value.telora"), BTreeMap::new())
            .unwrap();
        assert_eq!(
            session_limited.execute(&module).unwrap_err().kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );
        assert_eq!(
            session_limited.execute(&module).unwrap_err().kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ready_module_root_is_promoted_once_into_the_shared_world() {
        let directory = fixture_dir();
        let data = directory.join("data.json");
        fs::write(&data, r#"{"name":"Ada","items":[1,2,3]}"#).unwrap();
        let mut main = MainWorld::building();
        let mut sources = SourceDatabase::default();
        let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
        let builtin_modules = install_native_modules(&mut main, &mut sources, &debug_sink).unwrap();
        let mut loader = ModuleLoader {
            resolver: ModuleResolver::for_root(&data).unwrap(),
            cache: HashMap::new(),
            builtin_modules,
            main,
            visiting: Vec::new(),
            dependencies: BTreeSet::new(),
            module_quota: Quota::with_fuel(100_000),
            data_limits: DataLimits::default(),
            debug_sink,
            sources,
            semantic_inputs: BTreeMap::new(),
            source_policy: ModuleSourcePolicy::ExpressionHarness,
        };

        let first = loader.load_value(&data).unwrap();
        let counts = loader.main.heap.counts();
        let data_id = loader.resolver.resolve_root(&data).unwrap().id;
        let first_root = match loader.cache.get(&data_id).unwrap() {
            ModuleState::Ready(artifact) => artifact.root,
        };
        let second = loader.load_value(&data).unwrap();
        let second_root = match loader.cache.get(&data_id).unwrap() {
            ModuleState::Ready(artifact) => artifact.root,
        };

        assert_eq!(first.root, second.root);
        assert_eq!(first_root, second_root);
        assert_eq!(counts, loader.main.heap.counts());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sessions_use_fresh_work_worlds_and_leave_frozen_main_unchanged() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays; arrays.map([1, 2], fn(x) { x + 1 })"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let main_counts = module.runtime.main.heap.counts();
        assert!(
            main_counts.0 > 0,
            "builtin modules must be installed in Main"
        );

        assert_eq!(module.execute(100_000).unwrap().to_string(), "[2, 3]");
        assert_eq!(module.execute(100_000).unwrap().to_string(), "[2, 3]");
        assert_eq!(module.runtime.main.heap.counts(), main_counts);
        fs::remove_dir_all(directory).unwrap();
    }
