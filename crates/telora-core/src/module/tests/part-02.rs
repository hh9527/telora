    #[test]
    fn codec_accepts_user_computed_canonical_type_metadata() {
        let directory = fixture_dir();
        fs::write(directory.join("data.json"), r#"{"v":"plain"}"#).unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./data.json" { data };
               import "std/codec" as codec;
               import "std/result" as result;
               type StringRule = {kind: 'String};
               type UserRule = {kind: 'Struct, fields: {v: StringRule}};
               codec.decode(UserRule, data) |> result.unwrap"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{v: \"plain\"}"
        );

        fs::write(
            directory.join("legacy.telora"),
            r#"import "std/result" as result; result.unwrap('Err("legacy"))"#,
        )
        .unwrap();
        let legacy = load_module(directory.join("legacy.telora"), BTreeMap::new(), 100_000)
            .unwrap()
            .execute(100_000)
            .unwrap_err();
        assert_eq!(legacy.message, "legacy");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codec_encode_is_closed_over_structural_containers_of_declared_values() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               import "std/value" {Value};
               type Val = enum {'String(String), 'Int(Int)};
               type Query = struct {
                   sql: String,
                   bindings: Array(Val),
               };
               let first: Query = {sql: "SELECT ?", bindings: ['Int(1)]};
               let second: Query = {sql: "SELECT ?", bindings: ['String("two")]};
               let queries: Array(Query) = [first, second];
               let nested: Array(Array(Query)) = [[first], [second]];
               let indexed: Dict(Query) = {first: first, second: second};
               let maybe: Option(Query) = 'Some(first);
               codec.encode(Value, {queries, nested, indexed, maybe})
                   |> result.unwrap
                   |> json.stringify"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        for expected in ["queries", "nested", "indexed", "maybe", "SELECT ?", "two"] {
            assert!(output.contains(expected), "{output}");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codec_rejects_struct_shape_errors_and_json_is_strict() {
        let directory = fixture_dir();
        let cases = [
            (
                r#"import "std/codec" as codec;
                   import "std/result" as result;
                   type T = struct {name: String};
                   codec.decode(T, codec.encode(codec.Value, {}) |> result.unwrap) |> result.unwrap"#,
                "$.name: missing required field",
            ),
            (
                r#"import "std/codec" as codec;
                   import "std/result" as result;
                   type T = struct {name: String};
                   codec.decode(T, codec.encode(codec.Value, {name: "Ada", extra: 1}) |> result.unwrap) |> result.unwrap"#,
                "$.extra: unknown field",
            ),
            (
                r#"import "std/codec" as codec;
                   import "std/result" as result;
                   codec.encode(codec.Value, (1, 2)) |> result.unwrap"#,
                "unsupported Tuple",
            ),
            (
                r#"import "std/json" as json; json.stringify_pretty(17)"#,
                "indent must be between 0 and 16",
            ),
        ];
        for (index, (source, expected)) in cases.into_iter().enumerate() {
            let path = directory.join(format!("case-{index}.telora"));
            fs::write(&path, source).unwrap();
            let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
            let failure = module.execute(100_000).unwrap_err();
            assert!(failure.message.contains(expected), "{}", failure.message);
        }

        let path = directory.join("compact.telora");
        fs::write(
            &path,
            r#"import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               let value = codec.encode(codec.Value, {z: [1, 'True], a: "line\nnext"}) |> result.unwrap;
               json.stringify(value)"#,
        )
        .unwrap();
        let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            r#""{\"a\":\"line\\nnext\",\"z\":[1,true]}""#
        );
        assert_eq!(
            module
                .execute_with_quota(Quota::new(100_000, 1_000, 1))
                .expect_err("allocation must be exhausted")
                .kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loads_json_and_telora_modules_with_types() {
        let directory = fixture_dir();
        fs::write(directory.join("user.json"), r#"{"name":"Ada","age":36}"#).unwrap();
        fs::write(directory.join("answer.telora"), "40 + 2").unwrap();
        fs::write(
            directory.join("main.telora"),
            "import \"./user.json\" { data as user };\
             import \"./answer\" as answer;\
             import \"std/codec\" as codec;\
             import \"std/result\" as result;\
             type User = struct {name: String, age: Int};\
             let checked = codec.decode(User, user) |> result.unwrap;\
             (checked.name, answer)",
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.dependencies.len(), 3);
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(\"Ada\", 42)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loads_toml_modules_with_temporal_tags_and_reuses_resolved_identity() {
        let directory = fixture_dir();
        fs::write(
            directory.join("config.toml"),
            r#"title = "Telora"
released = 2026-08-04
[environment]
PATH = "/bin"
[[tools]]
name = "telora"
[[tools]]
name = "rustc"
"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./config.toml" { data as config };
               import "./sub/../config.toml" { data as same };
               import "std/codec" as codec;
               import "std/result" as result;
               type TomlDate = enum {
                   'OffsetDateTime(String),
                   'LocalDateTime(String),
                   'LocalDate(String),
                   'LocalTime(String),
               };
               type Tool = struct {name: String};
               type Config = struct {
                   title: String,
                   released: TomlDate,
                   environment: Dict(String),
                   tools: Array(Tool),
               };
               let checked = codec.decode(Config, config) |> result.unwrap;
               let same_checked = codec.decode(Config, same) |> result.unwrap;
               (checked.released, checked.tools, same_checked.title)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.dependencies.len(), 2);
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "('LocalDate(\"2026-08-04\"), [{name: \"telora\"}, {name: \"rustc\"}], \"Telora\")"
        );
        let toml = module
            .workspace
            .module_by_path(&canonicalize(&directory.join("config.toml")).unwrap())
            .unwrap();
        assert_eq!(toml.kind, WorkspaceModuleKind::Toml);
        assert_eq!(toml.state, WorkspaceModuleState::Available);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn toml_annotation_errors_label_data_and_type_declaration() {
        let directory = fixture_dir();
        fs::write(
            directory.join("user.toml"),
            "name = \"Ada\"\nage = \"old\"\n",
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            "import \"./user.toml\" { data as user };\n\
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
        assert!(message.contains("user.toml:2:7"), "{message}");
        assert!(message.contains("standalone/bin/main:4:"), "{message}");

        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/result" as result;
               import "./user.toml" { data as user };
               type User = struct {name: String, age: Int};
               codec.decode(User, user) |> result.unwrap"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        let rendered = error.with_sources(&module.sources).to_string();
        assert!(rendered.contains("user.toml:2:7:"), "{rendered}");
        assert!(rendered.contains("standalone/bin/main:4:"), "{rendered}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_retains_invalid_toml_source_and_diagnostics() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let config = directory.join("config.toml");
        fs::write(&config, "name = \"first\"\nname = \"second\"\n").unwrap();
        fs::write(&main, "import \"./config.toml\" { data as config }; config").unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let config = snapshot
            .module_by_path(&canonicalize(&config).unwrap())
            .unwrap();
        assert_eq!(config.kind, WorkspaceModuleKind::Toml);
        assert_eq!(config.state, WorkspaceModuleState::Available);
        let source = config.source.expect("invalid TOML source is retained");
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.message.contains("duplicate TOML key")
                && diagnostic
                    .labels
                    .first()
                    .is_some_and(|label| label.location.source == source)
        }));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loads_yaml_modules_and_retains_invalid_workspace_source() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let config = directory.join("config.yaml");
        fs::write(
            &config,
            "name: Telora\nfeatures:\n  - static data\n  - provenance\nlegacy: yes\n",
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./config.yaml" { data as config };
               import "std/codec" as codec;
               import "std/result" as result;
               type Config = struct {
                   name: String,
                   features: Array(String),
                   legacy: String,
               };
               let checked = codec.decode(Config, config) |> result.unwrap;
               (checked.name, checked.features, checked.legacy)"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(\"Telora\", [\"static data\", \"provenance\"], \"yes\")"
        );
        let yaml = module
            .workspace
            .module_by_path(&canonicalize(&config).unwrap())
            .unwrap();
        assert_eq!(yaml.kind, WorkspaceModuleKind::Yaml);
        assert_eq!(yaml.state, WorkspaceModuleState::Available);

        fs::write(&config, "name: first\nname: second\n").unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let yaml = snapshot
            .module_by_path(&canonicalize(&config).unwrap())
            .unwrap();
        assert_eq!(yaml.kind, WorkspaceModuleKind::Yaml);
        assert_eq!(yaml.state, WorkspaceModuleState::Available);
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message.contains("duplicate YAML key"))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_module_cycles() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            "import \"./a\" as a; a",
        )
        .unwrap();
        fs::write(directory.join("a.telora"), "import \"./b\" as b; b").unwrap();
        fs::write(directory.join("b.telora"), "import \"./a\" as a; a").unwrap();
        let error =
            load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.message().contains("cycle"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_non_std_and_nested_native_declarations_with_locations() {
        let directory = fixture_dir();
        fs::write(
            directory.join("native-value.telora"),
            "native missing: Fn(Int) -> Int; missing(1)",
        )
        .unwrap();
        let missing = load_module(
            directory.join("native-value.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(missing.message().contains("only allowed in built-in std modules"));
        assert!(
            missing
                .to_string()
                .contains("standalone/bin/native-value:1:1")
        );
        let recovered = recovery_engine()
            .recover_workspace(directory.join("native-value.telora"))
            .unwrap();
        assert!(recovered.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("only allowed in built-in std modules")
        }));

        fs::write(
            directory.join("native-type.telora"),
            "native type State @1; State",
        )
        .unwrap();
        let missing_type = load_module(
            directory.join("native-type.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(
            missing_type
                .message()
                .contains("only allowed in built-in std modules")
        );
        assert!(
            missing_type
                .to_string()
                .contains("standalone/bin/native-type:1:1")
        );

        fs::write(
            directory.join("nested-native.telora"),
            "let value = { native hidden: Fn(Int) -> Int; 1 }; value",
        )
        .unwrap();
        let nested = load_module(
            directory.join("nested-native.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(
            nested
                .message()
                .contains("only allowed at module top level")
        );
        fs::remove_dir_all(directory).unwrap();
    }

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
    fn external_input_is_any_and_available_at_runtime() {
        let directory = fixture_dir();
        fs::write(directory.join("main.telora"), "input").unwrap();
        let input = parse_json("input", r#"{"value":42}"#).unwrap();
        let module = load_module(
            directory.join("main.telora"),
            BTreeMap::from([("input".into(), input)]),
            100_000,
        )
        .unwrap();
        assert_eq!(
            module
                .analysis
                .types
                .node(module.analysis.binding_types["input"]),
            &crate::TypeNode::Any
        );
        assert_eq!(module.execute(100_000).unwrap().to_string(), "{value: 42}");
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
        assert!(message.contains("standalone/bin/main:4:"), "{message}");
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

    #[test]
    fn core_array_module_runs_higher_order_operations_and_nested_callbacks() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               let values = [1, 2, 3];
               let empty: Array(Int) = [];
               let empty_strings: Array(String) = [];
               {
                   length: arrays.length(values),
                   first: arrays.get(values, 0),
                   last: arrays.get(values, 2),
                   negative: arrays.get(values, -1),
                   out_of_range: arrays.get(values, 3),
                   empty_get: arrays.get(empty, 0),
                   enumerated: arrays.enumerate(values),
                   enumerated_empty: arrays.enumerate(empty),
                   pushed: arrays.push(values, 4),
                   pushed_empty: arrays.push(empty, 1),
                   original: values,
                   zipped: arrays.zip(values, ["a", "b", "c"]),
                   zip_mismatch: arrays.zip(values, ["a"]),
                   zip_empty: arrays.zip(empty, empty_strings),
                   mapped: arrays.map(values, fn(value) { value + 10 }),
                   filtered: arrays.filter(values, fn(value) { 1 < value }),
                   flattened: arrays.flat_map(values, fn(value) { [value, value] }),
                   folded: arrays.fold(values, 0, fn(total, value) { total + value }),
                   controlled: arrays.fold_control@[Int, Int, String](
                       values,
                       0,
                       fn(total, value) { 'Continue(total + value) },
                   ),
                   controlled_break: arrays.fold_control@[Int, Int, String](
                       [1, 0],
                       0,
                       fn(total, value) {
                           if 0 < value { 'Break("done") } else { 'Continue(total + 1 / value) }
                       },
                   ),
                   controlled_empty: arrays.fold_control@[Int, Int, String](
                       empty,
                       42,
                       fn(total, value) { 'Continue(total + 1 / value) },
                   ),
                   empty_map: arrays.map(empty, fn(value) { value / 0 }),
                   empty_filter: arrays.filter(empty, fn(unused) { 'True }),
                   empty_flat_map: arrays.flat_map(empty, fn(value) { [value] }),
                   empty_fold: arrays.fold(empty, 42, fn(total, value) { total + value }),
                   nested: arrays.map(values, fn(value) {
                       arrays.fold([value, value], 0, fn(total, item) { total + item })
                   }),
                   pipelined: values |> arrays.map\(_, fn(value) { value + 20 }),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let value = module.execute(100_000).unwrap();
        let result = value.value();
        assert_eq!(result.get("length").unwrap().to_string(), "3");
        assert_eq!(result.get("first").unwrap().to_string(), "'Some(1)");
        assert_eq!(result.get("last").unwrap().to_string(), "'Some(3)");
        assert_eq!(result.get("negative").unwrap().to_string(), "'None");
        assert_eq!(result.get("out_of_range").unwrap().to_string(), "'None");
        assert_eq!(result.get("empty_get").unwrap().to_string(), "'None");
        assert_eq!(
            result.get("enumerated").unwrap().to_string(),
            "[(0, 1), (1, 2), (2, 3)]"
        );
        assert_eq!(result.get("enumerated_empty").unwrap().to_string(), "[]");
        assert_eq!(result.get("pushed").unwrap().to_string(), "[1, 2, 3, 4]");
        assert_eq!(result.get("pushed_empty").unwrap().to_string(), "[1]");
        assert_eq!(result.get("original").unwrap().to_string(), "[1, 2, 3]");
        assert_eq!(
            result.get("zipped").unwrap().to_string(),
            "'Some([(1, \"a\"), (2, \"b\"), (3, \"c\")])"
        );
        assert_eq!(result.get("zip_mismatch").unwrap().to_string(), "'None");
        assert_eq!(result.get("zip_empty").unwrap().to_string(), "'Some([])");
        assert_eq!(result.get("mapped").unwrap().to_string(), "[11, 12, 13]");
        assert_eq!(result.get("filtered").unwrap().to_string(), "[2, 3]");
        assert_eq!(
            result.get("flattened").unwrap().to_string(),
            "[1, 1, 2, 2, 3, 3]"
        );
        assert_eq!(result.get("folded").unwrap().to_string(), "6");
        assert_eq!(
            result.get("controlled").unwrap().to_string(),
            "'Continue(6)"
        );
        assert_eq!(
            result.get("controlled_break").unwrap().to_string(),
            "'Break(\"done\")"
        );
        assert_eq!(
            result.get("controlled_empty").unwrap().to_string(),
            "'Continue(42)"
        );
        assert_eq!(result.get("empty_map").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_filter").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_flat_map").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_fold").unwrap().to_string(), "42");
        assert_eq!(result.get("nested").unwrap().to_string(), "[2, 4, 6]");
        assert_eq!(result.get("pipelined").unwrap().to_string(), "[21, 22, 23]");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_fold_widens_singleton_enum_fields_in_callback_results() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               type Kind = enum {
                   'Missing,
                   'Unauthorized,
               };
               type Rejection = struct {kind: Kind};
               let initial: Array(Rejection) = [];
               array.fold([1, 2], initial, fn(rejections, value) {
                   let rejection: Rejection = if value == 1 {
                       {kind: 'Missing}
                   } else {
                       {kind: 'Unauthorized}
                   };
                   array.push(rejections, rejection)
               })"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "Array<Rejection>"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "[{kind: 'Missing}, {kind: 'Unauthorized}]"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_fold_defers_singleton_evidence_until_declared_result_widens_it() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               def collect: Fn(Array(Int)) -> Array(Option(Int)) =
                   fn(values) {
                       let options = array.fold(values, [], fn(options, value) {
                           if value == 1 {
                               array.push(options, 'None)
                           } else {
                               array.push(options, 'Some(value))
                           }
                       });
                       options
                   };
               collect([1, 2])"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let inferred = module.analysis.display(module.analysis.result_type);
        assert_eq!(inferred, "Array<enum {None, Some(Int)}>");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "['None, 'Some(2)]"
        );

        fs::write(
            directory.join("types.telora"),
            r#"type Kind = enum {
                   'Missing,
                   'Unauthorized,
               };
               export { Kind };"#,
        )
        .unwrap();
        fs::write(
            directory.join("records.telora"),
            r#"import "std/array" as array;
               import "./types" { Kind };
               type Rejection(Subject) = struct {kind: Kind, subject: Subject};
               def reject_all: for(Subject)
                   Fn(Array(Int), Subject) -> Array(Rejection(Subject)) =
                   fn(values, subject) {
                       let rejections = array.fold(values, [], fn(rejections, value) {
                           let rejection: Rejection(Subject) = if value == 1 {
                               {kind: 'Missing, subject: subject}
                           } else {
                               {kind: 'Unauthorized, subject: subject}
                           };
                           array.push(rejections, rejection)
                       });
                       rejections
                   };
               reject_all([2, 1], "subject")"#,
        )
        .unwrap();
        let records =
            load_module(directory.join("records.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            records.analysis.display(records.analysis.result_type),
            "Array<Rejection>"
        );
        assert_eq!(
            records.execute(100_000).unwrap().to_string(),
            "[{kind: 'Unauthorized, subject: \"subject\"}, {kind: 'Missing, subject: \"subject\"}]"
        );

        fs::write(
            directory.join("invalid.telora"),
            r#"import "std/array" as array;
               def collect: Fn(Array(Int)) -> Array(Option(Int)) =
                   fn(values) {
                       let options = array.fold(values, [], fn(options, value) {
                           if value == 1 {
                               array.push(options, 'None)
                           } else {
                               array.push(options, 'Missing)
                           }
                       });
                       options
                   };
               collect([1, 2])"#,
        )
        .unwrap();
        let error =
            load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("'Missing"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_fold_infers_anonymous_state_from_computed_array_fields() {
        let directory = fixture_dir();
        let source = r#"import "std/array" as array;
               type Report(A) = struct {value: A, accepted: Bool};
               type CollectResult(A) = struct {
                   reports: Array(Report(A)),
                   diagnostics: Array(String),
               };
               def collect: for(A)
                   Fn(Array(A), Array(String)) -> CollectResult(A) =
                   fn(values, prior) {
                       let initial: CollectResult(A) = {reports: [], diagnostics: prior};
                       array.fold(values, initial, fn(acc, value) {
                           let next: CollectResult(A) = if value == value {
                               {reports: array.push(acc.reports, {value, accepted: 'True}),
                                diagnostics: acc.diagnostics}
                           } else {
                               {reports: array.push(acc.reports, {value, accepted: 'False}),
                                diagnostics: array.push(acc.diagnostics, "rejected")}
                           }; next
                       })
                   };
               collect([1, 2], [])"#;
        fs::write(directory.join("main.telora"), source).unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let expected = module.analysis.display(module.analysis.result_type);
        assert_eq!(expected, "CollectResult");

        let reversed = source
            .replace("if value == value", "if value != value")
            .replace(
                "{reports: array.push(acc.reports, {value, accepted: 'True}),\n                                diagnostics: acc.diagnostics}",
                "{diagnostics: acc.diagnostics,\n                                reports: array.push(acc.reports, {accepted: 'True, value})}",
            )
            .replace(
                "{reports: array.push(acc.reports, {value, accepted: 'False}),\n                                diagnostics: array.push(acc.diagnostics, \"rejected\")}",
                "{diagnostics: array.push(acc.diagnostics, \"rejected\"),\n                                reports: array.push(acc.reports, {accepted: 'False, value})}",
            );
        fs::write(directory.join("reversed.telora"), reversed).unwrap();
        let reversed =
            load_module(directory.join("reversed.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            reversed.analysis.display(reversed.analysis.result_type),
            expected
        );

        let incompatible =
            source.replace("Fn(Array(A), Array(String))", "Fn(Array(A), Array(Int))");
        fs::write(directory.join("incompatible.telora"), incompatible).unwrap();
        let error = load_module(
            directory.join("incompatible.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(error.message.contains("String"), "{}", error.message);
        assert!(!error.message.contains(" T"), "{}", error.message);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn function_contracts_resolve_qualified_imported_type_paths() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"type Input = struct {value: Int};
               type Item = struct {name: String};
               type Output = struct {count: Int};
               export { Input, Item, Output };"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./types" as types;
               def consume:
                   Fn(types.Input, Array(types.Item)) -> types.Output =
                   fn(input, items) { {count: input.value} };
               consume({value: 2}, [{name: "first"}])"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "Output"
        );
        assert_eq!(module.execute(100_000).unwrap().to_string(), "{count: 2}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_fold_widens_atom_fields_from_callback_results() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               let computed = array.fold(
                   [1, 2, 3],
                   {flag: 'False, items: []},
                   fn(state, item) {
                       {flag: item > 1 || state.flag,
                        items: array.push(state.items, item)}
                   },
               );
               let branched = array.fold(
                   [1, 2],
                   {flag: 'False, items: []},
                   fn(state, item) {
                       if item > 1 {
                           {flag: 'True, items: array.push(state.items, item)}
                       } else {
                           {flag: 'False, items: array.push(state.items, item)}
                       }
                   },
               );
               (computed, branched)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "({flag: enum {False, True}, items: Array<Int>}, {flag: 'False, items: Array<Int>} | {flag: 'True, items: Array<Int>})"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "({flag: 'True, items: [1, 2, 3]}, {flag: 'True, items: [1, 2]})"
        );

        fs::write(
            directory.join("invalid.telora"),
            r#"import "std/array" as array;
               array.fold([1], {flag: 'False}, fn(state, item) {
                   {flag: 'Foreign}
               })"#,
        )
        .unwrap();
        let error =
            load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("Foreign"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_map_widens_singleton_option_arm_in_generic_callback() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               def lower_all: for(Id, Input, Output, Capability)
                   Fn(
                       Array(Id),
                       Fn(Id) -> Option(Capability),
                       Fn(Capability) -> Fn(Id, Input) -> Option(Output),
                       Input,
                   ) -> Array(Option(Output)) =
                   fn(ids, find, lower, input) {
                       array.map(ids, fn(id) {
                           match find(id) {
                               'Some(capability) => lower(capability)(id, input),
                               'None => 'None,
                           }
                       })
                   };
               lower_all"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "Fn(Array<Any>, Fn(Any) -> enum {None, Some(Any)}, Fn(Any) -> Fn(Any, Any) -> enum {None, Some(Any)}, Any) -> Array<enum {None, Some(Any)}>"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_map_widens_option_fields_across_nested_generic_matches() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               type Capability(Id, Output) = struct {
                   id: Id,
                   lower: Fn(Id) -> Option(Output),
               };
               def collect: for(Id, Output)
                   Fn(Array(Capability(Id, Output)), Array(Id)) -> Array(Option(Output)) =
                   fn(catalog, requests) {
                       let steps = array.map(requests, fn(requested) {
                           match array.find(catalog, fn(capability) {
                               capability.id == requested
                           }) {
                               'Some(capability) => match capability.lower(requested) {
                                   'Some(value) => {
                                       evidence: 'Some(value),
                                       error: 'None,
                                   },
                                   'None => {
                                       evidence: 'None,
                                       error: 'Some("lowering failed"),
                                   },
                               },
                               'None => {
                                   evidence: 'None,
                                   error: 'Some("missing"),
                               },
                           }
                       });
                       array.map(steps, fn(step) { step.evidence })
                   };
               type Id = enum {'A, 'B};
               def lower_a: Fn(Id) -> Option(Int) = fn(id) {
                   if id == 'A { 'Some(1) } else { 'None }
               };
               let catalog: Array(Capability(Id, Int)) = [{id: 'A, lower: lower_a}];
               collect(catalog, ['A, 'B])"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "Array<enum {None, Some(Int)}>"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "['Some(1), 'None]"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deterministic_array_string_and_path_modules_cover_plan_composition() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               import "std/path" as paths;
               import "std/string" as strings;
               {
                   concat: arrays.concat([[1, 2], [], [3]]),
                   any: arrays.any([1, 0], fn(value) {
                       if 0 < value { 'True } else { value / 0 < 1 }
                   }),
                   all: arrays.all([0, 1], fn(value) {
                       if value < 1 { 'False } else { value / 0 < 1 }
                   }),
                   found: arrays.find([1, 2, 3], fn(value) { 1 < value }),
                   missing: arrays.find([1], fn(value) { value < 0 }),
                   empty_any: arrays.any([], fn(value) { value / 0 < 1 }),
                   empty_all: arrays.all([], fn(value) { value / 0 < 1 }),
                   chars: strings.length("形态a"),
                   joined: strings.join(["a", "形", "c"], ":"),
                   split: strings.split("a::形", ":"),
                   scalar_split: strings.split("a形", ""),
                   starts: strings.starts_with("形态", "形"),
                   ends: strings.ends_with("telora", "ra"),
                   contains: strings.contains("telora", "lor"),
                   replaced: strings.replace("a-b-a", "a", "xy"),
                   lines: strings.lines("a\r\nb\n"),
                   joined_lines: strings.join_lines(["a", "形", "c"]),
                   indented: strings.indent("a\n\nb", 2),
                   trailing: strings.ensure_trailing_newline("a"),
                   margin: strings.trim_margin(r"  |a
    |b
unchanged", "|"),
                   normalized: paths.normalize("/a/./b/../../../../c"),
                   relative: paths.normalize("../../a/../b"),
                   joined_path: paths.join(["/tool", "bin", "../lib", "gcc"]),
                   restarted: paths.join(["ignored", "/absolute", "file"]),
                   parent: paths.parent("/a/b/../c"),
                   root_parent: paths.parent("/"),
                   file_name: paths.file_name("a/b/../c"),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let result_world = module.execute(100_000).unwrap();
        let result = result_world.value();
        assert_eq!(result.get("concat").unwrap().to_string(), "[1, 2, 3]");
        assert_eq!(result.get("any").unwrap().to_string(), "'True");
        assert_eq!(result.get("all").unwrap().to_string(), "'False");
        assert_eq!(result.get("found").unwrap().to_string(), "'Some(2)");
        assert_eq!(result.get("missing").unwrap().to_string(), "'None");
        assert_eq!(result.get("empty_any").unwrap().to_string(), "'False");
        assert_eq!(result.get("empty_all").unwrap().to_string(), "'True");
        assert_eq!(result.get("chars").unwrap().to_string(), "3");
        assert_eq!(result.get("joined").unwrap().to_string(), r#""a:形:c""#);
        assert_eq!(
            result.get("split").unwrap().to_string(),
            r#"["a", "", "形"]"#
        );
        assert_eq!(
            result.get("scalar_split").unwrap().to_string(),
            r#"["", "a", "形", ""]"#
        );
        assert_eq!(result.get("starts").unwrap().to_string(), "'True");
        assert_eq!(result.get("ends").unwrap().to_string(), "'True");
        assert_eq!(result.get("contains").unwrap().to_string(), "'True");
        assert_eq!(result.get("replaced").unwrap().to_string(), r#""xy-b-xy""#);
        assert_eq!(
            result.get("lines").unwrap().to_string(),
            r#"["a", "b", ""]"#
        );
        assert_eq!(
            result.get("joined_lines").unwrap().to_string(),
            "\"a\\n形\\nc\""
        );
        assert_eq!(
            result.get("indented").unwrap().to_string(),
            "\"  a\\n\\n  b\""
        );
        assert_eq!(result.get("trailing").unwrap().to_string(), "\"a\\n\"");
        assert_eq!(
            result.get("margin").unwrap().to_string(),
            "\"a\\nb\\nunchanged\""
        );
        assert_eq!(result.get("normalized").unwrap().to_string(), r#""/c""#);
        assert_eq!(result.get("relative").unwrap().to_string(), r#""../../b""#);
        assert_eq!(
            result.get("joined_path").unwrap().to_string(),
            r#""/tool/lib/gcc""#
        );
        assert_eq!(
            result.get("restarted").unwrap().to_string(),
            r#""/absolute/file""#
        );
        assert_eq!(result.get("parent").unwrap().to_string(), r#"'Some("/a")"#);
        assert_eq!(result.get("root_parent").unwrap().to_string(), "'None");
        assert_eq!(
            result.get("file_name").unwrap().to_string(),
            r#"'Some("c")"#
        );

        for (source, expected) in [
            (
                "import \"std/string\" as strings; strings.indent(\"x\", -1)",
                "indentation width must be non-negative",
            ),
            (
                "import \"std/string\" as strings; strings.trim_margin(\"x\", \"\")",
                "margin marker must not be empty",
            ),
        ] {
            fs::write(directory.join("invalid.telora"), source).unwrap();
            let module =
                load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000).unwrap();
            let error = module.execute(100_000).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
        fs::remove_dir_all(directory).unwrap();
    }
