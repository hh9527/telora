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
