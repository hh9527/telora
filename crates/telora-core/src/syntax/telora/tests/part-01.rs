    #[test]
    fn cst_is_lossless_and_recovery_collects_diagnostics() {
        let source = "#!/usr/bin/env -S telora run\nlet x = 1 # keep me\n let y = ;\n match x { => 1, _ => }";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("broken.telora", source);
        let parsed = parse(id, source);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
        assert!(parsed.diagnostics.len() >= 2);
    }
    #[test]
    fn damaged_declaration_initializers_recover_later_bindings() {
        let source = "type Broken = struct {value: }; let after = 2; after";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("broken-declared.telora", source);
        let parsed = parse(id, source);
        assert!(parsed.has_errors());
        let names = Program::cast(&parsed.syntax, NodeRef::ROOT)
            .unwrap()
            .body()
            .unwrap()
            .bindings()
            .filter_map(Binding::name)
            .map(|name| {
                let range = name.range();
                source[range.start as usize..range.end as usize].to_owned()
            })
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == "after"), "{names:?}");
    }

    #[test]
    fn typed_views_query_later_syntax_around_a_missing_value() {
        let source = "let x = ; let y = 2; y";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("incomplete.telora", source);
        let parsed = parse(id, source);
        let program = Program::root(&parsed.syntax);
        let body = program.body().unwrap();
        let bindings = body.bindings().collect::<Vec<_>>();
        assert_eq!(bindings.len(), 2);
        assert_eq!(parsed.diagnostics.len(), 1);
        let names = bindings
            .iter()
            .map(|binding| {
                let range = binding.name().unwrap().range().to_usize();
                &source[range]
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["x", "y"]);
        assert_eq!(
            bindings[1].name().unwrap().range(),
            bindings[1].name().unwrap().range()
        );
        let Binding::Let(first) = bindings[0] else {
            panic!("expected let binding");
        };
        assert!(first.value().is_none());
        assert!(body.result().is_some());

        let issues = ast::validate(id, &parsed.syntax);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].expected, ExpectedSyntax::BindingValue);
        assert!(issues[0].location.start == issues[0].location.end);
        assert_eq!(
            std::mem::size_of_val(&program),
            std::mem::size_of_val(&program.syntax())
        );
    }

    #[test]
    fn typed_queries_tolerate_error_subtrees_and_arbitrary_input() {
        let samples = [
            "",
            "let",
            "let = ;",
            "let x = 1, 2; let y = 3; y",
            "\0",
            "let 名字 = ; 名字",
        ];
        for source in samples {
            let mut sources = crate::source::SourceDatabase::default();
            let id = sources.add("sample.telora", source);
            let parsed = parse(id, source);
            let program = Program::root(&parsed.syntax);
            let _ = program.body().map(|body| {
                body.bindings()
                    .map(|binding| binding.name())
                    .collect::<Vec<_>>()
            });
            let _ = ast::validate(id, &parsed.syntax);
        }

        let source = "let x = 1, 2; let y = 3; y";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("error.telora", source);
        let parsed = parse(id, source);
        assert!(contains_rule_error(&parsed.syntax, NodeRef::ROOT));
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);

        let source = "let = 1; 0";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("missing-name.telora", source);
        let parsed = parse(id, source);
        let issues = ast::validate(id, &parsed.syntax);
        assert!(
            issues
                .iter()
                .any(|issue| issue.expected == ExpectedSyntax::BindingName)
        );
        assert_eq!(parsed.diagnostics.len(), 1);
    }

    #[test]
    fn unknown_escape_remains_inside_a_queryable_string() {
        let source = r#""a\(b""#;
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("escape.telora", source);
        let parsed = parse(id, source);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].labels[0].location.range(), 2..4);
        let string_node = find_rule(&parsed.syntax, NodeRef::ROOT, parser::Rule::StringLiteral)
            .expect("string literal remains in CST");
        let string = StringLiteral::cast(&parsed.syntax, string_node).unwrap();
        let parts = string
            .parts()
            .filter_map(|part| part.token().map(|token| token.kind()))
            .collect::<Vec<_>>();
        assert_eq!(
            parts,
            [
                Token::StringText,
                Token::UnknownEscapeSequence,
                Token::StringText
            ]
        );
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }
