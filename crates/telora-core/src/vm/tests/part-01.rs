    #[test]
    fn semantic_value_measurement_does_not_report_invalid_graphs_as_quota_failures() {
        let current = Heap::work();
        let background = Heap::main();
        let error = semantic_codec_wrapper_bytes(
            &CodecNode::Tuple(Vec::new(), None),
            &current,
            &background,
        )
        .unwrap_err();
        assert_eq!(error.limit(), None);
        assert!(error.message.contains("unsupported semantic Value"));
    }

    #[test]
    fn executes_arithmetic_and_branching() {
        let result = run(
            &mut Vm::new(),
            4,
            vec![Constant::Int(20), Constant::Int(22), Constant::Int(0)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Add {
                    dst: Register(2),
                    left: Register(0),
                    right: Register(1),
                },
                Instruction::LoadConst {
                    dst: Register(3),
                    constant: 2,
                },
                Instruction::LessThan {
                    dst: Register(3),
                    left: Register(3),
                    right: Register(2),
                },
                Instruction::JumpIfFalse {
                    condition: Register(3),
                    target: 7,
                },
                Instruction::Return { src: Register(2) },
                Instruction::Return { src: Register(0) },
            ],
        )
        .unwrap();
        assert_eq!(result.value().as_int(), Some(42));
    }

    #[test]
    fn canonicalizes_and_interns_dict_shapes() {
        let result = run(
            &mut Vm::new(),
            4,
            vec![Constant::Int(1), Constant::Int(2)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::MakeDict {
                    dst: Register(2),
                    fields: vec![("b".into(), Register(1)), ("a".into(), Register(0))],
                },
                Instruction::MakeDict {
                    dst: Register(3),
                    fields: vec![("a".into(), Register(1)), ("b".into(), Register(0))],
                },
                Instruction::MakeTuple {
                    dst: Register(0),
                    items: vec![Register(2), Register(3)],
                },
                Instruction::Return { src: Register(0) },
            ],
        )
        .unwrap();
        let tuple = result.value();
        let left = tuple.sequence_get(0).expect("left Dict");
        let right = tuple.sequence_get(1).expect("right Dict");
        assert_eq!(left.dict_fields(), Some(vec!["a", "b"]));
        let (DecodedValue::Dict(left_handle), DecodedValue::Dict(right_handle)) =
            (left.value.value(), right.value.value())
        else {
            panic!("expected Dict values");
        };
        let Object::Dict {
            shape: left_shape, ..
        } = left.view.object(left_handle).unwrap()
        else {
            unreachable!()
        };
        let Object::Dict {
            shape: right_shape, ..
        } = right.view.object(right_handle).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(left_shape, right_shape);
        assert_eq!(left.dict_get("a").unwrap().as_int(), Some(1));
    }

    #[test]
    fn constructs_and_reads_structured_values() {
        let result = run(
            &mut Vm::new(),
            5,
            vec![
                Constant::Atom(Atom::builtin(BuiltinAtom::Ok)),
                Constant::Int(42),
            ],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::MakeTuple {
                    dst: Register(2),
                    items: vec![Register(0), Register(1)],
                },
                Instruction::MakeArray {
                    dst: Register(3),
                    items: vec![Register(1), Register(2)],
                },
                Instruction::MakeDict {
                    dst: Register(4),
                    fields: vec![("result".into(), Register(3))],
                },
                Instruction::GetField {
                    dst: Register(0),
                    dict: Register(4),
                    field: "result".into(),
                },
                Instruction::Return { src: Register(0) },
            ],
        )
        .unwrap();
        assert_eq!(result.to_string(), "[42, ('Ok, 42)]");
    }

    #[test]
    fn reports_integer_errors_consistently() {
        let overflow = run(
            &mut Vm::new(),
            3,
            vec![Constant::Int(i64::MAX), Constant::Int(1)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Add {
                    dst: Register(2),
                    left: Register(0),
                    right: Register(1),
                },
                Instruction::Return { src: Register(2) },
            ],
        )
        .unwrap_err();
        assert_eq!(overflow.kind, RuntimeErrorKind::IntegerOverflow);

        let division = run(
            &mut Vm::new(),
            3,
            vec![Constant::Int(1), Constant::Int(0)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Divide {
                    dst: Register(2),
                    left: Register(0),
                    right: Register(1),
                },
                Instruction::Return { src: Register(2) },
            ],
        )
        .unwrap_err();
        assert_eq!(division.kind, RuntimeErrorKind::DivisionByZero);
    }

    #[test]
    fn runtime_error_recoverability_is_typed_and_exhaustive() {
        use crate::evaluation::FailureClass;

        let function = BytecodeFunction::new("classification", 0, vec![], vec![]);
        for kind in [
            RuntimeErrorKind::DivisionByZero,
            RuntimeErrorKind::IntegerOverflow,
            RuntimeErrorKind::MissingField,
            RuntimeErrorKind::NoPatternMatched,
            RuntimeErrorKind::TypeMismatch,
            RuntimeErrorKind::UninitializedDefinition,
            RuntimeErrorKind::DuplicateDefinition,
        ] {
            assert_eq!(
                error(kind, "recoverable", &function, 0).failure_class(),
                FailureClass::Recoverable
            );
        }
        for kind in [
            RuntimeErrorKind::Cancelled,
            RuntimeErrorKind::FuelExhausted,
            RuntimeErrorKind::AllocationQuotaExceeded,
            RuntimeErrorKind::CallDepthExceeded,
            RuntimeErrorKind::InvalidBytecode,
            RuntimeErrorKind::StackLimitExceeded,
        ] {
            assert_eq!(
                error(kind, "terminal", &function, 0).failure_class(),
                FailureClass::Terminal
            );
        }
    }

    #[test]
    fn rejects_non_boolean_conditions() {
        let error = run(
            &mut Vm::new(),
            1,
            vec![Constant::Int(1)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::JumpIfFalse {
                    condition: Register(0),
                    target: 2,
                },
                Instruction::Return { src: Register(0) },
            ],
        )
        .unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
    }

    #[test]
    fn enforces_fuel_and_rejects_malformed_bytecode() {
        let loop_function =
            BytecodeFunction::new("loop", 0, vec![], vec![Instruction::Jump { target: 0 }]);
        let error = Vm::new().execute(&loop_function, 5).unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::FuelExhausted);

        let invalid = BytecodeFunction::new(
            "invalid",
            0,
            vec![],
            vec![Instruction::Return { src: Register(9) }],
        );
        let error = Vm::new().execute(&invalid, 5).unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::InvalidBytecode);

        let invalid_call_window = BytecodeFunction::new(
            "invalid-call-window",
            1,
            vec![Constant::Native(NativeFunction::new(
                "identity",
                1,
                native_identity,
            ))],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::TailCall {
                    base: Register(0),
                    argument_count: 1,
                },
            ],
        );
        let error = Vm::new().execute(&invalid_call_window, 5).unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::InvalidBytecode);
    }

    #[test]
    fn straight_line_and_forward_control_flow_need_no_fuel() {
        let straight = BytecodeFunction::new(
            "straight",
            1,
            vec![Constant::Int(42)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&straight, 0).unwrap().value().as_int(),
            Some(42)
        );

        let forward = BytecodeFunction::new(
            "forward",
            1,
            vec![Constant::Int(42)],
            vec![
                Instruction::Jump { target: 2 },
                Instruction::Fail {
                    message: "skipped".into(),
                },
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&forward, 0).unwrap().value().as_int(),
            Some(42)
        );
    }

    #[test]
    fn only_taken_back_edges_consume_fuel() {
        let untaken = BytecodeFunction::new(
            "untaken",
            1,
            vec![Constant::Atom(Atom::builtin(BuiltinAtom::True))],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::JumpIfFalse {
                    condition: Register(0),
                    target: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert!(Vm::new().execute(&untaken, 0).is_ok());

        let one_back_edge = BytecodeFunction::new(
            "one-back-edge",
            1,
            vec![
                Constant::Atom(Atom::builtin(BuiltinAtom::False)),
                Constant::Atom(Atom::builtin(BuiltinAtom::True)),
            ],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Jump { target: 3 },
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 1,
                },
                Instruction::JumpIfFalse {
                    condition: Register(0),
                    target: 2,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        let exhausted = Vm::new().execute(&one_back_edge, 0).unwrap_err();
        assert_eq!(exhausted.kind, RuntimeErrorKind::FuelExhausted);
        assert!(Vm::new().execute(&one_back_edge, 1).is_ok());
    }

    #[test]
    fn bytecode_and_native_calls_each_consume_one_fuel() {
        let callee = Arc::new(BytecodeFunction::new(
            "callee",
            1,
            vec![Constant::Int(42)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        ));
        let bytecode = BytecodeFunction::new(
            "bytecode-call",
            2,
            vec![],
            vec![
                Instruction::MakeClosure {
                    dst: Register(0),
                    function: callee,
                    captures: vec![],
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&bytecode, 0).unwrap_err().kind,
            RuntimeErrorKind::FuelExhausted
        );
        assert!(Vm::new().execute(&bytecode, 1).is_ok());

        let nested = BytecodeFunction::new(
            "nested-call",
            2,
            vec![],
            vec![
                Instruction::MakeClosure {
                    dst: Register(0),
                    function: Arc::new(bytecode),
                    captures: vec![],
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&nested, 1).unwrap_err().kind,
            RuntimeErrorKind::FuelExhausted
        );
        assert!(Vm::new().execute(&nested, 2).is_ok());

        let native = NativeFunction::new("identity", 1, native_identity);
        let native = BytecodeFunction::new(
            "native-call",
            3,
            vec![Constant::Native(native), Constant::Int(2)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 1,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&native, 0).unwrap_err().kind,
            RuntimeErrorKind::FuelExhausted
        );
        assert!(Vm::new().execute(&native, 1).is_ok());
    }

    fn native_identity(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
        let value = context
            .value(context.argument(0)?)?
            .as_int()
            .ok_or_else(|| NativeError::new("expected Int argument"))?;
        context.set_int(context.result(), value)
    }

    fn native_non_finite_float(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
        context.set_float(context.result(), f64::INFINITY)
    }

    #[test]
    fn native_non_finite_float_result_raises_blame_at_the_call() {
        let native = NativeFunction::new("non_finite_float", 0, native_non_finite_float);
        let function = BytecodeFunction::new(
            "native-float-call",
            1,
            vec![Constant::Native(native)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        let error = Vm::new().execute(&function, 1).unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "NonFiniteFloat");
    }

    #[test]
    fn tail_calls_native_functions_and_replace_bytecode_frames() {
        let native = NativeFunction::new("identity", 1, native_identity);
        let native_tail = BytecodeFunction::new(
            "native-tail",
            2,
            vec![Constant::Native(native), Constant::Int(42)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::TailCall {
                    base: Register(0),
                    argument_count: 1,
                },
            ],
        );
        assert_eq!(
            Vm::new().execute(&native_tail, 0).unwrap_err().kind,
            RuntimeErrorKind::FuelExhausted
        );
        assert_eq!(
            Vm::new().execute(&native_tail, 1).unwrap().value().as_int(),
            Some(42)
        );

        let large = Arc::new(BytecodeFunction::new(
            "large-frame",
            100,
            vec![Constant::Int(7)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        ));
        let replace = BytecodeFunction::new(
            "small-frame",
            1,
            vec![],
            vec![
                Instruction::MakeClosure {
                    dst: Register(0),
                    function: large,
                    captures: vec![],
                },
                Instruction::TailCall {
                    base: Register(0),
                    argument_count: 0,
                },
            ],
        );
        assert_eq!(
            Vm::new()
                .execute_with_quota(&replace, Quota::new(1, 100, u64::MAX))
                .unwrap()
                .value()
                .as_int(),
            Some(7)
        );
        assert_eq!(
            Vm::new()
                .execute_with_quota(&replace, Quota::new(1, 99, u64::MAX))
                .unwrap_err()
                .kind,
            RuntimeErrorKind::StackLimitExceeded
        );
    }

    #[test]
    fn native_closures_use_register_context() {
        let native = NativeFunction::new("identity", 1, native_identity);
        let function = BytecodeFunction::new(
            "test",
            3,
            vec![Constant::Native(native), Constant::Int(2)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 1,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&function, 20).unwrap().value().as_int(),
            Some(2)
        );
    }

    #[test]
    fn nested_calls_use_explicit_vm_frames() {
        let mut function = Arc::new(BytecodeFunction::new(
            "leaf",
            1,
            vec![Constant::Int(7)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        ));
        for depth in 0..512 {
            function = Arc::new(BytecodeFunction::new(
                format!("frame{depth}"),
                2,
                vec![],
                vec![
                    Instruction::MakeClosure {
                        dst: Register(0),
                        function,
                        captures: vec![],
                    },
                    Instruction::Call {
                        base: Register(0),
                        argument_count: 0,
                    },
                    Instruction::Return { src: Register(0) },
                ],
            ));
        }
        assert_eq!(
            Vm::new()
                .execute(&function, 2_000)
                .unwrap()
                .value()
                .as_int(),
            Some(7)
        );
    }

    #[test]
    fn enforces_independent_call_depth_and_stack_slot_limits() {
        let mut function = Arc::new(BytecodeFunction::new(
            "leaf",
            1,
            vec![Constant::Int(7)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        ));
        for _ in 0..MAX_CALL_DEPTH {
            function = Arc::new(BytecodeFunction::new(
                "recursive-shape",
                2,
                vec![],
                vec![
                    Instruction::MakeClosure {
                        dst: Register(0),
                        function,
                        captures: vec![],
                    },
                    Instruction::Call {
                        base: Register(0),
                        argument_count: 0,
                    },
                    Instruction::Return { src: Register(0) },
                ],
            ));
        }
        let depth = Vm::new().execute(&function, usize::MAX).unwrap_err();
        assert_eq!(depth.kind, RuntimeErrorKind::CallDepthExceeded);

        let oversized = BytecodeFunction::new(
            "oversized",
            MAX_STACK_SLOTS + 1,
            vec![],
            vec![Instruction::Return { src: Register(0) }],
        );
        let stack = Vm::new().execute(&oversized, usize::MAX).unwrap_err();
        assert_eq!(stack.kind, RuntimeErrorKind::StackLimitExceeded);
    }

    #[test]
    fn trace_does_not_deduplicate_equal_function_names_and_pcs() {
        let leaf = Arc::new(BytecodeFunction::new(
            "same",
            3,
            vec![Constant::Int(1), Constant::Int(0)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Divide {
                    dst: Register(2),
                    left: Register(0),
                    right: Register(1),
                },
                Instruction::Return { src: Register(2) },
            ],
        ));
        let mut function = leaf;
        for _ in 0..2 {
            function = Arc::new(BytecodeFunction::new(
                "same",
                2,
                vec![],
                vec![
                    Instruction::MakeClosure {
                        dst: Register(0),
                        function,
                        captures: vec![],
                    },
                    Instruction::Call {
                        base: Register(0),
                        argument_count: 0,
                    },
                    Instruction::Return { src: Register(0) },
                ],
            ));
        }
        let error = Vm::new().execute(&function, 100).unwrap_err();
        assert_eq!(error.trace.len(), 3);
        assert!(error.trace.iter().all(|frame| frame.function == "same"));
    }

    #[test]
    fn dict_allocation_charge_does_not_depend_on_shape_cache_hits() {
        let function = crate::compile_source("test", "{answer: 42}")
            .unwrap()
            .into_function();
        let mut vm = Vm::new();
        let mut account = QuotaAccount::new(Quota::new(0, 100, u64::MAX));
        vm.execute_with_account(&function, &[], &mut account)
            .unwrap();
        let first = account.requested_allocation_bytes();
        vm.execute_with_account(&function, &[], &mut account)
            .unwrap();
        let second = account.requested_allocation_bytes() - first;
        assert_eq!(first, second);
        assert!(first > 0);
    }

    #[test]
    fn debug_formatter_is_cycle_safe_and_bounded() {
        let background = Heap::main();
        let mut current = Heap::work();
        let cycle = current.reserve();
        current
            .initialize(
                cycle,
                Object::Array(vec![Val::unknown(DecodedValue::Array(cycle))].into()),
            )
            .unwrap();
        let cycle_text = DebugValueFormatter::new(HeapView {
            current: &current,
            background: Some(&background),
        })
        .format(DecodedValue::Array(cycle).into())
        .unwrap();
        assert_eq!(cycle_text, "[<cycle>]");

        let long = current.string(None, &"x".repeat(DEBUG_MAX_BYTES * 2));
        let long_text = DebugValueFormatter::new(HeapView {
            current: &current,
            background: Some(&background),
        })
        .format(long.into())
        .unwrap();
        assert_eq!(long_text.len(), DEBUG_MAX_BYTES);
        assert!(long_text.ends_with("..."));

        let bytes = DecodedValue::Bytes(current.allocate(Object::Bytes(
            (0..64).map(|value| value as u8).collect::<Vec<_>>().into(),
        )));
        let bytes_text = DebugValueFormatter::new(HeapView {
            current: &current,
            background: Some(&background),
        })
        .format(bytes.into())
        .unwrap();
        assert!(bytes_text.starts_with("b\"\\x00\\x01"));
        assert!(bytes_text.contains("..."));
    }

    #[test]
    fn json_writer_rejects_internal_cycles() {
        let background = Heap::main();
        let mut current = Heap::work();
        let cycle = current.reserve();
        current
            .initialize(
                cycle,
                Object::Array(vec![Val::unknown(DecodedValue::Array(cycle))].into()),
            )
            .unwrap();
        let mut writer = JsonWriter::new(
            HeapView {
                current: &current,
                background: Some(&background),
            },
            None,
        );
        assert_eq!(
            writer
                .value(DecodedValue::Array(cycle).into(), 0)
                .unwrap_err(),
            "JSON cannot encode cyclic values"
        );
    }

    #[test]
    fn reducer_transition_audits_the_complete_effect_batch() {
        let background = Heap::main();
        let mut heap = Heap::work();
        let failed_payload = Val::unknown(DecodedValue::Array(heap.allocate(Object::Array(
            vec![Val::unknown(DecodedValue::Failed(0))].into(),
        ))));
        let effects = Val::unknown(DecodedValue::Array(heap.allocate(Object::Array(
            vec![Val::unknown(DecodedValue::Int(1)), failed_payload].into(),
        ))));
        let root = Val::unknown(DecodedValue::Tuple(heap.allocate(Object::Tuple(
            vec![Val::unknown(DecodedValue::Int(0)), effects].into(),
        ))));
        let error = match (WorkWorld { heap, root }).into_reducer_transition(&background) {
            Ok(_) => panic!("failed effect batch crossed the Host boundary"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "failed evaluation node cannot cross the SystemEffect boundary"
        );
    }
