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
    fn field_and_interpolation_errors_render_their_expressions() {
        let field = run("let value = {present: 1};\nvalue.missing").unwrap_err();
        assert!(field.to_string().contains("test:2:1"));

        let interpolation =
            run("def render = fn(value) {\n  `value=\\{value}`\n};\nrender([1])").unwrap_err();
        assert!(
            interpolation
                .to_string()
                .contains("test:2:12: string interpolation type remains unresolved"),
            "{interpolation}"
        );
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

    #[test]
    fn bounded_generic_calls_forward_hidden_trait_evidence() {
        let output = run(
            r#"trait Display { display: Fn(Self) -> String };
               impl Display for Int { display: fn(value) { `int=\{value}` } };
               def render: for(T: Display) Fn(T) -> String = fn(value) {
                   Display.display(value)
               };
               def outer: for(T: Display) Fn(T) -> String = fn(value) {
                   render(value)
               };
               outer(42)"#,
        )
        .unwrap();
        assert_eq!(output.to_string(), "\"int=42\"");

        let explicit = run(
            r#"trait Display { display: Fn(Self) -> String };
               impl Display for Int { display: fn(value) { `int=\{value}` } };
               def render: for(T: Display) Fn(T) -> String = fn(value) {
                   Display.display(value)
               };
               render@[Int](7)"#,
        )
        .unwrap();
        assert_eq!(explicit.to_string(), "\"int=7\"");
    }
