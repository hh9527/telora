    #[test]
    fn imported_generic_apis_refine_option_results_of_let_bound_callbacks() {
        let directory = fixture_dir();
        fs::write(
            directory.join("api.telora"),
            r#"def apply: for(A, B) Fn(A, Fn(A) -> Option(B)) -> Option(B) =
                   fn(value, callback) { callback(value) };
               {apply}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./api" as api;
               let build = fn(value) {
                   if value > 0 { 'Some("ok") } else { 'None }
               };
               api.apply(1, build)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "enum {None, Some(String)}"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "'Some(\"ok\")"
        );
        fs::remove_dir_all(directory).unwrap();
    }


    #[test]
    fn local_concrete_family_dependencies_preserve_metadata_and_match_imports() {
        let directory = fixture_dir();
        fs::write(
            directory.join("attributes.telora"),
            r#"import "std/attributes" as attributes;
               type Local = attributes.add(Int, {marker: "local"});
               type Pair(A) = Tuple([Local, A]);
               {direct: Local, captured: Pair(String)}"#,
        )
        .unwrap();
        let module = load_module(
            directory.join("attributes.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap();
        let result_world = module.execute(100_000).unwrap();
        let result = result_world.value();
        let captured = result.get("captured").unwrap();
        let items = captured.get("items").unwrap();
        assert_eq!(
            items.sequence_get(0).unwrap().to_string(),
            result.get("direct").unwrap().to_string()
        );

        fs::write(
            directory.join("base.telora"),
            "type Status = enum {'Ready}; {Status: Status}",
        )
        .unwrap();
        fs::write(
            directory.join("local.telora"),
            "type Status = enum {'Ready};\
             type Box(A) = struct {status: Status, value: A};\
             {Box: Box}",
        )
        .unwrap();
        fs::write(
            directory.join("imported.telora"),
            "import \"./base\" {Status};\
             type Box(A) = struct {status: Status, value: A};\
             {Box: Box}",
        )
        .unwrap();
        fs::write(
            directory.join("compare.telora"),
            "import \"./local\" as local;\
             import \"./imported\" as imported;\
             (local.Box(Int), imported.Box(Int))",
        )
        .unwrap();
        let module =
            load_module(directory.join("compare.telora"), BTreeMap::new(), 100_000).unwrap();
        let items_world = module.execute(100_000).unwrap();
        let items = items_world.value();
        assert_eq!(
            items.sequence_get(0).unwrap().to_string(),
            items.sequence_get(1).unwrap().to_string()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parameterized_type_family_applications_preserve_authored_rule_provenance() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let data = directory.join("data.json");
        fs::write(&data, r#"{"value":42}"#).unwrap();
        fs::write(
            &main,
            r#"import "./data.json" { data };
               import "std/codec" as codec;
               import "std/result" as result;
               type Box(Item) = struct {value: Item};
               codec.decode(Box(String), data) |> result.unwrap"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module.execute(100_000).unwrap_err();
        assert!(failure.message.contains("$.value"), "{}", failure.message);
        let data_location = failure.data_location().expect("codec data location");
        assert_eq!(
            module.sources.get(data_location.source).name.as_ref(),
            "standalone/data.json"
        );
        let rule_location = failure.rule_location().expect("codec rule location");
        assert_eq!(
            module.sources.get(rule_location.source).name.as_ref(),
            "standalone/main"
        );
        assert!(
            module
                .sources
                .get(rule_location.source)
                .slice(rule_location)
                .is_some_and(|rule| rule.contains("String")),
            "rule location: {rule_location:?}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_type_family_application_interns_the_same_type_identity() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"type Ty(Item) = struct {value: Item};
               type A = Ty(String);
               type B = Ty(String);
               {A: A, B: B}"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.binding_types["A"], module.analysis.binding_types["B"],
            "identical family applications must intern to one AnalysisTypeId",
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_recursive_type_family_drives_codecs_and_schema() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"type Expr(A) = enum {
                   'Leaf(A),
                   'Call(Array(Expr(A))),
               };
               export {Expr};"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./types" {Expr}; export {Expr};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade" as types;
               import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               import "std/value" {Value};
               type IntExpr = types.Expr(Int);
               type SameIntExpr = types.Expr(Int);
               type StringExpr = types.Expr(String);
               let value: IntExpr = 'Call(['Leaf(1), 'Call(['Leaf(2)])]);
               let encoded = codec.encode(Value, value) |> result.unwrap;
               let decoded: IntExpr = codec.decode(IntExpr, encoded) |> result.unwrap;
               {decoded, encoded, schema: json.schema(IntExpr)}"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.binding_types["IntExpr"],
            module.analysis.binding_types["SameIntExpr"]
        );
        assert_ne!(
            module.analysis.binding_types["IntExpr"],
            module.analysis.binding_types["StringExpr"]
        );
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("'Leaf(2)"), "{output}");
        assert!(output.contains("$defs"), "{output}");
        assert!(output.contains("$ref"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_callbacks_share_fuel_allocation_and_tool_stage_execution() {
        let directory = fixture_dir();
        let item_count = 1_500usize;
        let data = format!("[{}]", vec!["1"; item_count].join(","));
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               arrays.map(values, fn(value) { value + 1 })"#,
        )
        .unwrap();
        let module = load_module(
            directory.join("main.telora"),
            BTreeMap::from([("values".into(), parse_json("values.json", &data).unwrap())]),
            100_000,
        )
        .unwrap();

        let mut exact = QuotaAccount::new(Quota::new(1_501, 1_000, u64::MAX));
        let arena = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut exact,
            )
            .unwrap();
        assert_eq!(
            exact.requested_allocation_bytes(),
            item_count as u64 * std::mem::size_of::<Val>() as u64
        );
        assert_eq!(
            arena.root_ref(&module.runtime.main.heap).sequence_len(),
            Some(item_count)
        );

        let mut fuel_short = QuotaAccount::new(Quota::new(1_500, 1_000, u64::MAX));
        assert_eq!(
            Vm::new()
                .execute_in_work(
                    &module.runtime.main.heap,
                    &module.runtime.externals,
                    &module.function,
                    &[],
                    &mut fuel_short,
                )
                .err()
                .expect("fuel must be exhausted")
                .kind,
            crate::RuntimeErrorKind::FuelExhausted
        );

        let requested = item_count as u64 * std::mem::size_of::<Val>() as u64;
        let mut allocation_short = QuotaAccount::new(Quota::new(1_501, 1_000, requested - 1));
        assert_eq!(
            Vm::new()
                .execute_in_work(
                    &module.runtime.main.heap,
                    &module.runtime.externals,
                    &module.function,
                    &[],
                    &mut allocation_short,
                )
                .err()
                .expect("allocation must be exhausted")
                .kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        fs::write(
            directory.join("types.telora"),
            r#"import "std/array" as arrays;
               type Pair = Tuple(arrays.map([Int, String], fn(item) { item }));
               let pair: Pair = (1, "one");
               pair"#,
        )
        .unwrap();
        let types = load_module(directory.join("types.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(types.execute(100_000).unwrap().to_string(), "(1, \"one\")");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_array_reports_boundary_and_callback_result_errors() {
        let directory = fixture_dir();
        let analysis_error = |name: &str, expression: &str| {
            let path = directory.join(name);
            fs::write(
                &path,
                format!("import \"std/array\" as arrays; {expression}"),
            )
            .unwrap();
            load_module(path, BTreeMap::new(), 100_000).unwrap_err()
        };
        let run_error = |name: &str, expression: &str| {
            let path = directory.join(name);
            fs::write(
                &path,
                format!("import \"std/array\" as arrays; {expression}"),
            )
            .unwrap();
            let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
            module.execute(100_000).unwrap_err()
        };

        assert!(
            analysis_error("length.telora", "arrays.length(1)")
                .to_string()
                .contains("cannot unify Int with Array")
        );
        assert!(
            analysis_error("get-index.telora", "arrays.get([1], \"first\")")
                .to_string()
                .contains("cannot unify String with Int")
        );
        assert!(
            analysis_error("enumerate.telora", "arrays.enumerate(1)")
                .to_string()
                .contains("cannot unify Int with Array")
        );
        assert!(
            analysis_error("push.telora", "arrays.push([1], \"wrong\")")
                .to_string()
                .contains("cannot unify")
        );
        assert!(
            analysis_error("arity.telora", "arrays.map([1], fn(a, b) { a + b })")
                .to_string()
                .contains("cannot unify")
        );
        assert!(
            analysis_error("filter.telora", "arrays.filter([1], fn(value) { value })")
                .to_string()
                .contains("cannot unify Int with enum {False, True}")
        );
        assert!(
            analysis_error(
                "flat-map.telora",
                "arrays.flat_map([1], fn(value) { value })"
            )
            .to_string()
            .contains("cannot unify Int with Array")
        );
        let callback = run_error(
            "callback.telora",
            "arrays.map([1], fn(value) { value / 0 })",
        );
        assert!(
            callback
                .to_string()
                .contains("standalone/callback:1:")
        );
        assert!(
            callback
                .trace
                .iter()
                .any(|frame| frame.function == "std/array.map")
        );
        let dynamic_get = run_error(
            "dynamic-get.telora",
            "let index: Any = \"first\"; arrays.get([1], index)",
        );
        assert_eq!(dynamic_get.kind, crate::RuntimeErrorKind::TypeMismatch);
        assert!(dynamic_get.message.contains("Int"));

        let nested_depth = run_error(
            "nested-depth.telora",
            "decl nest: Fn(Int) -> Int;
             def nest = fn(n) {
                 if n < 1 { 0 } else {
                     arrays.fold([n], 0, fn(total, value) { nest(value - 1) })
                 }
             };
             nest(1100)",
        );
        assert_eq!(
            nested_depth.kind,
            crate::RuntimeErrorKind::CallDepthExceeded
        );

        let unknown_path = directory.join("unknown-core.telora");
        fs::write(&unknown_path, "import \"std/unknown\" as unknown; unknown").unwrap();
        assert!(
            load_module(unknown_path, BTreeMap::new(), 100_000)
                .unwrap_err()
                .to_string()
                .contains("module \"std/unknown\" not found")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn array_push_preserves_existing_and_appended_value_provenance() {
        let directory = fixture_dir();
        let data = directory.join("data.json");
        let main = directory.join("main.telora");
        let source = r#"import "std/array" as arrays;
                        import "std/codec" as codec;
                        import "std/result" as result;
                        import "./data.json" { data };
                        let data = codec.decode(Array(Int), data) |> result.unwrap;
                        let values = arrays.push(data, APPENDED);
                        arrays.map(values, fn(value) {
                            if value == TARGET {
                                fail!("selected value", value)
                            } else { value }
                        })"#;

        fs::write(&data, "[1]").unwrap();
        fs::write(
            &main,
            source.replace("APPENDED", "2").replace("TARGET", "1"),
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let existing = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(existing.contains("data.json:1:2:"), "{existing}");

        fs::write(
            &main,
            source.replace("APPENDED", "2").replace("TARGET", "2"),
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let appended = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(appended.contains("standalone/main:6:"), "{appended}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn array_push_charges_the_complete_logical_result() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               arrays.push(values, 3)"#,
        )
        .unwrap();
        let module = load_module(
            directory.join("main.telora"),
            BTreeMap::from([(
                "values".into(),
                parse_json("values.json", "[1, 2]").unwrap(),
            )]),
            100_000,
        )
        .unwrap();
        let requested = 3 * std::mem::size_of::<Val>() as u64;
        let mut exact = QuotaAccount::new(Quota::new(1, 1_000, requested));
        let result = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut exact,
            )
            .unwrap();
        assert_eq!(
            result.root_ref(&module.runtime.main.heap).to_string(),
            "[1, 2, 3]"
        );
        assert_eq!(exact.requested_allocation_bytes(), requested);

        let mut short = QuotaAccount::new(Quota::new(1, 1_000, requested - 1));
        let failure = match Vm::new().execute_in_work(
            &module.runtime.main.heap,
            &module.runtime.externals,
            &module.function,
            &[],
            &mut short,
        ) {
            Ok(_) => panic!("allocation must be exhausted"),
            Err(error) => error,
        };
        assert_eq!(
            failure.kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn array_get_and_enumerate_obey_exact_allocation_and_tool_stage_contracts() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let load = |expression: &str| {
            fs::write(
                &main,
                format!("import \"std/array\" as arrays;\n{expression}"),
            )
            .unwrap();
            load_module(
                &main,
                BTreeMap::from([(
                    "values".into(),
                    parse_json("values.json", "[10, 20]").unwrap(),
                )]),
                100_000,
            )
            .unwrap()
        };
        let value_bytes = std::mem::size_of::<Val>() as u64;

        let some = load("arrays.get(values, 1)");
        let mut exact_some = QuotaAccount::new(Quota::new(1, 1_000, 2 * value_bytes));
        let result = Vm::new()
            .execute_in_work(
                &some.runtime.main.heap,
                &some.runtime.externals,
                &some.function,
                &[],
                &mut exact_some,
            )
            .unwrap();
        assert_eq!(
            result.root_ref(&some.runtime.main.heap).to_string(),
            "'Some(20)"
        );
        assert_eq!(exact_some.requested_allocation_bytes(), 2 * value_bytes);
        let mut short_some = QuotaAccount::new(Quota::new(1, 1_000, 2 * value_bytes - 1));
        let failure = match Vm::new().execute_in_work(
            &some.runtime.main.heap,
            &some.runtime.externals,
            &some.function,
            &[],
            &mut short_some,
        ) {
            Ok(_) => panic!("allocation must be exhausted"),
            Err(error) => error,
        };
        assert_eq!(
            failure.kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        let none = load("arrays.get(values, -1)");
        let mut no_allocation = QuotaAccount::new(Quota::new(1, 1_000, 0));
        let result = Vm::new()
            .execute_in_work(
                &none.runtime.main.heap,
                &none.runtime.externals,
                &none.function,
                &[],
                &mut no_allocation,
            )
            .unwrap();
        assert_eq!(
            result.root_ref(&none.runtime.main.heap).to_string(),
            "'None"
        );
        assert_eq!(no_allocation.requested_allocation_bytes(), 0);

        let enumerate = load("arrays.enumerate(values)");
        let requested = 6 * value_bytes;
        let mut exact_enumerate = QuotaAccount::new(Quota::new(1, 1_000, requested));
        let result = Vm::new()
            .execute_in_work(
                &enumerate.runtime.main.heap,
                &enumerate.runtime.externals,
                &enumerate.function,
                &[],
                &mut exact_enumerate,
            )
            .unwrap();
        assert_eq!(
            result.root_ref(&enumerate.runtime.main.heap).to_string(),
            "[(0, 10), (1, 20)]"
        );
        assert_eq!(exact_enumerate.requested_allocation_bytes(), requested);
        let mut short_enumerate = QuotaAccount::new(Quota::new(1, 1_000, requested - 1));
        let failure = match Vm::new().execute_in_work(
            &enumerate.runtime.main.heap,
            &enumerate.runtime.externals,
            &enumerate.function,
            &[],
            &mut short_enumerate,
        ) {
            Ok(_) => panic!("allocation must be exhausted"),
            Err(error) => error,
        };
        assert_eq!(
            failure.kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        fs::write(
            directory.join("types.telora"),
            r#"import "std/array" as arrays;
               type Pair = Tuple(arrays.map(
                   arrays.enumerate([String, Int]),
                   fn(entry) { let (index, item) = entry; item },
               ));
               let pair: Pair = ("ten", 10);
               pair"#,
        )
        .unwrap();
        let types = load_module(directory.join("types.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            types.analysis.display(types.analysis.result_type),
            "(String, Int)"
        );
        assert_eq!(types.execute(100_000).unwrap().to_string(), "(\"ten\", 10)");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn array_get_and_enumerate_preserve_element_and_call_provenance() {
        let directory = fixture_dir();
        let data = directory.join("data.json");
        let main = directory.join("main.telora");
        fs::write(&data, "[10]").unwrap();

        fs::write(
            &main,
            r#"import "std/array" as arrays;
               import "std/codec" as codec;
               import "std/result" as result;
               import "./data.json" { data };
               let data = codec.decode(Array(Int), data) |> result.unwrap;
               match arrays.get(data, 0) {
                   'Some(value) => fail!("selected", value),
                   'None => 0,
               }"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(failure.contains("data.json:1:2:"), "{failure}");

        fs::write(
            &main,
            r#"import "std/array" as arrays;
               import "std/codec" as codec;
               import "std/result" as result;
               import "./data.json" { data };
               let data = codec.decode(Array(Int), data) |> result.unwrap;
               let indexed = arrays.enumerate(data);
               arrays.map(indexed, fn(entry) {
                   let (index, value) = entry;
                   if value == 10 {
                       fail!("selected", value)
                   } else { index }
               })"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(failure.contains("data.json:1:2:"), "{failure}");

        fs::write(
            &main,
            r#"import "std/array" as arrays;
               import "std/codec" as codec;
               import "std/result" as result;
               import "./data.json" { data };
               let data = codec.decode(Array(Int), data) |> result.unwrap;
               let indexed = arrays.enumerate(data);
               let first = arrays.get(indexed, 0);
               match first {
                   'Some((index, value)) => fail!("index", index),
                   'None => 0,
               }"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module
            .execute(100_000)
            .unwrap_err()
            .with_sources(&module.sources)
            .to_string();
        assert!(failure.contains("standalone/main:6:"), "{failure}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_option_and_result_combinators_are_generic_telora_definitions() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/option" as options;
               import "std/result" as results;
               let ok: Result(Int, String) = 'Ok(3);
               let err: Result(Int, String) = 'Err("bad");
               {
                   option_map: options.map('Some(2), fn(value) { value + 1 }),
                   option_map_none: options.map('None, fn(value) { value / 0 }),
                   option_flat_map: options.flat_map('Some(2), fn(value) { 'Some(value + 2) }),
                   option_flat_none: options.flat_map('None, fn(value) { 'Some(value / 0) }),
                   option_some_or: options.unwrap_or('Some(4), 9),
                   option_none_or: options.unwrap_or('None, 9),
                   option_is_some: options.is_some('Some("x")),
                   option_is_none: options.is_some('None),
                   result_map: results.map(ok, fn(value) { value + 1 }),
                   result_map_err: results.map(err, fn(value) { value / 0 }),
                   result_err_map: results.map_err(err, fn(error) { error }),
                   result_err_map_ok: results.map_err(ok, fn(error) { error }),
                   result_ok_or: results.unwrap_or(ok, 9),
                   result_err_or: results.unwrap_or(err, 9),
                   result_is_ok: results.is_ok(ok),
                   result_is_err: results.is_ok(err),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let result_world = module.execute(100_000).unwrap();
        let result = result_world.value();
        let expected = [
            ("option_map", "'Some(3)"),
            ("option_map_none", "'None"),
            ("option_flat_map", "'Some(4)"),
            ("option_flat_none", "'None"),
            ("option_some_or", "4"),
            ("option_none_or", "9"),
            ("option_is_some", "'True"),
            ("option_is_none", "'False"),
            ("result_map", "'Ok(4)"),
            ("result_map_err", "'Err(\"bad\")"),
            ("result_err_map", "'Err(\"bad\")"),
            ("result_err_map_ok", "'Ok(3)"),
            ("result_ok_or", "3"),
            ("result_err_or", "9"),
            ("result_is_ok", "'True"),
            ("result_is_err", "'False"),
        ];
        for (name, expected) in expected {
            assert_eq!(result.get(name).unwrap().to_string(), expected, "{name}");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typed_metadata_witnesses_flow_through_codec_and_validation_boundaries() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/result" as result;
               type User = struct {name: String};
               let as_value = fn(value) { codec.encode(codec.Value, value) |> result.unwrap };
               let decoded = codec.decode(User, as_value({name: "Ada"}));
               let encoded = codec.encode(codec.Value, {name: "Lin"});
               let checked = validate(User, {name: "Grace"});
               let invalid = validate(User, {name: 1});
               let formatted = result.map_err(
                   codec.decode(User, as_value({name: 1})),
                   codec.format_error,
               );
               let chained = result.flat_map(
                   codec.decode(User, as_value({name: "Mira"})),
                   fn(user) { validate(User, user) },
               );
               let name = result.unwrap(result.map(
                   codec.decode(User, as_value({name: "Kai"})),
                   fn(user) { user.name },
               ));
               {
                   decoded: decoded,
                   encoded: encoded,
                   checked: checked,
                   invalid: invalid,
                   formatted: formatted,
                   chained: chained,
                   name: name,
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["decoded"]),
            "enum {Err({data: Any, message: String, rule: Any}), Ok(User)}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["checked"]),
            "enum {Err({data: Any, message: String, rule: Any}), Ok(User)}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["encoded"]),
            "enum {Err({data: Any, message: String, rule: Any}), Ok(Value)}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["formatted"]),
            "enum {Err(String), Ok(User)}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["chained"]),
            "enum {Err({data: Any, message: String, rule: Any}), Ok(User)}"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["name"]),
            "String"
        );
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        assert_eq!(output.get("name").unwrap().to_string(), "\"Kai\"");
        assert!(
            output
                .get("formatted")
                .unwrap()
                .to_string()
                .contains("expected String")
        );
        let (tag, error) = output.get("invalid").unwrap().tagged_parts().unwrap();
        assert_eq!(tag.as_atom().as_deref(), Some("Err"));
        let message = error.get("message").unwrap().to_string();
        assert!(message.contains("must be String"), "{message}");
        assert_eq!(error.get("data").unwrap().to_string(), "{name: 1}");
        let rule = error.get("rule").unwrap().to_string();
        assert!(rule.contains("User"), "{rule}");

        fs::write(
            directory.join("wrong-encode.telora"),
            r#"import "std/codec" as codec;
               type User = struct {name: String};
               let user: User = {name: 1};
               codec.encode(codec.Value, user)"#,
        )
        .unwrap();
        let error = load_module(
            directory.join("wrong-encode.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot unify Int with String"));

        fs::write(
            directory.join("erased.telora"),
            "let metadata: Type = Int; validate(metadata, 1)",
        )
        .unwrap();
        let error =
            load_module(directory.join("erased.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("TypeOf"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_dict_enumerates_constructs_and_merges_in_canonical_order() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dict" as dicts;
               let source = { z: 3, a: 1, middle: 2 };
               let empty: Array(Tuple([String, Int])) = [];
               {
                   keys: dicts.keys(source),
                   values: dicts.values(source),
                   pairs: dicts.pairs(source),
                   round_trip: dicts.from_pairs(dicts.pairs(source)),
                   merged: dicts.merge(
                       { a: 1, nested: { left: 1 } },
                       { b: 2, nested: { right: 2 } },
                   ),
                   empty_keys: dicts.keys({}),
                   empty_pairs: dicts.pairs({}),
                   empty_from_pairs: dicts.from_pairs(empty),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let result_world = module.execute(100_000).unwrap();
        let result = result_world.value();
        assert_eq!(
            result.get("keys").unwrap().to_string(),
            "[\"a\", \"middle\", \"z\"]"
        );
        assert_eq!(result.get("values").unwrap().to_string(), "[1, 2, 3]");
        assert_eq!(
            result.get("pairs").unwrap().to_string(),
            "[(\"a\", 1), (\"middle\", 2), (\"z\", 3)]"
        );
        assert_eq!(
            result.get("round_trip").unwrap().to_string(),
            "{a: 1, middle: 2, z: 3}"
        );
        assert_eq!(
            result.get("merged").unwrap().to_string(),
            "{a: 1, b: 2, nested: {right: 2}}"
        );
        assert_eq!(result.get("empty_keys").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_pairs").unwrap().to_string(), "[]");
        assert_eq!(result.get("empty_from_pairs").unwrap().to_string(), "{}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_type_desc_exposes_erased_kinds_and_structured_ref_errors() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/type-desc" as desc;
               import "std/attributes" as attributes;
               import "std/array" as arrays;
               import "std/result" as result;
               type Node = struct {children: Array(Node)};
               let node_body = result.unwrap(desc.resolve(Node));
               let struct_nodes = desc.children(node_body);
               let field_nodes = arrays.flat_map(struct_nodes, desc.children);
               let array_nodes = arrays.flat_map(field_nodes, desc.children);
               let refs = arrays.flat_map(array_nodes, desc.children);
               {
                   int_kind: desc.kind(Int),
                   func_kind: desc.kind(Func([Int], String)),
                   attributed_kind: desc.kind(attributes.add(Int, {doc: "number"})),
                   resolve_int: desc.resolve(Int),
                   node_kind: desc.kind(Node),
                   node_body_kind: desc.kind(node_body),
                   ref_kinds: arrays.map(refs, desc.kind),
                   resolved_kinds: arrays.map(refs, fn(reference) {
                       match desc.resolve(reference) {
                           'Ok(target) => desc.kind(target),
                           'Err(_) => 'Never,
                       }
                   }),
                   TypeDesc: desc.TypeDesc,
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        assert_eq!(output.get("int_kind").unwrap().to_string(), "'Int");
        assert_eq!(output.get("func_kind").unwrap().to_string(), "'Func");
        assert_eq!(
            output.get("attributed_kind").unwrap().to_string(),
            "'WithAttributes"
        );
        assert_eq!(output.get("TypeDesc").unwrap().to_string(), "{kind: 'Type}");
        assert_eq!(output.get("node_kind").unwrap().to_string(), "'Ref");
        assert_eq!(
            output.get("node_body_kind").unwrap().to_string(),
            "'WithAttributes"
        );
        assert_eq!(output.get("ref_kinds").unwrap().to_string(), "['Ref]");
        assert_eq!(
            output.get("resolved_kinds").unwrap().to_string(),
            "['WithAttributes]"
        );
        let (tag, error) = output.get("resolve_int").unwrap().tagged_parts().unwrap();
        assert_eq!(tag.as_atom().as_deref(), Some("Err"));
        assert_eq!(
            error.get("message").unwrap().to_string(),
            "\"type descriptor is not a recursive reference\""
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn opaque_type_is_nominal_reflectable_and_not_codec_data() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/type-desc" as desc;
               import "std/hash" as hash;
               {
                   kind: desc.kind(hash.HashState),
                   children: desc.children(hash.HashState),
                   name: desc.opaque_name(hash.HashState),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        assert_eq!(output.get("kind").unwrap().to_string(), "'Opaque");
        assert_eq!(output.get("children").unwrap().to_string(), "[]");
        assert_eq!(
            output.get("name").unwrap().to_string(),
            "'Some(\"std/hash#HashState\")"
        );
        fs::write(
            directory.join("main.telora"),
            r#"import "std/json" as json;
               import "std/hash" as hash;
               json.decode(hash.HashState, "1")"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let decoded = module.execute(100_000).unwrap();
        let (tag, error) = decoded.value().tagged_parts().expect("tagged Result");
        assert_eq!(tag.as_atom().as_deref(), Some("Err"));
        assert_eq!(
            error.get("message").unwrap().to_string(),
            "\"$: Opaque has no JSON codec\""
        );
        fs::write(
            directory.join("main.telora"),
            r#"import "std/json" as json;
               import "std/hash" as hash;
               json.schema(hash.HashState)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Opaque has no JSON Schema mapping")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_hash_state_is_persistent_and_follows_the_versioned_protocol() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/hash" as hash;
               let empty = hash.new();
               let string = hash.update_string(empty, "abc");
               let bytes = hash.update_bytes(empty, b"abc");
               let integer = hash.update_int(empty, -1);
               {
                   empty: hash.finish(empty),
                   empty_again: hash.finish(empty),
                   string: hash.finish(string),
                   bytes: hash.finish(bytes),
                   integer: hash.finish(integer),
                   empty_unchanged: empty == hash.new(),
                   kinds_differ: string == bytes,
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        let digest = |name: &str| {
            output
                .get(name)
                .unwrap()
                .as_bytes()
                .unwrap()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert_eq!(
            digest("empty"),
            "60ec81853a8626c6656e854de787935ca1d364e6b35721c911bb52f5ab6848e0"
        );
        assert_eq!(digest("empty_again"), digest("empty"));
        assert_eq!(
            digest("string"),
            "74365801acb81e0b715feefcc7f61d2ae2b69ca2db302ac8da1d4905903a2357"
        );
        assert_eq!(
            digest("bytes"),
            "8153fe52a36d2948a281e929455aca2f565b95c43b7b1731ac471a49d23ec1cd"
        );
        assert_eq!(
            digest("integer"),
            "322aacdbd881f3ea5904156d8e4030a2936e085ff92cd272ae99378207eb7d34"
        );
        assert_eq!(output.get("empty_unchanged").unwrap().to_string(), "'True");
        assert_eq!(output.get("kinds_differ").unwrap().to_string(), "'False");
        fs::remove_dir_all(directory).unwrap();
    }
