    #[test]
    fn links_heap_dependent_operands_out_of_the_code_blob() {
        let child = Arc::new(BytecodeFunction::new(
            "child",
            1,
            vec![Constant::Int(1)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        ));
        let function = BytecodeFunction::new(
            "parent",
            3,
            vec![Constant::String("constant".into())],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::GetField {
                    dst: Register(1),
                    dict: Register(0),
                    field: "name".into(),
                },
                Instruction::MakeClosure {
                    dst: Register(2),
                    function: child,
                    captures: vec![],
                },
                Instruction::Return { src: Register(2) },
            ],
        );

        assert!(matches!(
            function.instructions()[0],
            Opcode::LoadConst {
                value: ValueLinkId(0),
                ..
            }
        ));
        assert!(matches!(
            function.instructions()[1],
            Opcode::GetField {
                field: TextLinkId(0),
                ..
            }
        ));
        assert!(matches!(
            function.instructions()[2],
            Opcode::MakeClosure {
                prototype: ProtoLinkId(0),
                ..
            }
        ));
        assert_eq!(function.text_link(TextLinkId(0)), Some("name"));
        assert_eq!(
            function.prototype_link(ProtoLinkId(0)).unwrap().name(),
            "child"
        );
    }

    #[test]
    fn relinking_shares_heap_independent_code() {
        let function = BytecodeFunction::new(
            "value",
            1,
            vec![Constant::String("linked".into())],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        let relinked = function.relink();
        assert!(function.shares_code_with(&relinked));
        assert_eq!(relinked.constants().len(), 1);
    }
