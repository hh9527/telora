    #[test]
    fn remainder_supports_int_float_precedence_and_dynamic_boundaries() {
        let value = run("(7 % 3, -7 % 3, 7 % -3, -7 % -3, \
             5.5 % 2.0, -5.5 % 2.0, 5.5 % -2.0, \
             2 + 7 % 4 * 3, 20 / 3 % 2, 20 % 6 * 2)")
        .unwrap();
        assert_eq!(
            value.to_string(),
            "(1, -1, 1, -1, 1.5, -1.5, 1.5, 11, 0, 4)"
        );

        let dynamic =
            run("let rem: Fn(Any, Any) -> Int = fn(left, right) { left % right }; rem(7, 3)")
                .unwrap();
        assert_eq!(dynamic.to_string(), "1");

        for source in ["1 % 1.0", "\"x\" % 1"] {
            let error = compile_source("test", source).unwrap_err();
            assert!(
                error.message.contains("Int or Float") || error.message.contains("cannot unify"),
                "{source}: {}",
                error.message
            );
        }

        let ExecutionError::Runtime(error) =
            run("let rem: Fn(Any, Any) -> Int = fn(left, right) { left % right }; rem(7, \"x\")")
                .unwrap_err()
        else {
            panic!("expected runtime numeric type error")
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
    }

    #[test]
    fn remainder_uses_existing_numeric_failure_paths() {
        let ExecutionError::Runtime(error) = run("7 % 0").unwrap_err() else {
            panic!("expected Int zero-divisor failure")
        };
        assert_eq!(error.kind, RuntimeErrorKind::DivisionByZero);
        assert_eq!(error.message, "integer remainder by zero");
        assert!(error.origin().is_some());

        let ExecutionError::Runtime(error) = run("(-9223372036854775807 - 1) % -1").unwrap_err()
        else {
            panic!("expected Int remainder overflow")
        };
        assert_eq!(error.kind, RuntimeErrorKind::IntegerOverflow);

        for source in ["7.0 % 0.0", "7.0 % -0.0"] {
            let ExecutionError::Runtime(error) = run(source).unwrap_err() else {
                panic!("expected Float non-finite failure")
            };
            assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
            assert_eq!(error.message, "NonFiniteFloat");
            assert!(error.data_location().is_some());
            assert!(error.rule_location().is_some());
            assert_eq!(error.origin(), error.rule_location().map(Origin::Source));
        }
    }

    #[test]
    fn match_guards_use_pattern_bindings_and_continue_after_false() {
        let value = run("let value: Option(Int) = 'Some(3); match value {\
                'Some(item) if 4 < item => 40,\
                'Some(item) if 2 < item && item < 4 => item,\
                'Some(_) => 0,\
                'None => -1,\
            }")
        .unwrap();
        assert_eq!(value.to_string(), "3");
    }

    #[test]
    fn match_guards_require_bool() {
        let error = compile_source(
            "test",
            "let value: Option(Int) = 'Some(1); match value {\
                'Some(item) if item => item,\
                _ => 0,\
            }",
        )
        .unwrap_err();
        assert!(
            error.message.contains("Int")
                && error.message.contains("False")
                && error.message.contains("True"),
            "{}",
            error.message
        );
    }

    #[test]
    fn guarded_match_arms_do_not_establish_exhaustiveness() {
        let error = compile_source(
            "test",
            "let value: Option(Int) = 'Some(1); match value {\
                'Some(item) if 'True => item,\
                'None if 'True => 0,\
            }",
        )
        .unwrap_err();
        assert!(
            error.message.contains("non-exhaustive match"),
            "{}",
            error.message
        );
    }

    #[test]
    fn match_guard_redundancy_depends_only_on_unguarded_coverage() {
        compile_source(
            "test",
            "let value: Option(Int) = 'Some(1); match value {\
                'Some(item) if 0 < item => item,\
                'Some(item) => item,\
                'None => 0,\
            }",
        )
        .unwrap();

        let error = compile_source(
            "test",
            "let value: Option(Int) = 'Some(1); match value {\
                'Some(item) => item,\
                'Some(item) if 0 < item => item,\
                'None => 0,\
            }",
        )
        .unwrap_err();
        assert!(
            error.message.contains("unreachable match arm"),
            "{}",
            error.message
        );
    }

    #[test]
    fn array_spread_flattens_fragments_in_source_order() {
        let value =
            run("let middle = [1, 2]; let empty: Array(Int) = []; [0, ...middle, 3, ...empty, 4]")
                .unwrap();
        assert_eq!(value.to_string(), "[0, 1, 2, 3, 4]");

        let nested = run("let values = [[1], [2, 3]]; [...values]").unwrap();
        assert_eq!(nested.to_string(), "[[1], [2, 3]]");
    }

    #[test]
    fn array_spread_requires_an_array_operand() {
        let error = compile_source("test", "[0, ...1]").unwrap_err();
        assert!(
            error.message.contains("array spread requires Array") && error.message.contains("Int"),
            "{}",
            error.message
        );
    }

    #[test]
    fn dict_spread_merges_in_source_order_with_later_values_winning() {
        let value = run("let base: Dict(Int) = {a: 1, b: 2};\
             let extra: Dict(Int) = {b: 3, c: 4};\
             {...base, x: 0, ...extra, c: 5}")
        .unwrap();
        assert_eq!(value.to_string(), "{a: 1, b: 3, c: 5, x: 0}");

        let contextual = run("let value: Dict(Int) = {...{a: 1}, b: 2}; value").unwrap();
        assert_eq!(contextual.to_string(), "{a: 1, b: 2}");
    }

    #[test]
    fn dict_field_shorthand_lowers_to_an_ordinary_named_field() {
        let value = run("let name = \"telora\"; let version = 1; { name, version }").unwrap();
        assert_eq!(value.to_string(), "{name: \"telora\", version: 1}");

        let mixed = run("let name = 1; let extra: Dict(Int) = {version: 2};\
             {name, explicit: 3, ...extra}")
        .unwrap();
        assert_eq!(mixed.to_string(), "{explicit: 3, name: 1, version: 2}");

        let duplicate = compile_source("test", "let name = 1; {name, name: 2}").unwrap_err();
        assert!(duplicate.message.contains("duplicate Dict field"));

        let unknown = compile_source("test", "{missing}").unwrap_err();
        assert!(
            unknown.message.contains("unknown binding") && unknown.message.contains("missing"),
            "{}",
            unknown.message
        );
    }

    #[test]
    fn dict_spread_requires_dict_without_adding_struct_update() {
        let error = compile_source("test", "let base = {a: 1}; {...base, b: 2}").unwrap_err();
        assert!(
            error.message.contains("Dict spread requires Dict")
                && error.message.contains("{a: Int}"),
            "{}",
            error.message
        );

        let duplicate =
            compile_source("test", "let base: Dict(Int) = {}; {...base, a: 1, a: 2}").unwrap_err();
        assert!(duplicate.message.contains("duplicate Dict field"));
    }

    #[test]
    fn non_exhaustive_match_has_a_dedicated_error() {
        let error =
            run("let fail: Fn(Any) -> Int = fn(value) { match value { 'Some => 1 } }; fail('None)")
                .unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error");
        };
        assert_eq!(error.kind, RuntimeErrorKind::NoPatternMatched);
    }

    #[test]
    fn reports_unknown_bindings_and_arity_errors() {
        let unknown = compile_source("test", "let present = 1;\nmissing").unwrap_err();
        assert!(unknown.message.contains("unknown binding"));
        assert_eq!(unknown.location.line, 2);
        assert_eq!(unknown.location.column, 1);

        let error = run("let f = fn(a) { a }; f(1, 2)").unwrap_err();
        let ExecutionError::Frontend(error) = error else {
            panic!("expected frontend error");
        };
        assert!(error.message.contains("call expects 1 arguments, found 2"));
    }

    #[test]
    fn runtime_errors_retain_expression_origins_and_call_trace() {
        let error =
            run("let divide = fn(x) {\n  x / 0\n};\nlet result = divide(4); result").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error");
        };
        assert_eq!(error.kind, RuntimeErrorKind::DivisionByZero);
        assert_eq!(error.trace.len(), 2);
        let Origin::Source(location) = error.origin().expect("runtime origin") else {
            panic!("expected source origin");
        };
        assert_eq!(location.start, 23);
        assert!(error.to_string().contains("test:2:3"));

        let tail = run("let divide = fn(x) { x / 0 }; divide(4)").unwrap_err();
        let ExecutionError::Runtime(tail) = tail else {
            panic!("expected runtime error");
        };
        assert_eq!(tail.trace.len(), 1);
    }

    #[test]
    fn runtime_field_and_interpolation_errors_render_their_expressions() {
        let field = run("let value = {present: 1};\nvalue.missing").unwrap_err();
        assert!(field.to_string().contains("test:2:1"));

        let interpolation =
            run("def render = fn(value) {\n  `value=\\{value}`\n};\nrender([1])").unwrap_err();
        assert!(interpolation.to_string().contains("test:2:3"));
    }

    #[test]
    fn generated_function_results_rebase_to_the_authored_call_site() {
        let source = "def inner: Fn() -> Any = fn() { 1 + 1 };\ndef outer: Fn() -> Any = fn() { inner() };\nlet value = outer();\nvalue.missing";
        let call_start = source.find("outer();").unwrap();
        let error = run(source).unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error");
        };
        let data = error.data_location().expect("generated value location");
        assert_eq!(data.range(), call_start..call_start + "outer()".len());
    }

    #[test]
    fn fuel_exhaustion_points_to_the_call_expression() {
        let error = run_source("test", "let f = fn() { 1 };\nf()", 0).unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error");
        };
        assert_eq!(error.kind, RuntimeErrorKind::FuelExhausted);
        assert!(error.to_string().contains("test:2:1"));
    }
