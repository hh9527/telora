    #[test]
    fn chunk_bridge_matches_contiguous_lexing() {
        let samples = [
            r#"{"key":"text\\nvalue","number":-12.5e+3}"#,
            r#"[true,false,null,"😀","\\u4e2d"]"#,
            r#"{"bad":"\\u12x","next":1}"#,
        ];
        for sample in samples {
            let mut expected_diags = Vec::new();
            let expected = tokenize(sample, &mut expected_diags);
            for split in sample
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(sample.len()))
            {
                let mut actual_diags = Vec::new();
                let actual = tokenize_fragments(
                    [sample.get(..split).unwrap(), sample.get(split..).unwrap()],
                    &mut actual_diags,
                );
                assert_eq!(actual, expected, "split at {split} in {sample:?}");
                assert_eq!(
                    actual_diags, expected_diags,
                    "split at {split} in {sample:?}"
                );
            }
        }

        let source = format!(r#"{{"value":"{}"}}"#, "long text \\n ".repeat(200));
        let document = crate::document::DocumentText::new(&source);
        assert!(document.chunks().count() > 1);
        let mut expected_diags = Vec::new();
        let expected = tokenize(&source, &mut expected_diags);
        let mut actual_diags = Vec::new();
        let actual = tokenize_document(&document, &mut actual_diags);
        assert_eq!(actual, expected);
        assert_eq!(actual_diags, expected_diags);
    }

    #[test]
    fn recognizes_text_and_each_escape_as_source_ranges() {
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(r#""a\n\u0041b""#, &mut diagnostics);
        assert!(diagnostics.is_empty());
        assert_eq!(
            tokens,
            vec![
                Token::DoubleQuote,
                Token::StringText,
                Token::EscapeSequence,
                Token::EscapeSequence,
                Token::StringText,
                Token::DoubleQuote,
            ]
        );
        assert_eq!(spans[2], 2..4);
        assert_eq!(spans[3], 4..10);

        let (tokens, spans) = tokenize(r#"["text"]"#, &mut diagnostics);
        assert_eq!(tokens[1], Token::DoubleQuote);
        assert_eq!(spans[1], 1..2);
        assert_eq!(spans[2], 2..6);
        assert_eq!(spans[3], 6..7);
    }

    #[test]
    fn preserves_unknown_and_malformed_escapes_as_tokens() {
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(r#""a\q\u12xxb""#, &mut diagnostics);
        assert_eq!(tokens[2], Token::UnknownEscapeSequence);
        assert_eq!(tokens[3], Token::MalformedUnicodeEscape);
        assert_eq!(spans[2], 2..4);
        assert_eq!(spans[3], 4..10);
        assert_eq!(diagnostics.len(), 2);
    }
