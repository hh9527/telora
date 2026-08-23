    #[test]
    fn cst_is_lossless_for_json_trivia() {
        let source = "{\n  \"ok\": true\n}";
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("data.json", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors());
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }

    #[test]
    fn cst_preserves_empty_text_and_unicode_escape_structure() {
        let source = r#"["", "a\n\u0041"]"#;
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("strings.json", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let tokens = collect_tokens(&parsed.syntax, NodeRef::ROOT);
        assert_eq!(
            tokens,
            vec![
                Token::LBracket,
                Token::DoubleQuote,
                Token::DoubleQuote,
                Token::Comma,
                Token::Whitespace,
                Token::DoubleQuote,
                Token::StringText,
                Token::EscapeSequence,
                Token::EscapeSequence,
                Token::DoubleQuote,
                Token::RBracket,
            ]
        );
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }

    #[test]
    fn malformed_escape_is_diagnosed_without_breaking_the_string_cst() {
        let source = r#""a\u12xxb""#;
        let mut sources = crate::source::SourceDatabase::default();
        let id = sources.add("escape.json", source);
        let parsed = parse(id, source);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(parsed.diagnostics[0].labels[0].location.range(), 2..8);
        let tokens = collect_tokens(&parsed.syntax, NodeRef::ROOT);
        assert_eq!(
            tokens,
            [
                Token::DoubleQuote,
                Token::StringText,
                Token::MalformedUnicodeEscape,
                Token::StringText,
                Token::DoubleQuote,
            ]
        );
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
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
