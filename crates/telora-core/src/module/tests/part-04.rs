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
