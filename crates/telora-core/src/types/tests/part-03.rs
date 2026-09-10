    #[test]
    fn declared_family_applications_use_head_and_argument_identity() {
        let analysis = analyze_source(
            "family-identities.telora",
            "type Box(A) = struct {value: A};\
             type Other(A) = struct {value: A};\
             type Phantom(A) = struct {value: Int};\
             type Maybe(A) = enum {None, Some(A)};\
             type IntBox = Box(Int);\
             type IntBoxAlias = Box(Int);\
             type Text = Box(String);\
             type EqualShape = Other(Int);\
             type PhantomInt = Phantom(Int);\
             type PhantomText = Phantom(String);\
             type Nested = Box(Maybe(Int));\
             type Optional = Maybe(Int);\
             0",
        )
        .unwrap();
        let declared_id = |name: &str| {
            let TypeNode::Declared { id, .. } = analysis.types.node(analysis.declared_types[name])
            else {
                panic!("{name} must be a declared family application")
            };
            id
        };

        assert_eq!(declared_id("IntBox"), declared_id("IntBoxAlias"));
        assert_ne!(declared_id("IntBox"), declared_id("Text"));
        assert_ne!(declared_id("IntBox"), declared_id("EqualShape"));
        assert_ne!(declared_id("PhantomInt"), declared_id("PhantomText"));
        assert_eq!(declared_id("Nested").arguments().len(), 1);
        assert_eq!(declared_id("Optional").arguments().len(), 1);
        assert_ne!(declared_id("Nested"), declared_id("Optional"));

        let error = analyze_source(
            "phantom-mismatch.telora",
            "type Phantom(A) = struct {value: Int};\
             let int_value: Phantom(Int) = {value: 1};\
             let text_value: Phantom(String) = int_value;\
             text_value",
        )
        .unwrap_err();
        assert!(
            error.message.contains("not assignable"),
            "{}",
            error.message
        );
    }

    #[test]
    fn parameterized_type_family_diagnostics_preserve_bounded_failures() {
        let duplicate = analyze_source(
            "duplicate-family.telora",
            "type Pair(A, A) = Tuple([A, A]); 0",
        )
        .unwrap_err();
        assert!(duplicate.message.contains("duplicate type parameter \"A\""));

        let arity = analyze_source(
            "arity-family.telora",
            "type Box(A) = Array(A); type Broken = Box(Int, String); 0",
        )
        .unwrap_err();
        assert!(
            arity.message.contains("expected 1 arguments, got 2"),
            "{}",
            arity.message
        );

        let invalid = analyze_source("invalid-family.telora", "type Broken(A) = 1; 0").unwrap_err();
        assert!(invalid.message.contains("computed metadata cannot become a type"));

        let direct =
            analyze_source("recursive-family.telora", "type Loop(A) = Loop(A); 0").unwrap_err();
        assert!(direct.message.contains("recursive type alias component"));

        let mutual = analyze_source(
            "mutual-family.telora",
            "type Left(A) = Right(A); type Right(A) = Left(A); 0",
        )
        .unwrap_err();
        assert!(mutual.message.contains("recursive type alias component"));

        let mixed = analyze_source(
            "mixed-recursive-family.telora",
            "type Family(A) = Tuple([Concrete, A]);\
             type Concrete = Family(Int);\
             0",
        )
        .unwrap_err();
        assert!(
            mixed.message.contains("recursive type alias component")
                && mixed.message.contains("Family")
                && mixed.message.contains("Concrete"),
            "{}",
            mixed.message
        );
        let diagnostic = mixed.diagnostic.expect("mixed cycle diagnostic");
        assert_eq!(diagnostic.labels.len(), 2);
    }

    #[test]
    fn records_a_type_fact_for_every_resolved_hir_expression() {
        let analysis = analyze_source(
            "facts.telora",
            "let values = [1, 2]; let first = fn(x) { let y = x; y }; first(values)",
        )
        .unwrap();
        assert_eq!(
            analysis.expression_types.len(),
            analysis.hir.expressions().len()
        );
        assert!(
            analysis
                .expression_types
                .values()
                .any(|ty| matches!(analysis.types.node(*ty), TypeNode::Int))
        );
        assert!(
            analysis
                .expression_types
                .values()
                .any(|ty| matches!(analysis.types.node(*ty), TypeNode::Array(_)))
        );
        assert!(
            analysis
                .expression_types
                .values()
                .any(|ty| matches!(analysis.types.node(*ty), TypeNode::Function { .. }))
        );
    }


    #[test]
    fn tool_stage_respects_evaluation_fuel() {
        run_tool_expressions_with_fuel(100_000, 1).unwrap();
        let error = run_tool_expressions_with_fuel(0, 1).unwrap_err();
        assert!(error.message.contains("fuel"));
    }

    #[test]
    fn tool_expressions_share_one_module_account() {
        let spent = 100_000 - run_tool_expressions_with_fuel(100_000, 1).unwrap();
        assert!(spent > 0);
        run_tool_expressions_with_fuel(spent, 1).unwrap();
        let error = run_tool_expressions_with_fuel(spent, 2).unwrap_err();
        assert!(error.message.contains("fuel"));
    }

    fn run_tool_expressions_with_fuel(fuel: usize, count: usize) -> Result<usize, FrontendError> {
        let mut sources = SourceDatabase::default();
        let source = sources.add("tool-fuel", "let value = (fn(x) { x + 1 })(41); 0");
        let program = parse_registered(&sources, source).program.unwrap();
        let expression = &program.value.body.value.bindings[0].value.value;
        let mut main = Heap::main();
        let mut evaluator = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut main);
        let bootstrap = BootstrapPrelude::new();
        let hir = HirProgram::resolve(&program, bootstrap.types.keys().cloned());
        let mut context = ToolInferenceContext::new(
            TypeGraph::default(), &hir, BTreeMap::new(), bootstrap.types, bootstrap.schemes,
            BTreeMap::new(), true,
        );
        let mut account = QuotaAccount::new(Quota::with_fuel(fuel));
        let evidence = solve_tool_expression_types(expression, Some(&TypeDescriptor::Int),
            account.query_context(), &sources, &mut context)
            .and_then(|evidence| prepare_tool_execution(expression, evidence, &sources, &mut context.types))
            .map_err(|message| frontend_error("tool-fuel", message))?;
        evaluator.tool_types = context.types;
        for _ in 0..count {
            evaluate_prepared_tool_expression("tool-fuel", &BTreeMap::new(), &evidence,
                &mut account, &sources, &mut evaluator, true)?;
        }
        Ok(account.remaining_fuel())
    }

    #[test]
    fn static_type_definitions_do_not_consume_execution_fuel() {
        let analysis = analyze_source_with_fuel("test",
            "type Box(T) = struct {value: T}; type First = Box(Int); type Second = Array(First); 0", 0).unwrap();
        assert!(analysis.declared_types.contains_key("Second"));
    }


    #[test]
    fn function_type_errors_precede_construction_check_factory_execution() {
        let source = r#"
            @check(do { fail!("tool-stage-sentinel"); fn(value) { fail!("unused callback") } })
            type Item = struct(Int);
            def later: Fn(Int) -> Int = fn(x) { "bad" };
            0
        "#;
        let error = analyze_source("solve-before-tools", source).unwrap_err();
        assert!(!error.message.contains("tool-stage-sentinel"), "{}", error.message);
        assert!(error.message.contains("Int") && error.message.contains("String"), "{}", error.message);
        let valid = source.replace("fn(x) { \"bad\" }", "fn(x) { 1 }");
        let error = analyze_source("execute-after-solving", &valid).unwrap_err();
        assert!(error.message.contains("tool-stage-sentinel"), "{}", error.message);
    }
