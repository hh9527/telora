    #[test]
    fn chunk_bridge_matches_contiguous_lexing() {
        let samples = [
            "#!/usr/bin/env -S telora run\nlet identifier = 123.456 # comment\nidentifier",
            r#"b\"bytes\" `text \{name} tail`"#,
            r####"r##"raw "quotes", \slashes and `ticks`"##"####,
            r#"`first \
                second \{name}`"#,
            "_12 |> transform\\(_1, 2)",
            "let 中 = \"emoji 😀 and escape \\n\"; 中",
            "let option = 1; option \"module.test\" {}; option",
            "let pair = (0, (1, 2)); pair.1.0; (pair. 1.0, 1.0.1)",
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
                    &crate::document::DocumentText::new(sample),
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

        let source = format!(
            "let value = \"{}\"; value",
            "long text with an escape \\n and interpolation-like text ".repeat(100)
        );
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
    fn recognizes_structured_string_slices_without_payloads() {
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(r#"`hi, \{name}\n`"#, &mut diagnostics);
        assert!(diagnostics.is_empty());
        assert_eq!(
            tokens,
            vec![
                Token::Backtick,
                Token::StringText,
                Token::InterpolationStart,
                Token::Identifier,
                Token::RBrace,
                Token::EscapeSequence,
                Token::Backtick,
            ]
        );
        assert_eq!(spans[1], 1..5);
        assert_eq!(spans[2], 5..7);
        assert_eq!(spans[5], 12..14);

        let (tokens, spans) = tokenize(r#"let x = "text""#, &mut diagnostics);
        let quote = tokens
            .iter()
            .position(|token| *token == Token::DoubleQuote)
            .unwrap();
        assert_eq!(spans[quote], 8..9);
        assert_eq!(spans[quote + 1], 9..13);
        assert_eq!(spans[quote + 2], 13..14);
    }

    #[test]
    fn preserves_unknown_and_unterminated_escapes_as_tokens() {
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(r#""a\(b""#, &mut diagnostics);
        assert_eq!(tokens[2], Token::UnknownEscapeSequence);
        assert_eq!(spans[2], 2..4);
        assert_eq!(diagnostics.len(), 1);

        diagnostics.clear();
        let (tokens, spans) = tokenize("\"a\\", &mut diagnostics);
        assert_eq!(tokens.last(), Some(&Token::UnterminatedEscapeSequence));
        assert_eq!(spans.last(), Some(&(2..3)));
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn diagnoses_raw_string_delimiter_boundaries() {
        let too_many = format!("r{}\"value\"{}", "#".repeat(256), "#".repeat(256));
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize(&too_many, &mut diagnostics);
        assert_eq!(tokens[0], Token::RawString);
        assert!(diagnostics[0].message.contains("255"));

        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize("r##\"unfinished\"#", &mut diagnostics);
        assert_eq!(tokens, [Token::RawString]);
        assert!(diagnostics[0].message.contains("unterminated raw String"));
    }
