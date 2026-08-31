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
    fn cst_preserves_native_declarations_losslessly() {
        let source = "native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B); map";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("native.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
        let program = Program::cast(&parsed.syntax, NodeRef::ROOT).unwrap();
        let Some(Binding::Native(native)) = program.body().unwrap().bindings().next() else {
            panic!("expected native binding");
        };
        assert!(native.type_parameters().is_some());
    }

    #[test]
    fn cst_preserves_path_first_module_bindings() {
        let source = "import \"std/array\" as arrays, *; import \"std/array\" as qualified, { map, filter as select }; let from = 1; (arrays, qualified, map, select, from)";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("imports.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);

        for rejected in [
            "import \"std/array\"; 0",
            "import \"std/array\" * as arrays; arrays",
        ] {
            let id = sources.add("rejected-import.telora", rejected);
            assert!(parse(id, rejected).has_errors(), "{rejected}");
        }
    }

    #[test]
    fn cst_preserves_multiple_explicit_exports_without_a_final_expression() {
        let source = "export def value = 1; export type User = struct { name: String }; export { value as output, User };";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("exports.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
        assert!(
            Program::cast(&parsed.syntax, NodeRef::ROOT)
                .unwrap()
                .body()
                .unwrap()
                .bindings()
                .any(|binding| matches!(binding, Binding::Export(_)))
        );
    }

    #[test]
    fn cst_exposes_declared_struct_and_enum_initializers() {
        let source = "type User = struct {name: String}; type Maybe(A) = enum {'None, 'Some(A)};";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("declared.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let bindings = Program::cast(&parsed.syntax, NodeRef::ROOT)
            .unwrap()
            .body()
            .unwrap()
            .bindings()
            .collect::<Vec<_>>();
        let Binding::Type(user) = bindings[0] else {
            panic!("expected User type binding");
        };
        let Some(TypeInitializer::Struct(initializer)) = user.initializer() else {
            panic!("expected Struct initializer");
        };
        assert_eq!(initializer.fields().count(), 1);
        let Binding::Type(maybe) = bindings[1] else {
            panic!("expected Maybe type binding");
        };
        let Some(TypeInitializer::Enum(initializer)) = maybe.initializer() else {
            panic!("expected Enum initializer");
        };
        assert_eq!(initializer.variants().count(), 2);
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
    fn cst_preserves_dict_field_shorthand_losslessly() {
        let source = "let name = \"telora\"; { name, explicit: 1, ...extra }";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("shorthand.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);

        for invalid in [
            "let other = 1; { other, @tag name }",
            "let other = 1; { other, name.value }",
        ] {
            let id = sources.add("invalid-shorthand.telora", invalid);
            assert!(parse(id, invalid).has_errors(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn cst_preserves_chained_tuple_projections_losslessly() {
        let source = "let pair = (0, (1, \"ok\")); pair.1.0";
        let document = crate::document::DocumentText::new(source);
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("projection.telora", source);
        let parsed = parse_document(id, &document);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }

    #[test]
    fn cst_preserves_generic_definition_declarations_losslessly() {
        let source =
            "decl identity: for(A) Fn(A) -> A; def identity = fn(value) { value }; identity";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("identity.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
        let program = Program::cast(&parsed.syntax, NodeRef::ROOT).unwrap();
        let Some(Binding::Decl(declaration)) = program.body().unwrap().bindings().next() else {
            panic!("expected declaration");
        };
        assert!(declaration.type_parameters().is_some());
        assert!(declaration.contract().is_some());
    }

    #[test]
    fn cst_preserves_interpreter_expressions_losslessly() {
        let source =
            "def lift: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool = interpreter!(eq_i); lift";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("interpreter.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
        let program = Program::cast(&parsed.syntax, NodeRef::ROOT).unwrap();
        let Some(Binding::Def(definition)) = program.body().unwrap().bindings().next() else {
            panic!("expected definition");
        };
        assert!(definition.value().is_some());
    }

    #[test]
    fn cst_preserves_contextual_intrinsics_losslessly() {
        let source = "fail!(\"bad\", data)";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("fail.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }

    #[test]
    fn cst_preserves_annotated_definitions() {
        let source = "def identity: for(A) Fn(A) -> A = fn(value) { value }; identity";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("annotated-def.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
        let program = Program::cast(&parsed.syntax, NodeRef::ROOT).unwrap();
        let Some(Binding::Def(definition)) = program.body().unwrap().bindings().next() else {
            panic!("expected definition");
        };
        assert!(definition.type_parameters().is_some());
        assert!(definition.contract().is_some());
    }

    #[test]
    fn native_type_schemes_reject_nested_for_binders() {
        let source = "native use: Fn(for(A) Fn(A) -> A) -> Int; use";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("nested-scheme.telora", source);
        let parsed = parse(id, source);
        assert!(parsed.has_errors());
    }

    #[test]
    fn native_type_slots_are_lossless_and_required() {
        let source = "native type State @3; State";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("native-type.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }

    #[test]
    fn cst_preserves_type_and_field_decorators_losslessly() {
        let source = "@outer @factory(1) type T = Int; { @field value: 2 }";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("decorators.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
        let program = Program::cast(&parsed.syntax, NodeRef::ROOT).unwrap();
        let Binding::Type(binding) = program.body().unwrap().bindings().next().unwrap() else {
            panic!("expected type binding")
        };
        assert_eq!(binding.decorators().count(), 2);
    }

    #[test]
    fn decorators_reject_unsupported_binding_targets() {
        let source = "@f let value = 1; value";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("unsupported.telora", source);
        let parsed = parse(id, source);
        assert!(parsed.has_errors());
    }

    #[test]
    fn cst_preserves_string_quotes_text_escapes_and_interpolation() {
        let source = r#"`hi\n \{name}`"#;
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("strings.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let tokens = parsed
            .syntax
            .children(NodeRef::ROOT)
            .flat_map(|node| collect_tokens(&parsed.syntax, node))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                Token::Backtick,
                Token::StringText,
                Token::EscapeSequence,
                Token::StringText,
                Token::InterpolationStart,
                Token::Identifier,
                Token::RBrace,
                Token::Backtick,
            ]
        );
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }

    #[test]
    fn cst_preserves_raw_strings_and_explicit_continuations() {
        let source = r####"(r##"raw "quotes" and \slashes"##, "first \
    second")"####;
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("strings.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let tokens = parsed
            .syntax
            .children(NodeRef::ROOT)
            .flat_map(|node| collect_tokens(&parsed.syntax, node))
            .collect::<Vec<_>>();
        assert!(tokens.contains(&Token::RawString));
        assert!(tokens.contains(&Token::EscapeSequence));
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }

    #[test]
    fn cst_preserves_explicit_call_sections() {
        let source = r"value |> transform\(_1, 123, _0)";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("section.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
        let tokens = parsed
            .syntax
            .children(NodeRef::ROOT)
            .flat_map(|node| collect_tokens(&parsed.syntax, node))
            .collect::<Vec<_>>();
        assert!(tokens.contains(&Token::SectionLParen));
        assert!(tokens.contains(&Token::IndexedPlaceholder));
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

    #[test]
    fn cst_preserves_trait_and_impl_declarations() {
        let source = r#"trait Display { display: Fn(Self) -> String, };
impl Display for Endpoint { display: fn(value) { value.name }, };
impl(T: Property(DisplayBy)) Display for T { display: fn(value) { "ok" }, };"#;
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("traits.telora", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let bindings = Program::root(&parsed.syntax)
            .body()
            .unwrap()
            .bindings()
            .collect::<Vec<_>>();
        assert!(matches!(bindings[0], ast::Binding::Trait(_)));
        assert!(matches!(bindings[1], ast::Binding::Impl(_)));
        assert!(matches!(bindings[2], ast::Binding::Impl(_)));
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }

    fn find_rule(cst: &CstData, node: NodeRef, expected: parser::Rule) -> Option<NodeRef> {
        if matches!(cst.get(node), Node::Rule(rule, _) if rule == expected) {
            return Some(node);
        }
        cst.children(node)
            .find_map(|child| find_rule(cst, child, expected))
    }

    fn contains_rule_error(cst: &CstData, node: NodeRef) -> bool {
        matches!(cst.get(node), Node::Rule(parser::Rule::Error, _))
            || cst
                .children(node)
                .any(|child| contains_rule_error(cst, child))
    }

    fn collect_tokens(cst: &CstData, node: NodeRef) -> Vec<Token> {
        match cst.get(node) {
            Node::Token(token, _) => vec![token],
            Node::Rule(..) => cst
                .children(node)
                .flat_map(|child| collect_tokens(cst, child))
                .collect(),
        }
    }

    #[test]
    fn full_file_parse_baseline() {
        let source = include_str!("../../../../../../examples/mvp/main.telora");
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("main.telora", source);
        let started = std::time::Instant::now();
        for _ in 0..1_000 {
            assert!(!parse(id, source).has_errors());
        }
        eprintln!("1,000 full parses: {:?}", started.elapsed());
    }
