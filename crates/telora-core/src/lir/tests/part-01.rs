    #[test]
    fn resolves_labels_and_compresses_origins() {
        let function = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![Constant::Int(1)],
            items: vec![
                Item::Operation(WithOrigin {
                    value: Operation::LoadConst {
                        dst: RegisterId(0),
                        constant: ConstantId(0),
                    },
                    origin: origin(),
                }),
                Item::Operation(WithOrigin {
                    value: Operation::Jump { target: LabelId(0) },
                    origin: origin(),
                }),
                Item::Label(LabelId(0)),
                Item::Operation(WithOrigin {
                    value: Operation::Return { src: RegisterId(0) },
                    origin: origin(),
                }),
            ],
        };
        let bytecode = assemble(function).unwrap();
        assert!(matches!(
            bytecode.instructions()[1],
            crate::bytecode::Opcode::Jump { target: 2 }
        ));
        assert_eq!(bytecode.debug_origins().len(), 1);
        assert_eq!(bytecode.debug_origins()[0].end, 3);
    }

    #[test]
    fn rejects_undefined_labels_and_bad_registers() {
        let bad_label = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![],
            items: vec![Item::Operation(WithOrigin {
                value: Operation::Jump { target: LabelId(4) },
                origin: origin(),
            })],
        };
        assert!(
            assemble(bad_label)
                .unwrap_err()
                .message
                .contains("undefined label")
        );
        let bad_register = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![],
            items: vec![Item::Operation(WithOrigin {
                value: Operation::Return { src: RegisterId(1) },
                origin: origin(),
            })],
        };
        assert!(
            assemble(bad_register)
                .unwrap_err()
                .message
                .contains("out of bounds")
        );

        let bad_arguments = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![],
            items: vec![Item::Operation(WithOrigin {
                value: Operation::Call {
                    base: RegisterId(0),
                    argument_count: 2,
                },
                origin: origin(),
            })],
        };
        assert!(
            assemble(bad_arguments)
                .unwrap_err()
                .message
                .contains("call window")
        );

        let duplicate_label = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![],
            items: vec![Item::Label(LabelId(0)), Item::Label(LabelId(0))],
        };
        assert!(
            assemble(duplicate_label)
                .unwrap_err()
                .message
                .contains("duplicate label")
        );
    }
