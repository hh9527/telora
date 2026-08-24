    #[test]
    fn executes_precedence_blocks_and_dict_access() {
        let value = run("let x = 2 + 3 * 4; {b: x, a: 1}.b").unwrap();
        assert_int(&value, 14);
    }


    #[test]
    fn compares_tagged_tuples_structurally() {
        assert_atom(&run("('Ok, 42) == ('Ok, 42)").unwrap(), "True");
        assert_atom(&run("('Ok, 42) == ('Err, 42)").unwrap(), "False");
    }

    #[test]
    fn compares_functions_by_opaque_identity() {
        let value = run(
            "let f: Fn(Any) -> Any = fn(x) { x }; let same = f == f; let distinct = f == fn(x) { x }; (same, distinct)",
        )
        .unwrap();
        let values = value.value();
        assert_eq!(values.sequence_len(), Some(2));
        assert_eq!(
            values.sequence_get(0).unwrap().as_atom().as_deref(),
            Some("True")
        );
        assert_eq!(
            values.sequence_get(1).unwrap().as_atom().as_deref(),
            Some("False")
        );
    }

    #[test]
    fn executes_complete_numeric_comparison_semantics() {
        let integers = run("(1 == 1, 1 != 2, 1 < 2, 2 > 1, 1 <= 1, 2 >= 2)").unwrap();
        assert_eq!(
            integers.to_string(),
            "('True, 'True, 'True, 'True, 'True, 'True)"
        );

        let floats = run("(1.0 == 1.0, 1.0 != 2.0, 1.0 < 2.0, 2.0 > 1.0, \
             1.0 <= 1.0, 2.0 >= 2.0, -0.0 == 0.0)")
        .unwrap();
        assert_eq!(
            floats.to_string(),
            "('True, 'True, 'True, 'True, 'True, 'True, 'True)"
        );
    }

    #[test]
    fn non_finite_float_arithmetic_raises_sourced_blame() {
        let sources = [
            "0.0 / 0.0".to_owned(),
            "1.0 / 0.0".to_owned(),
            "-1.0 / 0.0".to_owned(),
            "1e308 * 2.0".to_owned(),
            "1e308 + 1e308".to_owned(),
            "-1e308 - 1e308".to_owned(),
        ];
        for source in sources {
            let error = run(&source).unwrap_err();
            let ExecutionError::Runtime(error) = error else {
                panic!("expected runtime blame for {source}")
            };
            assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame, "{source}");
            assert_eq!(error.message, "NonFiniteFloat", "{source}");
            assert!(error.data_location().is_some(), "{source}");
            assert!(error.rule_location().is_some(), "{source}");
            assert_eq!(error.origin(), error.rule_location().map(Origin::Source));
        }
    }

    #[test]
    fn compares_strings_by_internal_utf8_byte_sequence() {
        let value = run(r#"("app" < "apple", "10" < "2", "Z" < "a", "é" < "中",
                "same" <= "same", "z" > "a", "z" >= "z",
                "a deliberately heap-backed string" > "a")"#)
        .unwrap();
        assert_eq!(
            value.to_string(),
            "('True, 'True, 'True, 'True, 'True, 'True, 'True, 'True)"
        );
    }

    #[test]
    fn inequality_preserves_structural_equality_semantics() {
        let value = run("(('Ok, [1, 2]) != ('Ok, [1, 2]), ('Ok, [1]) != ('Err, [1]))").unwrap();
        assert_eq!(value.to_string(), "('False, 'True)");
    }

    #[test]
    fn dynamic_ordered_comparison_rejects_mismatched_domains() {
        let error = run("let left: Any = \"a\"; let right: Any = 1; left < right").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error")
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
        assert!(error.message.contains("matching Int, Float, or String"));
    }

    #[test]
    fn executes_single_assignment_recursive_definitions() {
        let explicit = run(
            "decl countdown: Fn(Int) -> Int; def countdown = fn(n) { if n < 1 { 0 } else { countdown(n - 1) } }; countdown(4)",
        )
        .unwrap();
        assert_int(&explicit, 0);

        let mutual = run(
            "decl even: Fn(Int) -> Int; decl odd: Fn(Int) -> Int; def even = fn(n) { if n < 1 { 0 } else { odd(n - 1) } }; def odd = fn(n) { if n < 1 { 1 } else { even(n - 1) } }; even(4)",
        )
        .unwrap();
        assert_int(&mutual, 0);

        let higher_order = run(
            "decl loop: Fn(Int) -> Int; let build = fn(body) { body }; def loop = build(fn(n) { if n < 1 { 0 } else { loop(n - 1) } }); loop(3)",
        )
        .unwrap();
        assert_int(&higher_order, 0);

        let passed_as_value = run(
            "decl countdown: Fn(Int) -> Int; def countdown = fn(n) { if n < 1 { 0 } else { countdown(n - 1) } }; let invoke = fn(f, n) { f(n) }; invoke(countdown, 4)",
        )
        .unwrap();
        assert_int(&passed_as_value, 0);

        let named = run(
            "def loop: Fn(Int) -> Int = fn(n) { if n < 1 { 0 } else { loop(n - 1) } }; loop(3)",
        )
        .unwrap();
        assert_int(&named, 0);

        let annotated =
            run("def increment: Fn(Int) -> Int = fn(value) { value + 1 }; increment(41)").unwrap();
        assert_int(&annotated, 42);
    }

    #[test]
    fn recursive_definitions_capture_their_lexical_context() {
        let fib = run(
            "let base = 10; decl fib: Fn(Int) -> Int; def fib = fn(v) { if v < 2 { base } else { fib(v - 1) + fib(v - 2) } }; fib(4)",
        )
        .unwrap();
        assert_int(&fib, 50);

        let prior_let = run(
            "let base = 2; decl countdown: Fn(Int) -> Int; def countdown = fn(n) { if n < 1 { base } else { countdown(n - 1) } }; countdown(3)",
        )
        .unwrap();
        assert_int(&prior_let, 2);

        let nested = run(
            "let outer = 40; { let local = 2; decl answer: Fn(Int) -> Int; def answer = fn(n) { if n < 1 { outer + local } else { answer(n - 1) } }; answer(3) }",
        )
        .unwrap();
        assert_int(&nested, 42);
    }

    #[test]
    fn executes_inferred_direct_and_mutual_recursive_definitions() {
        let direct = run("def countdown = fn(value) {\
                 if value < 1 { 0 } else { countdown(value - 1) }\
             }; countdown(4)")
        .unwrap();
        assert_int(&direct, 0);

        let mutual = run("def even = fn(value) {\
                 if value < 1 { 'True } else { odd(value - 1) }\
             };\
             def odd = fn(value) {\
                 if value < 1 { 'False } else { even(value - 1) }\
             }; even(4)")
        .unwrap();
        assert_atom(&mutual, "True");
    }

    #[test]
    fn proper_tail_calls_cross_recursive_branches_and_match_arms() {
        let direct =
            run("def countdown: Fn(Int) -> Int = fn(n) { if n < 1 { 0 } else { countdown(n - 1) } }; countdown(1500)")
                .unwrap();
        assert_int(&direct, 0);

        let mutual = run(
            "decl even: Fn(Int) -> Int; decl odd: Fn(Int) -> Int; def even = fn(n) { if n < 1 { 0 } else { odd(n - 1) } }; def odd = fn(n) { if n < 1 { 1 } else { even(n - 1) } }; even(1500)",
        )
        .unwrap();
        assert_int(&mutual, 0);

        let matched = run(
            "def countdown: Fn(Int) -> Int = fn(n) { match n { 0 => 0, value => countdown(value - 1) } }; countdown(1500)",
        )
        .unwrap();
        assert_int(&matched, 0);

        let higher_order = run(
            "let iterate: Fn(Any, Int) -> Int = fn(step, n) { if n < 1 { 0 } else { step(step, n - 1) } }; iterate(iterate, 1500)",
        )
        .unwrap();
        assert_int(&higher_order, 0);

        let non_tail =
            run("def descend: Fn(Int) -> Int = fn(n) { if n < 1 { 0 } else { 1 + descend(n - 1) } }; descend(1500)")
                .unwrap_err();
        assert!(matches!(
            non_tail,
            ExecutionError::Runtime(RuntimeError {
                kind: RuntimeErrorKind::CallDepthExceeded,
                ..
            })
        ));
    }

    #[test]
    fn emits_contiguous_call_windows_and_structural_tail_calls() {
        let tail = compile_source("test", "let id = fn(x) { x }; id(1)").unwrap();
        assert!(matches!(
            tail.instructions().last(),
            Some(crate::Opcode::TailCall {
                argument_count: 1,
                ..
            })
        ));

        let non_tail =
            compile_source("test", "let id = fn(x) { x }; let value = id(1); value").unwrap();
        assert!(non_tail.instructions().iter().any(|instruction| matches!(
            instruction,
            crate::Opcode::Call {
                argument_count: 1,
                ..
            }
        )));
        assert!(matches!(
            non_tail.instructions().last(),
            Some(crate::Opcode::Return { .. })
        ));

        let branches = compile_source(
            "test",
            "let id = fn(x) { x }; if 'True { id(1) } else { id(2) }",
        )
        .unwrap();
        assert_eq!(
            branches
                .instructions()
                .iter()
                .filter(|instruction| matches!(instruction, crate::Opcode::TailCall { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn definition_contract_failures_keep_source_origins() {
        let missing = run("decl missing: Fn(Int) -> Int; 0").unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("declared but never initialized")
        );

        let non_function = run("decl value: Int; def value = value + 1; value").unwrap_err();
        assert!(
            non_function
                .to_string()
                .contains("decl requires a function contract")
        );

        let shadow = run("def value = 1; { def value = 2; value }").unwrap_err();
        assert!(shadow.to_string().contains("cannot shadow"));

        let let_shadow = run("let value = 1; def value = 2; value").unwrap_err();
        assert!(let_shadow.to_string().contains("cannot shadow"));

        let declaration_conflict =
            run("decl value: Int; let value = 1; def value = 2; value").unwrap_err();
        assert!(declaration_conflict.to_string().contains("cannot shadow"));

        let wrong_arity = run(
            "decl f: Fn(Int) -> Int; let build = fn(value) { value }; def f = build(fn(a, b) { a + b }); f",
        )
        .unwrap_err();
        let ExecutionError::Frontend(wrong_arity) = wrong_arity else {
            panic!("expected strict contract error");
        };
        assert!(
            wrong_arity
                .message
                .contains("cannot unify Fn(Any, Any) -> Any with Fn(Int) -> Int")
        );
        assert_eq!(wrong_arity.location.line, 1);
        assert_eq!(wrong_arity.location.column, 72);
    }

    #[test]
    fn let_may_shadow_a_definition_but_def_may_not_shadow() {
        assert_eq!(
            run("def value = 1; let value = 2; value")
                .unwrap()
                .to_string(),
            "2"
        );

        assert_eq!(
            run("decl value: Fn(Int) -> Int; def value = fn(x) { x + 1 }; let value = fn(x) { x + 2 }; value(1)")
                .unwrap()
                .to_string(),
            "3"
        );

        let shadow =
            run("let value = fn(x) { x }; def value = fn(x) { x + 1 }; value(1)").unwrap_err();
        assert!(shadow.to_string().contains("cannot shadow"));

        let hidden_declaration = run(
            "decl value: Fn(Int) -> Int; let value = fn(x) { x }; def value = fn(x) { x + 1 }; value(1)",
        )
        .unwrap_err();
        assert!(hidden_declaration.to_string().contains("cannot shadow"));
    }

    #[test]
    fn allocation_and_stack_quotas_keep_source_origins() {
        let source = "[1, 2]";
        let function = compile_source("quota.telora", source).unwrap();
        let mut sources = SourceDatabase::default();
        sources.add("quota.telora", source);
        let allocation = function
            .execute_with_quota(&mut Vm::new(), Quota::new(0, 100, 0))
            .unwrap_err()
            .with_sources(&sources);
        assert_eq!(allocation.kind, RuntimeErrorKind::AllocationQuotaExceeded);
        assert!(allocation.to_string().contains("quota.telora:1:1"));

        let stack = function
            .execute_with_quota(&mut Vm::new(), Quota::new(0, 1, u64::MAX))
            .unwrap_err()
            .with_sources(&sources);
        assert_eq!(stack.kind, RuntimeErrorKind::StackLimitExceeded);
        assert!(stack.to_string().contains("quota.telora:1:"));

        let native_source = "validate(Int, \"wrong\")";
        let native = compile_source("native-quota.telora", native_source).unwrap();
        let native_error = native
            .execute_with_quota(&mut Vm::new(), Quota::new(1, 100, 0))
            .unwrap_err();
        assert_eq!(native_error.kind, RuntimeErrorKind::AllocationQuotaExceeded);
    }

    #[test]
    fn captures_values_and_calls_closures() {
        let value = run("let base = 40; let add = fn(value) { base + value }; add(2)").unwrap();
        assert_int(&value, 42);
    }

    #[test]
    fn executes_partially_annotated_closures_without_runtime_annotation_work() {
        let value = run("(fn(value: Int) -> Int { value + 1 })(41)").unwrap();
        assert_int(&value, 42);
    }

    #[test]
    fn erases_explicit_type_application_from_runtime_calls() {
        let value = run("decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { value };\
             identity@[Int](42)")
        .unwrap();
        assert_int(&value, 42);
    }

    #[test]
    fn executes_inferred_generic_closures_without_runtime_instances() {
        let value = run("let identity = fn(value) { value };\
             (identity(42), identity(\"value\"), identity@[Int](7))")
        .unwrap();
        let items = value.value();
        assert_eq!(items.sequence_get(0).unwrap().as_int(), Some(42));
        assert_eq!(
            items.sequence_get(1).unwrap().as_str().as_deref(),
            Some("value")
        );
        assert_eq!(items.sequence_get(2).unwrap().as_int(), Some(7));
    }

    #[test]
    fn indexes_arrays_and_projects_tuples() {
        let value = run("let values = [10, 20, 30]; (values[1], (\"left\", 42).1)").unwrap();
        let items = value.value();
        assert_eq!(items.sequence_get(0).unwrap().as_int(), Some(20));
        assert_eq!(items.sequence_get(1).unwrap().as_int(), Some(42));
    }

    #[test]
    fn array_index_out_of_range_raises_sourced_blame() {
        for source in ["[1][-1]", "[1][1]", "[1][2]"] {
            let ExecutionError::Runtime(error) = run(source).unwrap_err() else {
                panic!("expected runtime failure for {source}");
            };
            assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
            assert_eq!(error.message, "OutOfRange");
            assert!(error.rule_location().is_some());
        }

        let function = compile_source("test", "[1][1]").unwrap();
        let array_only = std::mem::size_of::<Val>() as u64;
        let allocation = function
            .execute_with_quota(&mut Vm::new(), Quota::new(0, 100, array_only))
            .unwrap_err();
        assert_eq!(allocation.kind, RuntimeErrorKind::AllocationQuotaExceeded);

        let complete_failure = array_only
            .checked_mul(6)
            .and_then(|bytes| bytes.checked_add(15))
            .unwrap();
        let blame = function
            .execute_with_quota(&mut Vm::new(), Quota::new(0, 100, complete_failure))
            .unwrap_err();
        assert_eq!(blame.kind, RuntimeErrorKind::RaisedBlame);
    }

    #[test]
    fn dynamic_projection_boundaries_check_runtime_values() {
        assert_int(&run("let pair = (0, (1, \"ok\")); pair.1.0").unwrap(), 1);
        assert_int(&run("let values: Any = [1, 2]; values[1]").unwrap(), 2);
        assert_eq!(
            run("let pair: Any = (1, \"x\"); pair.1")
                .unwrap()
                .value()
                .as_str()
                .as_deref(),
            Some("x")
        );

        for source in [
            "let value: Any = 1; value[0]",
            "let values: Any = [1]; let index: Any = \"x\"; values[index]",
            "let value: Any = 1; value.0",
        ] {
            let ExecutionError::Runtime(error) = run(source).unwrap_err() else {
                panic!("expected runtime type mismatch for {source}");
            };
            assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
        }

        let ExecutionError::Runtime(error) = run("let pair: Any = (1, 2); pair.2").unwrap_err()
        else {
            panic!("expected dynamic tuple bounds failure");
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "OutOfRange");
    }

    #[test]
    fn projection_types_are_checked_statically() {
        let tuple_bounds = compile_source("test", "(1, \"x\").2").unwrap_err();
        assert!(tuple_bounds.message.contains("has no item at index 2"));

        let old_type_application = compile_source(
            "test",
            "decl identity: for(A) Fn(A) -> A; def identity = fn(value) { value }; identity[Int](1)",
        )
        .unwrap_err();
        assert!(
            old_type_application.message.contains("cannot index value"),
            "{}",
            old_type_application.message
        );
    }

    #[test]
    fn pipeline_is_uniform_reverse_application() {
        let value = run("let add = fn(a) { fn(b) { a + b } }; 40 |> add(2)").unwrap();
        assert_int(&value, 42);

        let chained = run("let ops = { increment: fn(value) { value + 1 } }; \
             40 |> ops.increment |> fn(value) { value + 1 }")
        .unwrap();
        assert_int(&chained, 42);

        let error = run("let add = fn(a, b) { a + b }; 40 |> add(2)").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("call expects 2 arguments, found 1")
        );
    }

    #[test]
    fn call_sections_elaborate_to_ordinary_closures() {
        let bare = run("let combine = fn(a, middle, b) { a + middle + b }; \
             let section = combine\\(_, 10, _); section(1, 2)")
        .unwrap();
        assert_int(&bare, 13);

        let reordered = run("let subtract = fn(a, b) { a - b }; \
             let flipped = subtract\\(_1, _0); flipped(2, 10)")
        .unwrap();
        assert_int(&reordered, 8);

        let repeated =
            run("let add = fn(a, b) { a + b }; let twice = add\\(_0, _0); twice(21)").unwrap();
        assert_int(&repeated, 42);

        let nested = run("let increment = fn(value) { value + 1 }; \
             let apply = fn(callback, value) { callback(value) }; \
             apply(increment\\(_), 41)")
        .unwrap();
        assert_int(&nested, 42);

        let piped = run("let add = fn(a, b) { a + b }; \
             40 |> add\\(_, 2)")
        .unwrap();
        assert_int(&piped, 42);

        let native = run("let array_type = Array\\(_); array_type(Int)").unwrap();
        assert_eq!(
            native.value().dict_get("kind").unwrap().to_string(),
            "'Array"
        );

        let reevaluated = run("let second = fn(first, second) { second }; \
             let make: Fn() -> Fn(Any) -> Any = fn() { fn(value) { value } }; \
             let section = second\\(_, make()); \
             section(1) == section(2)")
        .unwrap();
        assert_atom(&reevaluated, "False");
    }

    #[test]
    fn interpolates_strings_numbers_and_atoms() {
        let value = run(
            r#"let name = "Ada"; let count = 3; let ratio = 3.0; let small = 1.25e-3; let zero = -0.0; let state = 'Ok; `hi, \{name} count=\{count} ratio=\{ratio} small=\{small} zero=\{zero} state=\{state}`"#,
        )
        .unwrap();
        assert_eq!(
            value.value().as_str().as_deref(),
            Some("hi, Ada count=3 ratio=3 small=0.00125 zero=-0 state=Ok")
        );

        let nested = run(r#"`value=\{if 'True { "yes" } else { "no" }}`"#).unwrap();
        assert_eq!(nested.value().as_str().as_deref(), Some("value=yes"));
    }

    #[test]
    fn evaluates_escaped_raw_and_continued_strings() {
        let value = run(r####"("A=\x41, shape=\u{5f62}, first \
                second", r##"raw \n "quote" and #"##)"####)
        .unwrap();
        assert_eq!(
            value.to_string(),
            "(\"A=A, shape=形, first second\", \"raw \\\\n \\\"quote\\\" and #\")"
        );
    }

    #[test]
    fn checks_known_and_dynamic_unsupported_interpolation_values() {
        let static_error = run(r#"`items=\{[1, 2]}`"#).unwrap_err();
        assert!(
            static_error
                .to_string()
                .contains("requires std/fmt.Display"),
            "{static_error}"
        );

        let dynamic_error = run(r#"def render = fn(x) { `x=\{x}` }; render([1])"#).unwrap_err();
        assert!(
            dynamic_error
                .to_string()
                .contains("interpolation type remains unresolved"),
            "{dynamic_error}"
        );
    }

    #[test]
    fn if_evaluates_only_the_selected_branch() {
        let value = run("if 'True { 42 } else { 1 / 0 }").unwrap();
        assert_int(&value, 42);
    }

    #[test]
    fn control_flow_else_chains_evaluate_like_nested_expressions() {
        let cases = [
            "if 'False { 1 } else if 'False { 2 } else if 'True { 3 } else { 4 }",
            "if 'False { 1 } else if let 'Some(value) = 'Some(3) { value } else { 4 }",
            "let choose = fn(value: Bool) { if 'False { 1 } else match value { 'True => 3, 'False => 4 } }; choose('True)",
        ];
        for source in cases {
            assert_int(&run(source).unwrap(), 3);
        }

        let returned = run(
            "let choose = fn(condition: Bool) { if condition { 3 } else return 4; }; (choose('True), choose('False))",
        )
        .unwrap();
        assert_eq!(returned.to_string(), "(3, 4)");
    }

    #[test]
    fn match_destructures_tagged_tuples() {
        let value = run("match ('Ok, 42) { ('Ok, value) => value }").unwrap();
        assert_int(&value, 42);
    }

    #[test]
    fn atom_call_constructs_tagged_value_and_pattern_destructures_it() {
        let value = run("let Some = 'Some; let option: Option(Int) = Some(42);\
             match option { 'None => 0, 'Some(value) => value }")
        .unwrap();
        assert_int(&value, 42);
    }

    #[test]
    fn struct_patterns_select_nested_fields_and_fall_through_dynamically() {
        let selected = run("let user = {name: \"Ada\", address: {city: \"London\"}};\
             match user { {name, address: {city}} => (name, city) }")
        .unwrap();
        assert_eq!(selected.to_string(), "(\"Ada\", \"London\")");

        let fallback = run("let select: Fn(Any) -> String = fn(value) {\
                match value { {name} => name, _ => \"fallback\" }\
             }; select(1)")
        .unwrap();
        assert_eq!(fallback.to_string(), "\"fallback\"");

        let empty = run("let is_struct: Fn(Any) -> Bool = fn(value) {\
                match value { {} => 'True, _ => 'False }\
             }; (is_struct({}), is_struct(1))")
        .unwrap();
        assert_eq!(empty.to_string(), "('True, 'False)");
    }

    #[test]
    fn local_destructuring_let_preserves_order_scope_and_nested_selection() {
        let value = run("let outer = \"outer\"; {
            let (left, user) = (1, {name: \"Ada\", address: {city: \"London\"}});
            let {name, address: {city}} = user;
            let outer = name;
            (left, outer, city)
        }")
        .unwrap();
        assert_eq!(value.to_string(), "(1, \"Ada\", \"London\")");
    }

    #[test]
    fn propagates_option_and_result_from_the_nearest_function() {
        let option = run("let step: Fn(Option(Int)) -> Option(Int) = fn(value) { let item = value?; 'Some(item + 1) }; (step('Some(2)), step('None))").unwrap();
        assert_eq!(option.to_string(), "('Some(3), 'None)");

        let result = run("let step: Fn(Result(Int, String)) -> Result(Int, String) = fn(value) { let item = value?; 'Ok(item + 1) }; (step('Ok(2)), step('Err(\"bad\")))").unwrap();
        assert_eq!(result.to_string(), "('Ok(3), 'Err(\"bad\"))");
    }

    #[test]
    fn infers_propagation_boundary_from_success_constructor() {
        let value = run("let step = fn(value: Option(Int)) { let item = { value? }; 'Some(item + 1) }; (step('Some(1)), step('None))").unwrap();
        assert_eq!(value.to_string(), "('Some(2), 'None)");
    }

    #[test]
    fn propagates_from_module_blocks_and_isolates_nested_functions() {
        let module =
            run("{ let value: Option(Int) = 'None; let item = value?; 'Some(item) }").unwrap();
        assert_eq!(module.to_string(), "'None");

        let nested = run("let outer = fn(value: Option(Int)) { let inner: Fn(Option(Int)) -> Option(Int) = fn(inner_value) { let item = inner_value?; 'Some(item + 1) }; 'Some(inner(value)) }; outer('None)").unwrap();
        assert_eq!(nested.to_string(), "'Some('None)");
    }

    #[test]
    fn rejects_mixed_and_unsupported_propagation() {
        let mixed = compile_source("test", "let f = fn(a: Option(Int), b: Result(Int, String)) { let x = a?; let y = b?; 'Some(x + y) }; f").unwrap_err();
        assert!(
            mixed.message.contains("cannot mix Option and Result"),
            "{}",
            mixed.message
        );

        let unsupported =
            compile_source("test", "let f = fn(value: Bool) { value? }; f").unwrap_err();
        assert!(
            unsupported
                .message
                .contains("Option-shaped or Result-shaped"),
            "{}",
            unsupported.message
        );
    }

    #[test]
    fn returns_values_from_the_nearest_function() {
        let value = run("let choose = fn(condition: Bool, value: Int, fallback: Int) { if condition { return value; } else { fallback } }; (choose('True, 1, 2), choose('False, 1, 2))").unwrap();
        assert_eq!(value.to_string(), "(1, 2)");

        let nested =
            run("let outer = fn() { let inner = fn() { return 1; }; inner() + 1 }; outer()")
                .unwrap();
        assert_eq!(nested.to_string(), "2");
    }

    #[test]
    fn rejects_return_outside_functions_and_wrong_result_types() {
        let module = compile_source("test", "return 1;").unwrap_err();
        assert!(module.message.contains("only inside a Function"));

        let wrong = compile_source("test", "let f: Fn(Bool) -> Int = fn(condition) { if condition { return \"wrong\"; } else { 1 } }; f").unwrap_err();
        assert!(wrong.message.contains("String") && wrong.message.contains("Int"));
    }

    #[test]
    fn panic_is_a_sourced_never_expression() {
        let value = run("if 'False { panic!(\"unused\") } else { 3 }").unwrap();
        assert_eq!(value.to_string(), "3");

        let error = run("let fail = fn() {\n  panic!(\"broken\")\n};\nfail()").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime panic")
        };
        assert_eq!(error.kind, RuntimeErrorKind::Panic);
        assert_eq!(error.message, "broken");
        assert!(error.to_string().contains("test:2:3"));
    }

    #[test]
    fn panic_requires_one_string_message() {
        let wrong_type = compile_source("test", "panic!(1)").unwrap_err();
        assert!(wrong_type.message.contains("Int") && wrong_type.message.contains("String"));

        let wrong_arity = compile_source("test", "panic!()").unwrap_err();
        assert!(wrong_arity.message.contains("exactly one argument"));
    }

    #[test]
    fn fail_preserves_structured_diagnostic_locations() {
        let source = "let stop = fn() {\n  let data = 1;\n  fail!(\"bad\", data)\n};\nstop()";
        let error = run(source).unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected raised blame")
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "bad");
        assert_eq!(
            &source[error.data_location().expect("data location").range()],
            "1"
        );
        assert_eq!(
            &source[error.rule_location().expect("rule location").range()],
            "stop()"
        );
        assert_eq!(
            &source[error
                .implementation_rule_location()
                .expect("implementation rule location")
                .range()],
            "fail!(\"bad\", data)"
        );
        let diagnostic = error.diagnostic().expect("structured diagnostic");
        assert_eq!(
            &source[diagnostic
                .labels
                .iter()
                .find(|label| label.primary)
                .expect("primary label")
                .location
                .range()],
            "stop()"
        );
        assert!(error.trace.iter().any(|frame| frame.origin.is_some()));
    }

    #[test]
    fn direct_fail_uses_its_own_rule_location() {
        let source = "fail!(\"bad\", 1)";
        let ExecutionError::Runtime(error) = run(source).unwrap_err() else {
            panic!("expected raised blame")
        };
        assert_eq!(
            &source[error.rule_location().expect("rule location").range()],
            source
        );
        assert_eq!(
            &source[error.data_location().expect("data location").range()],
            "1"
        );
    }

    #[test]
    fn fail_retains_ordered_unique_subject_locations_without_expanding_values() {
        let source = "let shared = 1; fail!(\"bad\", shared, 2, shared, {nested: 3})";
        let ExecutionError::Runtime(error) = run(source).unwrap_err() else {
            panic!("expected raised blame")
        };
        let subjects = error
            .data_sources()
            .iter()
            .map(|location| &source[location.range()])
            .collect::<Vec<_>>();
        assert_eq!(subjects, ["1", "2", "{nested: 3}"]);
    }

    #[test]
    fn fail_keeps_the_outermost_rule_boundary_through_tail_calls() {
        let source = "let leaf = fn(value) { fail!(\"bad\", value) };\n\
            let middle = fn(value) { leaf(value) };\n\
            let outer = fn(value) { middle(value) };\n\
            outer(7)";
        let ExecutionError::Runtime(error) = run(source).unwrap_err() else {
            panic!("expected raised blame")
        };
        assert_eq!(
            &source[error.rule_location().expect("rule location").range()],
            "outer(7)"
        );
        assert_eq!(
            &source[error
                .implementation_rule_location()
                .expect("implementation rule location")
                .range()],
            "fail!(\"bad\", value)"
        );
        assert_eq!(
            &source[error.data_location().expect("data location").range()],
            "7"
        );
    }

    #[test]
    fn fail_requires_a_string_message() {
        let wrong_type = compile_source("test", "fail!(1)").unwrap_err();
        assert!(
            wrong_type.message.contains("Int") && wrong_type.message.contains("message"),
            "{}",
            wrong_type.message
        );

        let wrong_arity = compile_source("test", "fail!()").unwrap_err();
        assert!(wrong_arity.message.contains("message followed by"));
    }

    #[test]
    fn fail_accepts_heterogeneous_variadic_subjects() {
        let error = run("fail!(\"different subjects\", 1, \"two\", 'Three)").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected raised blame")
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "different subjects");
    }

    #[test]
    fn type_ascription_is_bidirectional_and_emits_no_runtime_call() {
        for source in ["ty!([], Array(Int))", "[].ty!(Array(Int))"] {
            let function = compile_source("test", source).unwrap();
            assert!(
                !function
                    .instructions()
                    .iter()
                    .any(|instruction| matches!(instruction, crate::Opcode::Call { .. })),
                "{source} emitted a runtime Call"
            );
            assert_eq!(run(source).unwrap().to_string(), "[]");
        }

        let truth = run("'True.ty!(Bool)").unwrap();
        assert_eq!(truth.to_string(), "'True");

        let invalid = compile_source("test", "\"1\".ty!(Int)").unwrap_err();
        assert!(
            invalid.message.contains("String") && invalid.message.contains("Int"),
            "{}",
            invalid.message
        );
    }

    #[test]
    fn checked_cast_preserves_representation_and_nominal_identity() {
        for source in ["cast!(1, Int)", "1.cast!(Int)"] {
            assert_eq!(run(source).unwrap().to_string(), "'Ok(1)");
        }
        assert_eq!(
            run("\"1\".cast!(Int)").unwrap().to_string(),
            "'Err(\"value must be Int, got String\")"
        );
        assert_eq!(
            run("1.cast!(Float)").unwrap().to_string(),
            "'Err(\"value must be Float, got Int\")"
        );

        let raw = run("type User = struct {id: Int, name: String};\
             {id: 1, name: \"Ada\"}.cast!(User)")
        .unwrap();
        assert_eq!(raw.to_string(), "'Ok({id: 1, name: \"Ada\"})");

        let conflict = run("type A = struct {value: Int};\
             type B = struct {value: Int};\
             let a: A = {value: 1};\
             a.cast!(B)")
        .unwrap();
        assert_eq!(
            conflict.to_string(),
            "'Err(\"value has a different declared type identity\")"
        );

        let nested = run("type Address = struct {zip: Int};\
             type User = struct {address: Address};\
             {address: {zip: \"bad\"}}.cast!(User)")
        .unwrap();
        assert!(nested.to_string().contains("value.address.zip"));
    }

    #[test]
    fn checked_cast_propagates_fail_and_any_narrowing_is_explicit() {
        let failure = run("fail!(\"cast input failed\", 1).cast!(Int)").unwrap_err();
        let ExecutionError::Runtime(failure) = failure else {
            panic!("expected propagated failure")
        };
        assert_eq!(failure.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(failure.message, "cast input failed");

        run("let concrete: Int = 1; let erased: Any = concrete; erased").unwrap();
        let unchecked = compile_source(
            "test",
            "let erased: Any = 1; let value: Int = erased; value",
        )
        .unwrap_err();
        assert!(
            unchecked.message.contains("Any") && unchecked.message.contains("Int"),
            "{}",
            unchecked.message
        );

        for source in [
            "let erased: Any = 1; let consume: Fn(Int) -> Int = fn(value) { value }; consume(erased)",
            "let broken: Fn(Any) -> Int = fn(value) { value }; broken",
            "let erase: Fn(Int) -> Any = fn(value) { value }; let concrete: Int = erase(1); concrete",
            "let erased: Array(Any) = [1]; let concrete: Array(Int) = erased; concrete",
        ] {
            let error = compile_source("test", source).unwrap_err();
            assert!(
                error.message.contains("Any") && error.message.contains("cast!"),
                "{source}: {}",
                error.message
            );
        }
    }

    #[test]
    fn check_records_a_warning_and_returns_option() {
        let function = compile_source(
            "test",
            "let reject: Fn(Int, String) -> Result(Int, String) = fn(a, b) { 'Err(\"warning\") }; reject.should_ok!(1, \"two\")",
        )
        .unwrap();
        let mut account = crate::QuotaAccount::new(crate::Quota::with_fuel(100_000));
        let value = function
            .execute_with_account(&mut Vm::new(), &mut account)
            .unwrap();
        assert_eq!(value.to_string(), "'None");
        assert_eq!(account.diagnostics().len(), 1);
        assert_eq!(
            account.diagnostics()[0].severity,
            crate::source::Severity::Warning
        );
        assert_eq!(account.diagnostics()[0].labels.len(), 3);

        let discarded = compile_source(
            "test",
            "let reject: Fn(Int) -> Result(Int, String) = fn(value) { 'Err(\"discarded\") }; let ignored = reject.should_ok!(1); 0",
        )
        .unwrap();
        let mut account = crate::QuotaAccount::new(crate::Quota::with_fuel(100_000));
        let value = discarded
            .execute_with_account(&mut Vm::new(), &mut account)
            .unwrap();
        assert_eq!(value.to_string(), "0");
        assert_eq!(account.diagnostics().len(), 1);
        assert_eq!(account.diagnostics()[0].message, "discarded");
    }

    #[test]
    fn check_and_fail_have_distinct_control_flow() {
        let function = compile_source(
            "test",
            "let accept: Fn(Int) -> Result(Int, String) = fn(value) { 'Ok(value + 1) }; accept.must_ok!(6)",
        )
        .unwrap();
        let mut account = crate::QuotaAccount::new(crate::Quota::with_fuel(100_000));
        let value = function
            .execute_with_account(&mut Vm::new(), &mut account)
            .unwrap();
        assert_eq!(value.to_string(), "7");
        assert!(account.diagnostics().is_empty());

        let source = "let reject: Fn(Int, String) -> Result(Int, String) = fn(a, b) { 'Err(\"rejected\") }; reject.must_ok!(1, \"two\")";
        let error = run(source).unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected check failure")
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "rejected");
        assert_eq!(
            &source[error.rule_location().expect("rule location").range()],
            "reject.must_ok!(1, \"two\")"
        );
        assert_eq!(
            error
                .data_sources()
                .iter()
                .map(|location| &source[location.range()])
                .collect::<Vec<_>>(),
            ["1", "\"two\""]
        );

        let error = run("fail!(\"stopped\", 42)").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected raised blame")
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "stopped");
    }

    #[test]
    fn result_unwrap_intrinsics_choose_recovery_policy() {
        let value = run("let result: Result(Int, String) = 'Ok(7); result.try_unwrap!()").unwrap();
        assert_eq!(value.to_string(), "'Some(7)");
        let value = run("let result: Result(Int, String) = 'Ok(7); result.unwrap!()").unwrap();
        assert_eq!(value.to_string(), "7");

        let function = compile_source(
            "test",
            "let result: Result(Int, String) = 'Err(\"recoverable\"); result.try_unwrap!()",
        )
        .unwrap();
        let mut account = crate::QuotaAccount::new(crate::Quota::with_fuel(100_000));
        let value = function
            .execute_with_account(&mut Vm::new(), &mut account)
            .unwrap();
        assert_eq!(value.to_string(), "'None");
        assert_eq!(account.diagnostics().len(), 1);
        assert_eq!(account.diagnostics()[0].message, "recoverable");

        let source = "let result: Result(Int, String) = 'Err(\"required\"); result.unwrap!()";
        let error = run(source).unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected unwrap failure")
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "required");
        assert_eq!(
            &source[error.rule_location().expect("rule location").range()],
            "result.unwrap!()"
        );
        assert_eq!(
            &source[error.data_location().expect("data location").range()],
            "'Err(\"required\")"
        );
    }

    #[test]
    fn diagnostic_convenience_intrinsics_require_string_messages() {
        for source in [
            "let invalid: Fn() -> Result(Int, Int) = fn() { 'Err(1) }; invalid.should_ok!()",
            "let invalid: Fn() -> Result(Int, Int) = fn() { 'Err(1) }; invalid.must_ok!()",
            "fail!(1)",
        ] {
            let error = compile_source("test", source).unwrap_err();
            assert!(
                error.message.contains("Int") && error.message.contains("String"),
                "{source}: {}",
                error.message
            );
        }
    }

    #[test]
    fn if_let_selects_and_scopes_structural_patterns() {
        let some = run(
            "let value: Option(Int) = 'Some(3); if let 'Some(item) = value { item + 1 } else { 0 }",
        )
        .unwrap();
        assert_eq!(some.to_string(), "4");

        let none = run(
            "let value: Option(Int) = 'None; if let 'Some(item) = value { item + 1 } else { 0 }",
        )
        .unwrap();
        assert_eq!(none.to_string(), "0");

        let error =
            compile_source("test", "if let 'Some(item) = 1 { item } else { 0 }").unwrap_err();
        assert!(error.message.contains("pattern cannot match Int"));
    }

    #[test]
    fn let_else_binds_the_remaining_block_and_requires_divergence() {
        let value = run("let step: Fn(Option(Int)) -> Option(Int) = fn(option) { let 'Some(item) = option else { return 'None; }; 'Some(item + 1) }; (step('Some(2)), step('None))").unwrap();
        assert_eq!(value.to_string(), "('Some(3), 'None)");

        let panic = run("let require = fn(option: Option(Int)) { let 'Some(item) = option else { panic!(\"none\") }; item }; require('Some(4))").unwrap();
        assert_eq!(panic.to_string(), "4");

        let non_never = compile_source(
            "test",
            "let f = fn(option: Option(Int)) { let 'Some(item) = option else { 0 }; item }; f",
        )
        .unwrap_err();
        assert!(non_never.message.contains("must have type Never"));

        let irrefutable = compile_source("test", "type Pair = struct {a: Int, b: Int}; let f = fn(pair: Pair) { let {a, b} = pair else { panic!(\"never\") }; a + b }; f").unwrap_err();
        assert!(
            irrefutable.message.contains("irrefutable"),
            "{}",
            irrefutable.message
        );
    }

    #[test]
    fn boolean_operators_short_circuit_and_preserve_precedence() {
        let value =
            run("('False && (1 / 0 == 0), 'True || (1 / 0 == 0), 'False || 'True && 'True)")
                .unwrap();
        assert_eq!(value.to_string(), "('False, 'True, 'True)");

        let error = compile_source("test", "'True && 1").unwrap_err();
        assert!(error.message.contains("Int"), "{}", error.message);
    }

    #[test]
    fn logical_negation_executes_with_unary_precedence_and_dynamic_checks() {
        let value =
            run("(!'True, !'False, !!'True, !('True && 'False), !'False == 'True, !0, !-1)")
                .unwrap();
        assert_eq!(
            value.to_string(),
            "('False, 'True, 'True, 'True, 'True, -1, 0)"
        );

        let dynamic = run("let invert: Fn(Any) -> Bool = fn(value) { !value };\
             (invert('True), invert('False))")
        .unwrap();
        assert_eq!(dynamic.to_string(), "('False, 'True)");

        let ExecutionError::Runtime(error) =
            run("let invert: Fn(Any) -> Bool = fn(value) { !value }; invert(1)").unwrap_err()
        else {
            panic!("expected runtime Bool check");
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);

        let dynamic = run("let invert: Fn(Any) -> Any = fn(value) { !value };\
             (invert('True), invert(0))")
        .unwrap();
        assert_eq!(dynamic.to_string(), "('False, -1)");

        let ExecutionError::Runtime(error) =
            run("let invert: Fn(Any) -> Int = fn(value) { !value }; invert('True)").unwrap_err()
        else {
            panic!("expected runtime Int check");
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
    }

    #[test]
    fn bitwise_integer_operators_execute_with_stable_precedence() {
        let value = run("(6 & 3, 4 | 1, 6 ^ 3, 1 | 2 ^ 3 & 1, 6 & 3 == 2)").unwrap();
        assert_eq!(value.to_string(), "(2, 5, 5, 3, 'True)");

        for source in ["1 & 1.0", "1 | 'True", "\"x\" ^ 1"] {
            let error = compile_source("test", source).unwrap_err();
            assert!(error.message.contains("Int"), "{}", error.message);
        }

        let ExecutionError::Runtime(error) =
            run("let bit_and: Fn(Any, Any) -> Int = fn(left, right) { left & right }; bit_and(1, \"x\")")
                .unwrap_err()
        else {
            panic!("expected runtime Int check");
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
    }
