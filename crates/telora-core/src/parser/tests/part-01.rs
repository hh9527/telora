    #[test]
    fn accepts_hash_comments_and_shebangs() {
        let program = parse(
            "script.telora",
            "#!/usr/bin/env -S telora run\nlet value = 42; # answer\nvalue",
        )
        .unwrap();
        assert_eq!(program.value.body.value.bindings.len(), 1);
    }

    #[test]
    fn lowers_directly_from_cst_with_spans_and_precedence() {
        let mut sources = SourceDatabase::default();
        let id = sources.add("test.telora", "let x = 1; x == 2");
        let parsed = parse_registered(&sources, id);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let program = parsed.program.unwrap();
        assert_eq!(program.location.range(), 0..17);
        assert_eq!(program.value.body.value.bindings[0].location.range(), 0..10);
        assert!(matches!(
            &program.value.body.value.result.value,
            ExprKind::Binary {
                operator: Located {
                    value: BinaryOperator::Equal,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn lowers_tagged_patterns() {
        let program = parse("test.telora", "match 'Some(1) { 'Some(value) => value }").unwrap();
        let ExprKind::Match { arms, .. } = &program.value.body.value.result.value else {
            panic!("expected match");
        };
        assert!(
            matches!(
                &arms[0].value.pattern.value,
                PatternKind::Tagged { payload, .. }
                    if matches!(payload.value, PatternKind::Binding(_))
            ),
            "{:?}",
            arms[0].value.pattern.value
        );
    }

    #[test]
    fn lowers_control_flow_else_chains_into_else_blocks() {
        let cases = [
            ("if 'False { 1 } else if 'True { 2 } else { 3 }", "if"),
            (
                "if 'False { 1 } else if let 'Some(value) = 'Some(2) { value } else { 3 }",
                "if let",
            ),
            (
                "if 'False { 1 } else match 'Some(2) { 'Some(value) => value, 'None => 3 }",
                "match",
            ),
            ("if 'False { 1 } else return 2;", "return"),
        ];
        for (source, expected) in cases {
            let program = parse("test.telora", source).unwrap();
            let ExprKind::If { else_branch, .. } = &program.value.body.value.result.value else {
                panic!("expected outer if");
            };
            assert!(else_branch.value.bindings.is_empty());
            assert!(match expected {
                "if" => matches!(else_branch.value.result.value, ExprKind::If { .. }),
                "if let" => matches!(else_branch.value.result.value, ExprKind::IfLet { .. }),
                "match" => matches!(else_branch.value.result.value, ExprKind::Match { .. }),
                "return" => matches!(else_branch.value.result.value, ExprKind::Return { .. }),
                _ => unreachable!(),
            });
        }

        let program = parse(
            "test.telora",
            "if let 'Some(value) = 'None { value } else match 'Some(2) { 'Some(item) => item, 'None => 3 }",
        )
        .unwrap();
        let ExprKind::IfLet { else_branch, .. } = &program.value.body.value.result.value else {
            panic!("expected outer if let");
        };
        assert!(matches!(
            else_branch.value.result.value,
            ExprKind::Match { .. }
        ));

        let program = parse(
            "test.telora",
            "if let 'Some(value) = 'None { value } else return 2;",
        )
        .unwrap();
        let ExprKind::IfLet { else_branch, .. } = &program.value.body.value.result.value else {
            panic!("expected outer if let");
        };
        assert!(matches!(
            else_branch.value.result.value,
            ExprKind::Return { .. }
        ));
    }

    #[test]
    fn malformed_else_if_chain_recovers_without_panicking() {
        let mut sources = SourceDatabase::default();
        let id = sources.add("test.telora", "if 'True { 1 } else if { 2 } else { 3 }");
        let parsed = parse_registered(&sources, id);
        assert!(!parsed.diagnostics.is_empty());
        assert!(parsed.program.is_none());
    }

    #[test]
    fn lowers_shorthand_and_nested_struct_patterns() {
        let program = parse(
            "test.telora",
            "match user { { name, address: { city }, } => (name, city) }",
        )
        .unwrap();
        let ExprKind::Match { arms, .. } = &program.value.body.value.result.value else {
            panic!("expected match");
        };
        let PatternKind::Struct(fields) = &arms[0].value.pattern.value else {
            panic!("expected Struct pattern");
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name.value, "name");
        assert!(matches!(fields[0].pattern.value, PatternKind::Binding(_)));
        assert!(matches!(fields[1].pattern.value, PatternKind::Struct(_)));
    }

    #[test]
    fn elaborates_local_destructuring_let_into_an_irrefutable_match() {
        let program = parse(
            "test.telora",
            "{ let (left, {name}) = (1, {name: \"Ada\"}); (left, name) }",
        )
        .unwrap();
        let ExprKind::Block(block) = &program.value.body.value.result.value else {
            panic!("expected source block");
        };
        let ExprKind::Match { arms, .. } = &block.value.result.value else {
            panic!("expected elaborated match");
        };
        assert_eq!(arms.len(), 1);
        assert!(arms[0].value.irrefutable_required);
        assert!(matches!(arms[0].value.pattern.value, PatternKind::Tuple(_)));
    }

    #[test]
    fn rejects_module_level_destructuring_let() {
        let error = parse("test.telora", "let (left, right) = (1, 2); left").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("destructuring let is allowed only inside a local block")
        );
    }

    #[test]
    fn lowers_explicit_exports_without_creating_lexical_bindings() {
        let program = parse(
            "exports.telora",
            r#"export def value = 1;
export def identity = fn(item) { item };
export type User = struct { name: String };
def private = 2;
export { private as visible, identity as map };"#,
        )
        .unwrap();
        assert!(!program.value.authored_result);
        let bindings = &program.value.body.value.bindings;
        assert_eq!(
            bindings
                .iter()
                .filter(|binding| binding.value.kind == BindingKind::Export)
                .count(),
            5
        );
        assert_eq!(
            bindings
                .iter()
                .filter(|binding| binding.value.kind == BindingKind::Def)
                .count(),
            3
        );
        let visible = bindings
            .iter()
            .find(|binding| {
                binding.value.kind == BindingKind::Export && binding.value.name.value == "visible"
            })
            .unwrap();
        assert_eq!(
            visible.value.imported_name.as_deref().unwrap().value,
            "private"
        );
    }

    #[test]
    fn diagnoses_duplicate_mixed_and_nested_exports() {
        for (source, expected) in [
            (
                "export def value = 1; export { value };",
                "duplicate export",
            ),
            (
                "export def value = 1; value",
                "top-level expressions are not supported",
            ),
            (
                "let value = { export def nested = 1; nested }; value",
                "only at module top level",
            ),
        ] {
            let error = parse("exports.telora", source).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn lowers_heterogeneous_tuple_contracts_through_array_metadata() {
        let program = parse(
            "test.telora",
            "native pairs: Fn(Any) -> Array(Tuple([String, Any])); 0",
        )
        .unwrap();
        let annotation = program.value.body.value.bindings[0]
            .value
            .annotation
            .as_ref()
            .expect("native annotation");
        let ExprKind::Call { arguments, .. } = &annotation.value else {
            panic!("expected Fn metadata call");
        };
        let ExprKind::Call { arguments, .. } = &arguments[1].value else {
            panic!("expected Array metadata call");
        };
        let ExprKind::Call { arguments, .. } = &arguments[0].value else {
            panic!("expected Tuple metadata call");
        };
        assert!(matches!(&arguments[0].value, ExprKind::Array(items) if items.len() == 2));
    }

    #[test]
    fn function_notation_lowers_to_the_func_metadata_constructor() {
        let program = parse("test.telora", "native convert: Fn(A) -> Tuple([B, C]); 0").unwrap();
        let annotation = program.value.body.value.bindings[0]
            .value
            .annotation
            .as_ref()
            .expect("native annotation");
        let ExprKind::Call { callee, arguments } = &annotation.value else {
            panic!("expected Func metadata call");
        };
        assert!(is_variable(callee, "Func"));
        assert!(matches!(&arguments[0].value, ExprKind::Array(items) if items.len() == 1));
        let ExprKind::Call { callee, arguments } = &arguments[1].value else {
            panic!("expected Tuple metadata call");
        };
        assert!(is_variable(callee, "Tuple"));
        assert!(matches!(&arguments[0].value, ExprKind::Array(items) if items.len() == 2));
    }

    #[test]
    fn function_contracts_accept_qualified_type_paths() {
        let program = parse(
            "types.telora",
            "native consume: Fn(types.Input, Array(types.Item)) -> types.Output; 0",
        )
        .unwrap();
        let annotation = program.value.body.value.bindings[0]
            .value
            .annotation
            .as_ref()
            .expect("native annotation");
        let ExprKind::Call { arguments, .. } = &annotation.value else {
            panic!("expected Func metadata call");
        };
        assert!(matches!(arguments[0].value, ExprKind::Array(ref items) if items.len() == 2));
        assert!(matches!(arguments[1].value, ExprKind::Field { .. }));
    }

    #[test]
    fn rejects_constructor_shaped_fn_notation() {
        let error = parse("test.telora", "native invalid: Fn([A], B); 0").unwrap_err();
        assert!(error.to_string().contains("expected"), "{error}");
    }

    #[test]
    fn diagnoses_invalid_placeholder_sections_with_source_locations() {
        let cases = [
            (
                "mixed.telora",
                "let f = fn(a, b) { a }; f\\(_0, _)",
                "cannot mix",
            ),
            (
                "gap.telora",
                "let f = fn(a, b) { a }; f\\(_2, _0)",
                "missing _1",
            ),
            (
                "limit.telora",
                "let f = fn(a) { a }; f\\(_65535)",
                "exceeds the limit",
            ),
            (
                "overflow.telora",
                "let f = fn(a) { a }; f\\(_999999999999999999999999999999999)",
                "exceeds the supported range",
            ),
        ];
        for (name, source, expected) in cases {
            let mut sources = SourceDatabase::default();
            let id = sources.add(name, source);
            let parsed = parse_registered(&sources, id);
            assert!(parsed.program.is_none(), "{name} unexpectedly lowered");
            let rendered = parsed
                .diagnostics
                .iter()
                .map(|diagnostic| sources.render(diagnostic))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(rendered.contains(expected), "{rendered}");
            assert!(rendered.contains(&format!("{name}:1:")), "{rendered}");
        }

        let mut sources = SourceDatabase::default();
        let id = sources.add("outside.telora", "let value = _; value");
        let parsed = parse_registered(&sources, id);
        assert!(parsed.program.is_none());
        assert!(!parsed.diagnostics.is_empty());

        for (name, source) in [
            ("ordinary-call.telora", "let f = fn(a, b) { a }; f(_, 1)"),
            ("reserved-name.telora", "let _0 = 1; _0"),
        ] {
            let mut sources = SourceDatabase::default();
            let id = sources.add(name, source);
            let parsed = parse_registered(&sources, id);
            assert!(parsed.program.is_none(), "{name} unexpectedly lowered");
            assert!(!parsed.diagnostics.is_empty(), "{name} has no diagnostic");
        }

        let mut sources = SourceDatabase::default();
        let id = sources.add("empty-section.telora", "let f = fn(a) { a }; f\\(1)");
        let parsed = parse_registered(&sources, id);
        assert!(parsed.program.is_none());
        assert!(
            parsed.diagnostics[0]
                .message
                .contains("requires at least one placeholder")
        );
    }

    #[test]
    fn lowers_only_direct_type_argument_placeholders() {
        let program = parse("types.telora", "pair@[Int, _](1, \"x\")").unwrap();
        let ExprKind::Call { callee, .. } = &program.value.body.value.result.value else {
            panic!("expected call");
        };
        let ExprKind::TypeApply { arguments, .. } = &callee.value else {
            panic!("expected type application");
        };
        assert!(matches!(arguments[0].value, TypeArgumentKind::Explicit(_)));
        assert!(matches!(arguments[1].value, TypeArgumentKind::Infer));
        assert_eq!(arguments[1].location.range(), 11..12);

        for source in ["pair@[Int, _0](1, 2)", "pair@[Array(_), Int](1, 2)"] {
            let mut sources = SourceDatabase::default();
            let id = sources.add("invalid.telora", source);
            let parsed = parse_registered(&sources, id);
            assert!(parsed.program.is_none(), "{source} unexpectedly parsed");
            assert!(!parsed.diagnostics.is_empty(), "{source} has no diagnostic");
        }
    }

    #[test]
    fn distinguishes_postfix_application_and_projection_forms() {
        let program = parse(
            "postfix.telora",
            "(generic@[Int], values[index], pair.0, pair.1.0, pair.1.0.2, \
             record.pair.1.0, values[0].1.0, make().1.0, \
             pair.1.0.field, pair.1.0[0], pair.1.0(), 1.0)",
        )
        .unwrap();
        let ExprKind::Tuple(items) = &program.value.body.value.result.value else {
            panic!("expected tuple");
        };
        assert!(matches!(items[0].value, ExprKind::TypeApply { .. }));
        assert!(matches!(items[1].value, ExprKind::Index { .. }));
        assert!(matches!(
            items[2].value,
            ExprKind::TupleProjection { ref index, .. } if index.value == 0
        ));
        let ExprKind::TupleProjection {
            receiver,
            index: outer_index,
        } = &items[3].value
        else {
            panic!("expected outer tuple projection");
        };
        assert_eq!(outer_index.value, 0);
        assert!(matches!(
            receiver.value,
            ExprKind::TupleProjection { ref index, .. } if index.value == 1
        ));
        assert!(matches!(items[11].value, ExprKind::Float(value) if value == 1.0));
    }

    #[test]
    fn rejects_non_finite_float_literals_and_patterns() {
        for source in [
            "1e9999".to_owned(),
            "match 1.0 { 1e9999 => 0, _ => 1 }".to_owned(),
        ] {
            let mut sources = SourceDatabase::default();
            let id = sources.add("float.telora", &source);
            let parsed = parse_registered(&sources, id);
            assert!(parsed.program.is_none(), "accepted {source}");
            assert!(
                parsed
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("Float literal must be finite")),
                "{source}: {:?}",
                parsed.diagnostics
            );
        }
    }

    #[test]
    fn parses_finite_float_exponent_notation() {
        let program = parse("float.telora", "(1e3, 1.25e-3, 1.0E+8)").unwrap();
        let ExprKind::Tuple(values) = &program.value.body.value.result.value else {
            panic!("expected tuple")
        };
        assert!(matches!(values[0].value, ExprKind::Float(value) if value == 1000.0));
        assert!(matches!(values[1].value, ExprKind::Float(value) if value == 0.00125));
        assert!(matches!(values[2].value, ExprKind::Float(value) if value == 100_000_000.0));
    }

    #[test]
    fn preserves_independent_recovery_diagnostics() {
        let mut sources = SourceDatabase::default();
        let id = sources.add("broken.telora", "let x = ; let y = ; y");
        let parsed = parse_registered(&sources, id);
        assert!(parsed.program.is_none());
        assert!(parsed.diagnostics.len() >= 2);
    }

    #[test]
    fn commits_to_one_actionable_diagnostic_per_syntax_root() {
        let cases: &[(&str, &[&str])] = &[
            (
                "export def broken = (1 + 2;",
                &["invalid syntax, expected one of: ',', ')'"],
            ),
            (
                "export def broken = match 'A { 'A 1, _ => 2 };",
                &["missing FatArrow"],
            ),
            (
                "type Broken = enum { @json.rename(\"bad\") }; export {Broken};",
                &["missing Atom"],
            ),
            (
                "type Broken = enum { @json.rename(\"bad\", 'Bad }; export {Broken};",
                &["invalid syntax, expected one of: ',', ')'"],
            ),
            (
                "export def broken = match 'A { @ => 1, _ => 2 };",
                &[
                    "invalid syntax, expected one of: <atom>, '\"', <float>, <identifier>, <integer>, '{', '(', '_', <raw string>",
                ],
            ),
        ];

        for (source, expected) in cases {
            let mut sources = SourceDatabase::default();
            let id = sources.add("broken.telora", *source);
            let parsed = parse_registered(&sources, id);
            let messages = parsed
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>();
            assert_eq!(messages, *expected, "{source}");
        }
    }

    #[test]
    fn keeps_separate_syntax_roots_independently_actionable() {
        let cases: &[(&str, &[&str])] = &[
            (
                "export def first = (1 + 2; export def second = match 'A { 'A 1, _ => 2 };",
                &[
                    "invalid syntax, expected one of: ',', ')'",
                    "missing FatArrow",
                ],
            ),
            (
                "export def broken = match 'A { 'A 1, 'B 2, _ => 3 };",
                &[
                    "missing FatArrow",
                    "invalid syntax, expected one of: '=>', 'if'",
                ],
            ),
        ];

        for (source, expected) in cases {
            let mut sources = SourceDatabase::default();
            let id = sources.add("broken.telora", *source);
            let parsed = parse_registered(&sources, id);
            let messages = parsed
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>();
            assert_eq!(messages, *expected, "{source}");
        }
    }

    #[test]
    fn recovers_complete_bindings_around_a_damaged_sibling() {
        let mut sources = SourceDatabase::default();
        let id = sources.add(
            "recover.telora",
            "let before = 1; let broken = ; let after = 2; after",
        );
        let parsed = parse_registered(&sources, id);
        assert!(parsed.program.is_none());
        assert!(!parsed.diagnostics.is_empty());
        let names = parsed
            .recovered
            .bindings
            .iter()
            .map(|binding| binding.value.name.value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["before", "after"]);
        assert!(parsed.recovered.result.is_some());
    }

    #[test]
    fn comparisons_share_a_non_associative_precedence_level() {
        for source in ["1 < 2 == 3", "1 < 2 <= 3", "1 == 2 != 3", "1 >= 2 > 3"] {
            let chained = parse("test", source).unwrap_err();
            assert!(chained.message.contains("do not associate"), "{source}");
        }
        assert!(parse("test", "(1 < 2) == 3").is_ok());
        assert!(parse("test", "1 < (2 == 3)").is_ok());
    }

    #[test]
    fn lowers_prefix_logical_negation_with_unary_precedence() {
        let program = parse("test", "!!'True == !'False").unwrap();
        let ExprKind::Binary { left, right, .. } = &program.value.body.value.result.value else {
            panic!("expected equality expression");
        };
        assert!(matches!(
            &left.value,
            ExprKind::Unary {
                operator: Located {
                    value: UnaryOperator::Not,
                    ..
                },
                operand,
            } if matches!(operand.value, ExprKind::Unary { .. })
        ));
        assert!(matches!(
            &right.value,
            ExprKind::Unary {
                operator: Located {
                    value: UnaryOperator::Not,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn missing_binary_right_operands_are_diagnosed_without_panicking() {
        for operator in [
            "+", "-", "*", "/", "%", "&", "|", "^", "<", "<=", ">", ">=", "==", "!=", "&&", "||",
        ] {
            let mut sources = SourceDatabase::default();
            let source_id = sources.add("missing.telora", format!("1 {operator} ;"));
            let parsed = parse_registered(&sources, source_id);
            assert!(parsed.program.is_none(), "{operator} unexpectedly parsed");
            assert!(
                !parsed.diagnostics.is_empty(),
                "{operator} has no diagnostic"
            );
        }
    }

    #[test]
    fn lowers_interpolation_with_located_text_and_expression_parts() {
        let program = parse("test", r#"let name = "Ada"; `hi, \{name}`"#).unwrap();
        let ExprKind::InterpolatedString(parts) = &program.value.body.value.result.value else {
            panic!("expected interpolated string");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0].value, StringPartKind::Text(text) if text == "hi, "));
        assert!(matches!(
            &parts[1].value,
            StringPartKind::Expression(expression)
                if matches!(&expression.value, ExprKind::Variable(name) if name.value == "name")
        ));
        assert_eq!(parts[1].location.range(), 25..29);
    }

    #[test]
    fn lowers_definition_bindings_and_function_contracts() {
        let program = parse(
            "defs.telora",
            "decl f: Fn(Int) -> Int; def f = fn(x) { x }; f",
        )
        .unwrap();
        assert_eq!(program.value.body.value.bindings.len(), 2);
        assert_eq!(
            program.value.body.value.bindings[0].value.kind,
            BindingKind::Decl
        );
        assert_eq!(
            program.value.body.value.bindings[1].value.kind,
            BindingKind::Def
        );
        assert!(matches!(
            program.value.body.value.bindings[0]
                .value
                .annotation
                .as_ref()
                .map(|annotation| &annotation.value),
            Some(ExprKind::Call { .. })
        ));
    }

    #[test]
    fn lowers_generic_definition_declarations_with_located_parameters() {
        let program = parse(
            "identity.telora",
            "decl identity: for(A) Fn(A) -> A; def identity = fn(value) { value }; identity",
        )
        .unwrap();
        let declaration = &program.value.body.value.bindings[0];
        assert_eq!(declaration.value.kind, BindingKind::Decl);
        assert_eq!(declaration.value.type_parameters.len(), 1);
        assert_eq!(declaration.value.type_parameters[0].value, "A");
        assert_eq!(
            declaration.value.type_parameters[0].location.range(),
            19..20
        );
        assert!(declaration.value.annotation.is_some());
    }

    #[test]
    fn lowers_located_native_bindings_with_contracts() {
        let program = parse(
            "native.telora",
            "native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B); map",
        )
        .unwrap();
        let binding = &program.value.body.value.bindings[0];
        assert_eq!(binding.value.kind, BindingKind::Native);
        assert_eq!(binding.value.name.value, "map");
        assert_eq!(
            binding
                .value
                .type_parameters
                .iter()
                .map(|parameter| parameter.value.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "B"]
        );
        assert_eq!(binding.value.type_parameters[0].location.range(), 16..17);
        assert!(binding.value.annotation.is_some());
        assert_eq!(binding.location.range(), 0..59);
    }

    #[test]
    fn lowers_native_type_declarations_with_explicit_slots() {
        let program = parse(
            "native-type.telora",
            "native type State @3; native new: Fn() -> State; State",
        )
        .unwrap();
        let binding = &program.value.body.value.bindings[0];
        assert_eq!(binding.value.kind, BindingKind::NativeType);
        assert_eq!(binding.value.name.value, "State");
        assert!(binding.value.annotation.is_none());
        assert!(matches!(binding.value.value.value, ExprKind::Int(3)));
        assert_eq!(binding.value.value.location.range(), 19..20);
        assert_eq!(binding.location.range(), 0..21);
    }

    #[test]
    fn retains_decorators_and_lowers_their_rhs_calls() {
        let program = parse(
            "decorators.telora",
            "@outer @factory(1) type T = Int; { @field value: 2 }",
        )
        .unwrap();
        let binding = &program.value.body.value.bindings[0];
        assert_eq!(binding.value.decorators.len(), 2);
        assert!(!binding.value.decorators[0].value.configured);
        assert!(binding.value.decorators[1].value.configured);
        assert!(matches!(binding.value.value.value, ExprKind::Call { .. }));
        let ExprKind::Dict(fields) = &program.value.body.value.result.value else {
            panic!("expected Dict")
        };
        assert_eq!(fields[0].value.decorators.len(), 1);
        assert!(matches!(fields[0].value.value.value, ExprKind::Call { .. }));
    }

    #[test]
    fn lowers_parameterized_type_declarations_with_located_parameters() {
        let program = parse(
            "family.telora",
            "type Pair(Left, Right) = struct {left: Left, right: Right}; Pair",
        )
        .unwrap();
        let binding = &program.value.body.value.bindings[0];
        assert_eq!(binding.value.kind, BindingKind::Type);
        assert_eq!(binding.value.name.value, "Pair");
        assert_eq!(
            binding
                .value
                .type_parameters
                .iter()
                .map(|parameter| parameter.value.as_str())
                .collect::<Vec<_>>(),
            vec!["Left", "Right"]
        );
        assert_eq!(binding.value.type_parameters[0].location.range(), 10..14);
        assert_eq!(binding.value.type_parameters[1].location.range(), 16..21);
        assert!(matches!(binding.value.value.value, ExprKind::Call { .. }));
    }

    #[test]
    fn lowers_struct_and_enum_initializers_to_model_calls() {
        let program = parse(
            "declared.telora",
            "type User = struct {name: String}; type Maybe(A) = enum {'None, 'Some(A)}; Maybe",
        )
        .unwrap();
        let user = &program.value.body.value.bindings[0];
        let ExprKind::Call { callee, arguments } = &user.value.value.value else {
            panic!("expected Struct model call");
        };
        assert!(
            matches!(&callee.value, ExprKind::Variable(name) if name.value == "\0telora_struct")
        );
        assert_eq!(arguments.len(), 2);
        assert!(matches!(&arguments[1].value, ExprKind::Dict(fields) if fields.len() == 1));

        let maybe = &program.value.body.value.bindings[1];
        let ExprKind::Call { callee, arguments } = &maybe.value.value.value else {
            panic!("expected Enum model call");
        };
        assert!(matches!(&callee.value, ExprKind::Variable(name) if name.value == "\0telora_enum"));
        assert!(matches!(&arguments[1].value, ExprKind::Dict(variants) if variants.len() == 2));
    }

    #[test]
    fn rejects_duplicate_declared_type_members() {
        for (source, expected) in [
            (
                "type T = struct {value: Int, value: String};",
                "duplicate Struct field",
            ),
            (
                "type T = enum {'Value, 'Value(Int)};",
                "duplicate Enum variant",
            ),
        ] {
            let error = parse("duplicate.telora", source).unwrap_err();
            assert!(error.message.contains(expected), "{}", error.message);
        }
    }

    #[test]
    fn declaration_initializers_preserve_root_and_member_decorator_order() {
        let program = parse(
            "decorated.telora",
            "@outer type T = struct {@inner value: Int}; T",
        )
        .unwrap();
        let binding = &program.value.body.value.bindings[0];
        assert_eq!(binding.value.decorators.len(), 2);
        let ExprKind::Call {
            callee: outer,
            arguments: outer_arguments,
        } = &binding.value.value.value
        else {
            panic!("expected outer decorator call");
        };
        assert!(matches!(&outer.value, ExprKind::Variable(name) if name.value == "outer"));
        let ExprKind::Call {
            callee: model,
            arguments: model_arguments,
        } = &outer_arguments[1].value
        else {
            panic!("expected Struct model call");
        };
        assert!(
            matches!(&model.value, ExprKind::Variable(name) if name.value == "\0telora_struct")
        );
        let ExprKind::Dict(fields) = &model_arguments[1].value else {
            panic!("expected Struct fields");
        };
        assert!(matches!(fields[0].value.value.value, ExprKind::Call { .. }));
    }

    #[test]
    fn declaration_initializers_are_not_general_expressions_or_variadic_variants() {
        for source in [
            "let value = struct {field: Int}; value",
            "fn() { enum {'None} }",
            "type Event = enum {'Moved(Int, Int)}; Event",
        ] {
            assert!(
                parse("invalid.telora", source).is_err(),
                "accepted {source}"
            );
        }
    }

    #[test]
    fn separates_strings_from_concat_only_contexts() {
        let string_error = parse("test", r#""\{1}""#).unwrap_err();
        assert!(string_error.message.contains("unsupported string escape"));
        assert!(parse("test", r#"match "x" { `\{1}` => 1 }"#).is_err());
        assert!(parse("test", r#"{`\{"x"}`: 1}"#).is_err());
    }

    #[test]
    fn reports_invalid_and_unterminated_string_parts() {
        let invalid = parse("test", r#""bad\q""#).unwrap_err();
        assert!(invalid.message.contains("unsupported string escape"));
        assert_eq!(invalid.location.offset, 4);

        let unterminated = parse("test", r#""unfinished"#).unwrap_err();
        assert!(unterminated.message.contains("expected"));

        let non_ascii = parse("test", r#""\xff""#).unwrap_err();
        assert!(non_ascii.message.contains("must be ASCII"));

        let invalid_scalar = parse("test", r#""\u{d800}""#).unwrap_err();
        assert!(invalid_scalar.message.contains("Unicode scalar"));
    }

    #[test]
    fn lowers_only_immediate_ordered_module_options() {
        let program = parse(
            "options.telora",
            r#"option "module.documentation" {name: "tool"}; export def value = 0; option "module.documentation" 'Stable;"#,
        )
        .unwrap();
        assert_eq!(program.value.options.len(), 2);
        assert_eq!(program.value.options[0].key.value, "module.documentation");
        assert!(matches!(
            program.value.options[0].value.value,
            ExprKind::Dict(_)
        ));
        assert!(matches!(
            program.value.options[1].value.value,
            ExprKind::Atom(_)
        ));

        for invalid in [
            "@@manifest {}; export def value = 0;",
            "option \"documentation\" {}; export def value = 0;",
            "option \"module.documentation\" value; export def value = 0;",
            "option \"module.documentation\" `tool-\\{value}`; export def value = 0;",
            "option \"module.documentation\" {...value}; export def value = 0;",
            "export def f = fn() { option \"module.documentation\" {}; 0 };",
        ] {
            assert!(
                parse("invalid-option.telora", invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn selective_imports_are_in_scope_for_exported_definition_contracts() {
        let program = parse(
            "selective-type.telora",
            r#"import "std/rt-types/exec.telora" { ExecFn };
               export def exec: ExecFn = fn(settings, request) { request };"#,
        )
        .unwrap();
        assert_eq!(
            program
                .value
                .body
                .value
                .bindings
                .iter()
                .map(|binding| binding.value.name.value.as_str())
                .collect::<Vec<_>>(),
            vec!["ExecFn", "exec", "exec"]
        );
        let hir = crate::hir::HirProgram::resolve(&program, Vec::<String>::new());
        assert!(
            hir.unresolved().next().is_none(),
            "selective import must precede the exported contract"
        );
    }

    #[test]
    fn preserves_interpreter_operand_in_ast() {
        let program = parse(
            "interpreter.telora",
            "def lift: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool = interpreter!(eq_i); lift",
        )
        .unwrap();
        let value = &program.value.body.value.bindings[0].value.value;
        let ExprKind::Interpreter { operand, .. } = &value.value else {
            panic!("expected interpreter expression")
        };
        assert!(matches!(
            &operand.value,
            ExprKind::Variable(name) if name.value == "eq_i"
        ));
    }

    #[test]
    fn parses_propagation_as_a_postfix_expression() {
        let program = parse("propagate.telora", "value?").unwrap();
        assert!(matches!(
            program.value.body.value.result.value,
            ExprKind::Propagate { .. }
        ));

        let program = parse("propagate.telora", "left + right?").unwrap();
        let ExprKind::Binary { right, .. } = &program.value.body.value.result.value else {
            panic!("expected binary expression")
        };
        assert!(matches!(right.value, ExprKind::Propagate { .. }));
    }

    #[test]
    fn diagnoses_contextual_intrinsic_names_and_arity() {
        for (source, expected) in [
            (
                "def lift: for(A) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter(eq_i); lift",
                "interpreter(...) has been replaced by interpreter!(...)",
            ),
            ("unknown!(1)", "unknown contextual intrinsic unknown!"),
            ("blame!()", "unknown contextual intrinsic blame!"),
            (
                "dbg!()",
                "dbg! expects an expression and an optional String literal",
            ),
            (
                "let message = \"dynamic\"; dbg!(1, message)",
                "dbg! message must be a String literal",
            ),
            (
                "must_ok!()",
                "must_ok! expects a checker followed by zero or more arguments",
            ),
            (
                "fail!()",
                "fail! expects a message followed by zero or more subjects",
            ),
            ("file!()", "file! is reserved but not implemented"),
            ("line!()", "line! is reserved but not implemented"),
            (
                "def lift: for(A) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter!(); lift",
                "interpreter! expects exactly one argument, found 0",
            ),
            (
                "def lift: for(A) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter!(a, b); lift",
                "interpreter! expects exactly one argument, found 2",
            ),
        ] {
            let error = parse("intrinsic.telora", source).unwrap_err();
            assert!(
                error.message.contains(expected),
                "expected {expected:?}, got {:?}",
                error.message
            );
        }
    }

    #[test]
    fn lowers_prefix_and_postfix_debug_with_authored_expression_names() {
        for (source, expected_name) in [
            ("dbg!(request.user, \"seen\")", "request.user"),
            ("request.user.dbg!(\"seen\")", "request.user"),
            ("make(1).dbg!().field", "make(1)"),
            ("items[0].dbg!()", "items[0]"),
        ] {
            let program = parse("debug.telora", source).unwrap();
            let mut expression = &program.value.body.value.result;
            if let ExprKind::Field { receiver, .. } = &expression.value {
                expression = receiver;
            }
            let ExprKind::Debug {
                expression: name,
                message,
                ..
            } = &expression.value
            else {
                panic!("expected contextual debug for {source}")
            };
            assert_eq!(name, expected_name);
            assert_eq!(
                message.as_deref(),
                source.contains("seen").then_some("seen")
            );
        }
    }

    #[test]
    fn postfix_contextual_intrinsics_prepend_the_receiver() {
        let program = parse("postfix.telora", "\"bad\".fail!(error)").unwrap();
        let ExprKind::Raise { error } = &program.value.body.value.result.value else {
            panic!("expected raise")
        };
        assert!(matches!(&error.value, ExprKind::Dict(_)));

        let error = parse("postfix.telora", "error.raise!()").unwrap_err();
        assert!(
            error
                .message
                .contains("unknown contextual intrinsic raise!")
        );

        let error = parse("postfix.telora", "value.unknown!()").unwrap_err();
        assert!(
            error
                .message
                .contains("unknown contextual intrinsic unknown!")
        );
    }

    #[test]
    fn lowers_fail_to_a_raise_with_an_internal_envelope() {
        let program = parse("fail.telora", "fail!(message, data)").unwrap();
        let ExprKind::Raise { error } = &program.value.body.value.result.value else {
            panic!("expected raise")
        };
        let ExprKind::Dict(fields) = &error.value else {
            panic!("expected envelope")
        };
        assert_eq!(
            fields
                .iter()
                .map(|field| {
                    field
                        .value
                        .name
                        .as_ref()
                        .expect("blame fields have names")
                        .value
                        .as_str()
                })
                .collect::<Vec<_>>(),
            ["data", "message", "rule"]
        );
        assert!(matches!(
            &fields[2].value.value.value,
            ExprKind::String(marker) if marker == "fail!"
        ));
        assert_eq!(fields[2].value.value.location.range(), 0..20);
    }
