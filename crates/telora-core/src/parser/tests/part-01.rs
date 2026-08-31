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
    fn malformed_else_if_chain_recovers_without_panicking() {
        let mut sources = SourceDatabase::default();
        let id = sources.add("test.telora", "if 'True { 1 } else if { 2 } else { 3 }");
        let parsed = parse_registered(&sources, id);
        assert!(!parsed.diagnostics.is_empty());
        assert!(parsed.program.is_none());
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
                "type Broken = enum { @bad(\"name\") }; export {Broken};",
                &["missing Atom"],
            ),
            (
                "type Broken = enum { @bad(\"name\", 'Bad }; export {Broken};",
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
