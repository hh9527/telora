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
    fn core_dyn_structural_observers_preserve_child_descriptors() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dyn" as dyn;
               import "std/result" as result;
               import "std/array" as arrays;
               type User = struct {name: String, pair: Tuple([Int, String])};
               type Node = struct {value: Int, children: Array(Node)};
               type Maybe = enum {'None, 'Some(Int)};
               let user = dyn.pack(User, {name: "Ada", pair: (1, "one")});
               let name = result.unwrap(dyn.field(user, "name"));
               let pair = result.unwrap(dyn.field(user, "pair"));
               let dict = dyn.pack(Dict(Int), {a: 7});
               let numbers = dyn.pack(Array(Int), [1, 2]);
               let root = dyn.pack(Node, {
                   value: 1,
                   children: [{value: 2, children: []}],
               });
               let children = result.unwrap(dyn.field(root, "children"));
               let child_nodes = result.unwrap(dyn.array_items(children));
               {
                   name: dyn.check_string(name),
                   user_fields: arrays.map(result.unwrap(dyn.fields(user)), fn(pair) {
                       match pair { (name, value) => (name, dyn.kind(value)) }
                   }),
                   dict_fields: arrays.map(result.unwrap(dyn.fields(dict)), fn(pair) {
                       match pair { (name, value) => (name, dyn.kind(value)) }
                   }),
                   dict_value: dyn.check_int(result.unwrap(dyn.field(dict, "a"))),
                   array_values: arrays.map(
                       result.unwrap(dyn.array_items(numbers)),
                       dyn.check_int,
                   ),
                   tuple_values: arrays.map(
                       result.unwrap(dyn.tuple_items(pair)),
                       dyn.kind,
                   ),
                   enum_tag: result.unwrap(dyn.tag(dyn.pack(Maybe, 'Some(3)))),
                   enum_payload: match result.unwrap(dyn.payload(dyn.pack(Maybe, 'Some(3)))) {
                       'Some(value) => dyn.check_int(value),
                       'None => 'None,
                   },
                   unit_payload: result.unwrap(dyn.payload(dyn.pack(Maybe, 'None))),
                   recursive_values: arrays.map(child_nodes, fn(child) {
                       dyn.check_int(result.unwrap(dyn.field(child, "value")))
                   }),
                   missing: dyn.field(user, "missing"),
                   wrong_shape: dyn.array_items(user),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 200_000).unwrap();
        let output_world = module.execute(200_000).unwrap();
        let output = output_world.value();
        assert_eq!(output.get("name").unwrap().to_string(), "'Some(\"Ada\")");
        assert_eq!(
            output.get("user_fields").unwrap().to_string(),
            "[(\"name\", 'String), (\"pair\", 'Tuple)]"
        );
        assert_eq!(
            output.get("dict_fields").unwrap().to_string(),
            "[(\"a\", 'Int)]"
        );
        assert_eq!(output.get("dict_value").unwrap().to_string(), "'Some(7)");
        assert_eq!(
            output.get("array_values").unwrap().to_string(),
            "['Some(1), 'Some(2)]"
        );
        assert_eq!(
            output.get("tuple_values").unwrap().to_string(),
            "['Int, 'String]"
        );
        assert_eq!(output.get("enum_tag").unwrap().to_string(), "\"Some\"");
        assert_eq!(output.get("enum_payload").unwrap().to_string(), "'Some(3)");
        assert_eq!(output.get("unit_payload").unwrap().to_string(), "'None");
        assert_eq!(
            output.get("recursive_values").unwrap().to_string(),
            "['Some(2)]"
        );
        for field in ["missing", "wrong_shape"] {
            let (tag, blame) = output.get(field).unwrap().tagged_parts().unwrap();
            assert_eq!(tag.as_atom().as_deref(), Some("Err"));
            assert!(blame.get("message").unwrap().to_string().len() > 4);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interpreter_keyword_lifts_erased_binary_consumers() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dyn" as dyn;
               def int_eq_i: Fn(Dyn, Dyn) -> Bool = fn(left, right) {
                   match dyn.check_int(left) {
                       'Some(a) => match dyn.check_int(right) {
                           'Some(b) => a == b,
                           'None => 'False,
                       },
                       'None => 'False,
                   }
               };
               def eq_fn: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool =
                   interpreter!(int_eq_i);
               {
                   equal: eq_fn@[Int](Int)(1, 1),
                   different: eq_fn@[Int](Int)(1, 2),
                   inferred: eq_fn(Int)(2, 2),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 200_000).unwrap();
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["eq_fn"]),
            "Fn(TypeOf(Any)) -> Fn(Any, Any) -> enum {False, True}"
        );
        let output_world = module.execute(200_000).unwrap();
        let output = output_world.value();
        assert_eq!(output.get("equal").unwrap().to_string(), "'True");
        assert_eq!(output.get("different").unwrap().to_string(), "'False");
        assert_eq!(output.get("inferred").unwrap().to_string(), "'True");

        fs::write(
            directory.join("invalid.telora"),
            r#"def bad_i: Fn(Dyn) -> Bool = fn(value) { 'True };
               def bad: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool =
                   interpreter!(bad_i);
               0"#,
        )
        .unwrap();
        let error =
            load_module(directory.join("invalid.telora"), BTreeMap::new(), 200_000).unwrap_err();
        assert!(error.to_string().contains("expects 1 arguments, found 2"));

        for (source, expected) in [
            (
                "def bad = interpreter!(eq_i); 0",
                "interpreter requires an explicit",
            ),
            (
                "def bad: for(A) Fn(A) -> Fn(A, A) -> Bool = interpreter!(eq_i); 0",
                "witness parameter 1",
            ),
            (
                "def bad: for(A) Fn(TypeOf(A)) -> Fn(Array(A)) -> Bool = interpreter!(eq_i); 0",
                "inner parameter 1 contains type parameter A",
            ),
            (
                "def bad: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> A = interpreter!(eq_i); 0",
                "result contains type parameter A",
            ),
            (
                "def bad: for(A, B) Fn(TypeOf(A), TypeOf(A)) -> Fn(A) -> Bool = interpreter!(eq_i); 0",
                "type parameter A has more than one TypeOf witness",
            ),
            (
                "def bad: for(A, B) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter!(eq_i); 0",
                "type parameter B has no TypeOf witness",
            ),
            (
                "def bad: for(A) Fn(TypeOf(A)) -> Fn(Fn(A) -> Bool) -> Bool = interpreter!(eq_i); 0",
                "inner parameter 1 contains type parameter A",
            ),
            (
                "def bad: for(A) Fn(TypeOf(A)) -> Fn(A) -> Option(A) = interpreter!(eq_i); 0",
                "result contains type parameter A",
            ),
            (
                "let bad = interpreter!(eq_i); 0",
                "interpreter requires an explicit",
            ),
        ] {
            fs::write(directory.join("invalid-shape.telora"), source).unwrap();
            let error = load_module(
                directory.join("invalid-shape.telora"),
                BTreeMap::new(),
                200_000,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error}"
            );
        }
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
    fn telora_equality_interpreter_matches_native_structural_equality() {
        let directory = fixture_dir();
        fs::write(
            directory.join("reference-equality.telora"),
            include_str!("../../../../../examples/reference-equality.telora"),
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./reference-equality" as equality;
               import "std/eq" as eq;
               type Node = struct {value: Int, children: Array(Node)};
               type Choice = enum {'None, 'Some(String)};
               type Pair = Tuple([Int, String]);
               type Unary = Fn(Int) -> Int;
               let left: Node = {value: 1, children: [{value: 2, children: []}]};
               let same: Node = {value: 1, children: [{value: 2, children: []}]};
               let different: Node = {value: 1, children: [{value: 3, children: []}]};
               let none: Choice = 'None;
               let some: Choice = 'Some("x");
               {
                   int_equal: (equality.equal(Int)(1, 1), eq.equal(1, 1), 1 == 1),
                   int_different: (equality.equal(Int)(1, 2), eq.equal(1, 2), 1 == 2),
                   array_equal: (equality.equal(Array(Int))([1, 2], [1, 2]), eq.equal([1, 2], [1, 2]), [1, 2] == [1, 2]),
                   array_length: (equality.equal(Array(Int))([1], [1, 2]), eq.equal([1], [1, 2]), [1] == [1, 2]),
                   tuple_equal: (equality.equal(Pair)((1, "a"), (1, "a")), eq.equal((1, "a"), (1, "a")), (1, "a") == (1, "a")),
                   dict_different: (equality.equal(Dict(Int))({a: 1}, {a: 2}), eq.equal({a: 1}, {a: 2}), {a: 1} == {a: 2}),
                   enum_equal: (equality.equal(Choice)(some, some), eq.equal(some, some), some == some),
                   enum_tag: (equality.equal(Choice)(none, some), eq.equal(none, some), none == some),
                   recursive_equal: (equality.equal(Node)(left, same), eq.equal(left, same), left == same),
                   recursive_different: (equality.equal(Node)(left, different), eq.equal(left, different), left == different),
                   function_error: equality.equal(Unary)(fn(x) { x }, fn(x) { x }),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 500_000).unwrap();
        let output_world = module.execute(500_000).unwrap();
        let output = output_world.value();
        for field in [
            "int_equal",
            "array_equal",
            "tuple_equal",
            "enum_equal",
            "recursive_equal",
        ] {
            assert_eq!(
                output.get(field).unwrap().to_string(),
                "('Ok('True), 'True, 'True)"
            );
        }
        for field in [
            "int_different",
            "array_length",
            "dict_different",
            "enum_tag",
            "recursive_different",
        ] {
            assert_eq!(
                output.get(field).unwrap().to_string(),
                "('Ok('False), 'False, 'False)"
            );
        }
        assert!(
            output
                .get("function_error")
                .unwrap()
                .to_string()
                .starts_with("'Err(")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_eq_is_the_function_form_of_the_equality_operator() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as arrays;
               import "std/eq" as eq;
               let function = fn(value) { value };
               let pairs = arrays.zip([1, 2, 3], [1, 2, 3]);
               def accepts_int_eq: Fn(Fn(Int, Int) -> Bool) -> Bool = fn(compare) {
                   compare(1, 1)
               };
               {
                   scalar: (eq.equal(1, 1), 1 == 1),
                   nested: (eq.equal([{a: 1}], [{a: 1}]), [{a: 1}] == [{a: 1}]),
                   same_function: (eq.equal(function, function), function == function),
                   other_function: (eq.equal(function, fn(value) { value }), function == fn(value) { value }),
                   higher_order: match pairs {
                       'None => 'False,
                       'Some(values) => arrays.all(values, fn(pair) {
                           match pair { (left, right) => eq.equal(left, right) }
                       }),
                   },
                   direct_callback: accepts_int_eq(eq.equal),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 200_000).unwrap();
        let output_world = module.execute(200_000).unwrap();
        let output = output_world.value();
        for field in ["scalar", "nested", "same_function"] {
            assert_eq!(output.get(field).unwrap().to_string(), "('True, 'True)");
        }
        assert_eq!(
            output.get("other_function").unwrap().to_string(),
            "('False, 'False)"
        );
        assert_eq!(output.get("higher_order").unwrap().to_string(), "'True");
        assert_eq!(output.get("direct_callback").unwrap().to_string(), "'True");

        fs::write(
            directory.join("invalid.telora"),
            r#"import "std/eq" as eq; (eq.equal(1, "1"), 1 == "1")"#,
        )
        .unwrap();
        let error =
            load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("cannot unify String with Int"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typed_equality_allows_distinct_runtime_variants_of_one_union() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"type Scalar = union('None, [Int, String]);
               let left: Scalar = 1;
               let right: Scalar = "1";
               (left == right, left != right)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().value().to_string(),
            "('False, 'True)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn homogeneous_dict_combinators_preserve_types_and_canonical_order() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/dict" as dicts;
               let source: Dict(Int) = {z: 3, a: 1, middle: 2};
               let empty: Dict(Int) = {};
               {
                   mapped: dicts.map_values(source, fn(value) { `v\{value}` }),
                   filtered: dicts.filter(source, fn(value) { 1 < value }),
                   folded: dicts.fold(source, "", fn(total, key, value) {
                       `\{total}\{key}=\{value};`
                   }),
                   empty_mapped: dicts.map_values(empty, fn(value) { `v\{value}` }),
                   empty_filtered: dicts.filter(empty, fn(value) { 0 < value }),
                   empty_folded: dicts.fold(empty, 42, fn(total, key, value) {
                       total + value
                   }),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "{empty_filtered: Dict<Int>, empty_folded: Int, empty_mapped: Dict<String>, filtered: Dict<Int>, folded: String, mapped: Dict<String>}"
        );
        let result_world = module.execute(100_000).unwrap();
        let result = result_world.value();
        assert_eq!(
            result.get("mapped").unwrap().to_string(),
            r#"{a: "v1", middle: "v2", z: "v3"}"#
        );
        assert_eq!(
            result.get("filtered").unwrap().to_string(),
            "{middle: 2, z: 3}"
        );
        assert_eq!(
            result.get("folded").unwrap().to_string(),
            r#""a=1;middle=2;z=3;""#
        );
        assert_eq!(result.get("empty_mapped").unwrap().to_string(), "{}");
        assert_eq!(result.get("empty_filtered").unwrap().to_string(), "{}");
        assert_eq!(result.get("empty_folded").unwrap().to_string(), "42");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn homogeneous_dict_combinators_reject_invalid_contracts_and_trace_callbacks() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/dict" as dicts;
               dicts.filter({a: 1}, fn(value) { value })"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot unify Int with enum {False, True}")
        );

        fs::write(
            &main,
            r#"import "std/dict" as dicts;
               let mixed = {number: 1, text: "two"};
               dicts.map_values(mixed, fn(value) { value })"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("cannot unify"));

        fs::write(
            &main,
            r#"import "std/dict" as dicts;
               let source: Dict(Int) = {a: 1};
               dicts.map_values(source, fn(value) { value / 0 })"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(error.to_string().contains("standalone/bin/main:3:"));
        assert!(
            error
                .trace
                .iter()
                .any(|frame| frame.function == "std/dict.map_values")
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
    fn core_attributes_normalizes_flattens_and_inspects_arbitrary_values() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/attributes" as attributes;
               let nested = {
                   kind: 'WithAttributes,
                   inner: {
                       kind: 'WithAttributes,
                       inner: 42,
                       attributes: { shared: "inner", only_inner: 1 },
                   },
                   attributes: { shared: "outer", only_outer: 2 },
               };
               let augmented = attributes.add(
                   nested,
                   { shared: "addition", "vendor:acme.flag": 'True },
               );
               {
                   normalized: attributes.normalize(nested),
                   all: attributes.all(augmented),
                   shared: attributes.get(augmented, "shared"),
                   missing: attributes.get(augmented, "missing"),
                   has: attributes.has(augmented, "vendor:acme.flag"),
                   lacks: attributes.has(augmented, "missing"),
                   stripped: attributes.strip(augmented),
                   plain: attributes.normalize("plain"),
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let result_world = module.execute(100_000).unwrap();
        let result = result_world.value();
        assert_eq!(
            result.get("all").unwrap().to_string(),
            "{only_inner: 1, only_outer: 2, shared: \"addition\", vendor:acme.flag: 'True}"
        );
        assert_eq!(
            result.get("shared").unwrap().to_string(),
            "'Some(\"addition\")"
        );
        assert_eq!(result.get("missing").unwrap().to_string(), "'None");
        assert_eq!(result.get("has").unwrap().to_string(), "'True");
        assert_eq!(result.get("lacks").unwrap().to_string(), "'False");
        assert_eq!(result.get("stripped").unwrap().to_string(), "42");

        let normalized = result.get("normalized").unwrap();
        assert_eq!(
            normalized.get("attributes").unwrap().to_string(),
            "{only_inner: 1, only_outer: 2, shared: \"outer\"}"
        );
        assert_eq!(normalized.get("inner").unwrap().to_string(), "42");
        let plain = result.get("plain").unwrap();
        assert_eq!(plain.get("attributes").unwrap().to_string(), "{}");
        assert_eq!(plain.get("inner").unwrap().to_string(), "\"plain\"");
        fs::remove_dir_all(directory).unwrap();
    }


    #[test]
    fn enum_validation_rejects_unknown_tags_and_payload_shape_mismatches() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/result" as result;
               type Choice = enum {'None, 'Number(Int)};
               {
                   unknown: validate(Choice, 'Other),
                   missing: validate(Choice, 'Number),
                   unexpected: validate(Choice, 'None(1)),
                   wrong: validate(Choice, 'Number("one")),
                   codec: codec.decode(Choice, codec.encode(codec.Value, "None") |> result.unwrap),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let result_world = module.execute(100_000).unwrap();
        let result = result_world.value();
        for field in ["unknown", "missing", "unexpected", "wrong"] {
            assert!(result.get(field).unwrap().to_string().starts_with("'Err("));
        }
        assert_eq!(result.get("codec").unwrap().to_string(), "'Ok('None)");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enum_json_codecs_round_trip_external_and_untagged_representations() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               type User = struct {name: String};
               @json.rename_all('CamelCase)
               type Event = enum {
                   'Idle,
                   'UserJoined(User),
                   'FatalError(String),
               };
               @json.untagged
               type Scalar = enum {'Text(String), 'Count(Int)};
               type Envelope = struct {event: Event};
               let fatal: Event = 'FatalError("boom");
               let nested: Envelope = {event: 'UserJoined({name: "Lin"})};
               let count: Scalar = 'Count(3);
               {
                   idle: codec.decode(Event, codec.encode(codec.Value, "idle") |> result.unwrap) |> result.unwrap,
                   joined: codec.decode(Event, codec.encode(codec.Value, {userJoined: {name: "Ada"}}) |> result.unwrap) |> result.unwrap,
                   fatal: codec.encode(codec.Value, fatal) |> result.unwrap,
                   nested: codec.encode(codec.Value, nested) |> result.unwrap,
                   text: codec.decode(Scalar, codec.encode(codec.Value, "hello") |> result.unwrap) |> result.unwrap,
                   count: codec.encode(codec.Value, count) |> result.unwrap,
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        assert_eq!(output.get("idle").unwrap().to_string(), "'Idle");
        assert_eq!(
            output.get("joined").unwrap().to_string(),
            "'UserJoined({name: \"Ada\"})"
        );
        assert_eq!(
            output.get("fatal").unwrap().to_string(),
            "'Object({fatalError: 'String(\"boom\")})"
        );
        assert_eq!(
            output.get("nested").unwrap().to_string(),
            "'Object({event: 'Object({userJoined: 'Object({name: 'String(\"Lin\")})})})"
        );
        assert_eq!(output.get("text").unwrap().to_string(), "'Text(\"hello\")");
        assert_eq!(output.get("count").unwrap().to_string(), "'Int(3)");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn untagged_enum_json_codec_rejects_no_match_and_ambiguity() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               @json.untagged type Scalar = enum {'Text(String), 'Count(Int)};
               @json.untagged type Ambiguous = enum {'Anything(Any), 'Text(String)};
               {
                   no_match: codec.decode(Scalar, codec.encode(codec.Value, []) |> result.unwrap),
                   ambiguous: codec.decode(Ambiguous, codec.encode(codec.Value, "text") |> result.unwrap),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        assert!(
            output
                .get("no_match")
                .unwrap()
                .to_string()
                .contains("matches no untagged")
        );
        assert!(
            output
                .get("ambiguous")
                .unwrap()
                .to_string()
                .contains("ambiguously matches")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn json_schema_and_codecs_share_one_vertical_model_plan() {
        let directory = fixture_dir();
        fs::write(
            directory.join("data.json"),
            r#"{"userId":7,"cityName":"London","event":{"userJoined":{"name":"Ada"}},"scalar":"active","notes":""}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./data.json" { data };
               import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               type User = struct {name: String};
               @json.rename_all('CamelCase)
               type Event = enum {'Idle, 'UserJoined(User)};
               @json.untagged type Scalar = enum {'Text(String), 'Count(Int)};
               @json.rename_all('CamelCase)
               type Model = struct {
                   user_id: Int,
                   city_name: String,
                   nickname: Option(String),
                   event: Event,
                   scalar: Scalar,
                   notes: String,
               };
               let decoded = codec.decode(Model, data) |> result.unwrap;
               let schema = json.schema(Model);
               let schema_value = codec.encode(codec.Value, schema) |> result.unwrap;
               {
                   decoded: decoded,
                   encoded: codec.encode(codec.Value, decoded) |> result.unwrap,
                   schema: schema,
                   schema_text: json.stringify(schema_value),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        let schema = output.get("schema").unwrap();
        assert_eq!(schema.get("$ref").unwrap().to_string(), "\"#/$defs/Type0\"");
        let definitions = schema.get("$defs").unwrap();
        let model_schema = definitions.get("Type0").unwrap();
        assert_eq!(model_schema.get("type").unwrap().to_string(), "\"object\"");
        assert_eq!(
            model_schema
                .get("additionalProperties")
                .unwrap()
                .to_string(),
            "'False"
        );
        let properties = model_schema.get("properties").unwrap();
        for key in [
            "userId",
            "cityName",
            "nickname",
            "event",
            "scalar",
            "notes",
        ] {
            assert!(
                properties.get(key).is_some(),
                "missing schema property {key}"
            );
        }
        assert!(
            model_schema
                .get("required")
                .unwrap()
                .to_string()
                .contains("userId")
        );
        assert!(
            !model_schema
                .get("required")
                .unwrap()
                .to_string()
                .contains("nickname")
        );
        assert!(
            output
                .get("schema_text")
                .unwrap()
                .to_string()
                .contains("$schema")
        );
        assert!(output.get("encoded").unwrap().to_string().contains("notes"));
        assert!(
            output
                .get("encoded")
                .unwrap()
                .to_string()
                .contains("userId")
        );

        fs::write(
            directory.join("data.json"),
            r#"{"userId":"wrong","cityName":"London","event":"idle","scalar":1,"notes":""}"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let failure = module.execute(100_000).unwrap_err();
        assert!(failure.message.contains("$.userId"));
        assert!(failure.data_location().is_some());
        assert!(failure.rule_location().is_some());
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
    fn builtin_bool_and_option_keep_natural_json_codec_and_schema_forms() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               {
                   boolean: codec.decode(Bool, codec.encode(codec.Value, 'True) |> result.unwrap) |> result.unwrap,
                   none: codec.decode(Option(Int), codec.encode(codec.Value, 'None) |> result.unwrap) |> result.unwrap,
                   some: codec.decode(Option(Int), codec.encode(codec.Value, 3) |> result.unwrap) |> result.unwrap,
                   encoded: codec.encode(codec.Value, 4) |> result.unwrap,
                   bool_schema: json.schema(Bool),
                   option_schema: json.schema(Option(Int)),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("boolean: 'True"), "{output}");
        assert!(output.contains("none: 'None"), "{output}");
        assert!(output.contains("some: 'Some(3)"), "{output}");
        assert!(output.contains("encoded: 'Int(4)"), "{output}");
        assert!(output.contains("type: \"boolean\""), "{output}");
        assert!(output.contains("type: \"null\""), "{output}");
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
