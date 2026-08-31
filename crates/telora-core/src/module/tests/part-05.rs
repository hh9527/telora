    #[test]
    fn core_dyn_packs_projects_and_publishes_opaque_values() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dyn" as dyn;
               let int_value = dyn.pack(Int, 41);
               let string_value = dyn.pack(String, "text");
               let float_value = dyn.pack(Float, 1.5);
               let bytes_value = dyn.pack(Bytes, b"ab");
               type Unary = Func([Int], Int);
               type A = struct {value: Int};
               type B = struct {value: Int};
               let identity: Fn(Int) -> Int = fn(value) { value };
               let func_value = dyn.pack(Unary, identity);
               let nominal = dyn.pack(A, {value: 7});
               let captured = fn() { int_value };
               {
                   int_type: dyn.desc(int_value),
                   int_kind: dyn.kind(int_value),
                   func_kind: dyn.kind(func_value),
                   int_value: dyn.check_int(int_value),
                   wrong_value: dyn.check_string(int_value),
                   string_value: dyn.check_string(string_value),
                   float_value: dyn.check_float(float_value),
                   bytes_value: dyn.check_bytes(bytes_value),
                   projected_int: dyn.project_with(Int, int_value),
                   projected_sugar: dyn.project@[Int](int_value),
                   projected_wrong: dyn.project_with(Float, int_value),
                   projected_nominal: dyn.project_with(A, nominal),
                   projected_conflict: dyn.project_with(B, nominal),
                   same_identity: int_value == int_value,
                   different_identity: int_value == dyn.pack(Int, 41),
                   values: [captured(), string_value],
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["int_value"]),
            "Dyn"
        );
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        assert_eq!(output.get("int_type").unwrap().to_string(), "{kind: 'Int}");
        assert_eq!(output.get("int_kind").unwrap().to_string(), "'Int");
        assert_eq!(output.get("func_kind").unwrap().to_string(), "'Func");
        assert_eq!(output.get("int_value").unwrap().to_string(), "'Some(41)");
        assert_eq!(output.get("wrong_value").unwrap().to_string(), "'None");
        assert_eq!(
            output.get("string_value").unwrap().to_string(),
            "'Some(\"text\")"
        );
        assert_eq!(output.get("float_value").unwrap().to_string(), "'Some(1.5)");
        assert_eq!(
            output.get("bytes_value").unwrap().to_string(),
            "'Some(b\"\\x61\\x62\")"
        );
        assert_eq!(
            output.get("projected_int").unwrap().to_string(),
            "'Some(41)"
        );
        assert_eq!(
            output.get("projected_sugar").unwrap().to_string(),
            "'Some(41)"
        );
        assert_eq!(output.get("projected_wrong").unwrap().to_string(), "'None");
        assert_eq!(
            output.get("projected_nominal").unwrap().to_string(),
            "'Some({value: 7})"
        );
        assert_eq!(
            output.get("projected_conflict").unwrap().to_string(),
            "'None"
        );
        assert_eq!(output.get("same_identity").unwrap().to_string(), "'True");
        assert_eq!(
            output.get("different_identity").unwrap().to_string(),
            "'False"
        );
        assert_eq!(output.get("values").unwrap().to_string(), "[<dyn>, <dyn>]");

        fs::write(
            directory.join("invalid.telora"),
            r#"import "std/dyn" as dyn;
               dyn.pack@[Int](Int, "wrong")"#,
        )
        .unwrap();
        let error =
            load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("cannot unify String with Int"));

        fs::write(
            directory.join("invalid-project.telora"),
            r#"let dyn = {project: fn(value) { value }};
               dyn.project@[Int](1)"#,
        )
        .unwrap();
        let error = load_module(
            directory.join("invalid-project.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(error.to_string().contains("imported std/dyn namespace"));

        fs::write(
            directory.join("generic-project.telora"),
            r#"import "std/dyn" as dyn;
               def project: for(A) Fn(Dyn) -> Option(A) = fn(value) {
                   dyn.project@[A](value)
               };
               0"#,
        )
        .unwrap();
        let error = load_module(
            directory.join("generic-project.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("runtime TypeOf witness"),
            "{error}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interpreter_wrappers_are_memoized_by_function_and_canonical_type_arguments() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dyn" as dyn;
               def consume: Fn(Dyn) -> Bool = fn(value) {
                   match dyn.project_with(String, value) {
                       'Some(_) => 'True,
                       'None => 'False,
                   }
               };
               def first: for(A) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter!(consume);
               def second: for(A) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter!(consume);
               let first_int = first(Int);
               let repeated_int = first(Int);
               let first_string = first(String);
               let second_int = second(Int);
               {
                   same_key: first_int == repeated_int,
                   int_projection: first_int(1),
                   string_projection: first_string("text"),
                   different_interpreter: first_int == second_int,
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 200_000).unwrap();
        let world = module.execute(200_000).unwrap();
        let output = world.value().to_string();
        assert!(output.contains("same_key: 'True"), "{output}");
        assert!(output.contains("int_projection: 'False"), "{output}");
        assert!(output.contains("string_projection: 'True"), "{output}");
        assert!(output.contains("different_interpreter: 'False"), "{output}");
        let (_, work) = world.into_parts();
        assert_eq!(work.heap().memoized_interpreter_count(), 3);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interpreter_memoization_rejects_non_type_host_arguments() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"def consume: Fn(Dyn) -> String = fn(value) { "ok" };
               export def prepare: for(A) Fn(TypeOf(A)) -> Fn(A) -> String =
                   interpreter!(consume);"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let loaded = engine.load_module(&main, BTreeMap::new()).unwrap();
        let prepare = engine.execute(&loaded).unwrap().select("prepare").unwrap();
        let error = engine
            .invoke_world(&loaded, prepare, &[crate::DataWorld::int(1)])
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("interpreter static argument"),
            "{error}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_dict_supports_tool_stage_and_exact_output_quota() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dict" as dicts;
               dicts.keys(data)"#,
        )
        .unwrap();
        let module = load_module(
            directory.join("main.telora"),
            BTreeMap::from([(
                "data".into(),
                parse_json("data.json", r#"{"a":1,"long":2}"#).unwrap(),
            )]),
            100_000,
        )
        .unwrap();
        let requested = 2 * std::mem::size_of::<Val>() as u64 + 5;
        let mut exact = QuotaAccount::new(Quota::new(1, 1_000, requested));
        let arena = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut exact,
            )
            .unwrap();
        assert_eq!(exact.requested_allocation_bytes(), requested);
        assert_eq!(
            arena.root_ref(&module.runtime.main.heap).to_string(),
            "[\"a\", \"long\"]"
        );

        let mut short = QuotaAccount::new(Quota::new(1, 1_000, requested - 1));
        assert_eq!(
            Vm::new()
                .execute_in_work(
                    &module.runtime.main.heap,
                    &module.runtime.externals,
                    &module.function,
                    &[],
                    &mut short,
                )
                .err()
                .expect("allocation must be exhausted")
                .kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        fs::write(
            directory.join("types.telora"),
            r#"import "std/dict" as dicts;
               type Pair = Tuple(dicts.values({ first: Int, second: String }));
               let pair: Pair = (1, "one");
               pair"#,
        )
        .unwrap();
        let types = load_module(directory.join("types.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(types.execute(100_000).unwrap().to_string(), "(1, \"one\")");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn json_schema_maps_composites_and_obeys_allocation_quota() {
        let directory = fixture_dir();
        let path = directory.join("main.telora");
        fs::write(
            &path,
            r#"import "std/json" as json;
               json.schema(union('None, [Int, Array(String), {kind: 'Tuple, items: [Int, String]}]))"#,
        )
        .unwrap();
        let module = load_module(&path, BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("anyOf"));
        assert!(output.contains("prefixItems"));
        assert!(output.contains("items"));

        let mut account = QuotaAccount::new(Quota::new(10, 1_000, 1));
        let error = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            )
            .err()
            .expect("schema generation must exhaust allocation quota");
        assert_eq!(error.kind, crate::RuntimeErrorKind::AllocationQuotaExceeded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recursive_type_metadata_publishes_and_drives_codecs_and_schema_refs() {
        let directory = fixture_dir();
        fs::write(
            directory.join("Types.telora"),
            r#"import "std/json" as json;
               type Node = struct {
                   value: Int,
                   children: Array(Node),
               };
               type Left = struct {right: Option(Right)};
               type Right = struct {left: Option(Left)};
               {Node: Node, Left: Left, Right: Right}"#,
        )
        .unwrap();
        let types_module =
            load_module(directory.join("Types.telora"), BTreeMap::new(), 100_000).unwrap();
        let node = types_module.analysis.declared_types["Node"];
        let crate::TypeNode::Declared { id, body, .. } = types_module.analysis.types.node(node)
        else {
            panic!("Node must be a declared Type in the authoritative type graph");
        };
        let crate::TypeNode::Struct(fields) = types_module.analysis.types.node(*body) else {
            panic!("Node body must be a Struct in the authoritative type graph");
        };
        let crate::TypeNode::Array(children) = types_module.analysis.types.node(fields["children"])
        else {
            panic!("Node.children must be an Array");
        };
        assert!(matches!(
            types_module.analysis.types.node(*children),
            crate::TypeNode::Declared { id: child_id, .. } if child_id == id
        ));
        assert_eq!(types_module.analysis.display(node), "Node");
        assert!(types_module.analysis.types.is_assignable(node, node));

        fs::write(
            directory.join("main.telora"),
            r#"import "./Types" as Types;
               import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               let node = codec.decode(Types.Node, codec.encode(codec.Value, {
                   value: 1,
                   children: [{value: 2, children: []}],
               }) |> result.unwrap) |> result.unwrap;
               let pair = codec.decode(Types.Left, codec.encode(codec.Value, {
                   right: {left: 'None},
               }) |> result.unwrap) |> result.unwrap;
               {
                   node: node,
                   encoded: codec.encode(codec.Value, node) |> result.unwrap,
                   pair: pair,
                   schema: json.schema(Types.Node),
                   mutual_schema: json.schema(Types.Left),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(
            output.contains("children: [{children: [], value: 2}]"),
            "{output}"
        );
        assert!(
            output.contains("pair: {right: 'Some({left: 'None})}"),
            "{output}"
        );
        assert!(output.contains("$defs"), "{output}");
        assert!(output.contains("#/$defs/Type0"), "{output}");
        assert!(output.contains("#/$defs/Type1"), "{output}");

        fs::write(
            directory.join("bad.json"),
            r#"{"value":1,"children":[{"value":"wrong","children":[]}]}"#,
        )
        .unwrap();
        fs::write(
            directory.join("bad.telora"),
            r#"import "./bad.json" { data };
               import "./Types" as Types;
               import "std/codec" as codec;
               import "std/result" as result;
               codec.decode(Types.Node, data) |> result.unwrap"#,
        )
        .unwrap();
        let bad = load_module(directory.join("bad.telora"), BTreeMap::new(), 100_000).unwrap();
        let failure = bad.execute(100_000).unwrap_err();
        assert!(failure.message.contains("$.children[0].value"));
        assert!(failure.data_location().is_some());
        assert!(failure.rule_location().is_some());

        fs::write(
            directory.join("leak.telora"),
            r#"import "./Types" as Types;
               import "std/codec" as codec;
               import "std/result" as result;
               codec.encode(codec.Value, Types.Node) |> result.unwrap"#,
        )
        .unwrap();
        let leak = load_module(directory.join("leak.telora"), BTreeMap::new(), 100_000).unwrap();
        let leak_error = leak.execute(100_000).unwrap_err();
        assert!(
            leak_error.message.contains("cannot encode Type"),
            "{}",
            leak_error.message
        );
        fs::remove_dir_all(directory).unwrap();
    }
