    #[test]
    fn recognizes_the_complete_comparison_operator_family() {
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize("< <= > >= == !=", &mut diagnostics);
        assert_eq!(
            tokens
                .into_iter()
                .filter(|token| *token != Token::Whitespace)
                .collect::<Vec<_>>(),
            vec![
                Token::Less,
                Token::LessEqual,
                Token::Greater,
                Token::GreaterEqual,
                Token::EqualEqual,
                Token::BangEqual,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_remainder_as_an_operator() {
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize("7 % 3", &mut diagnostics);
        assert_eq!(
            tokens
                .into_iter()
                .filter(|token| *token != Token::Whitespace)
                .collect::<Vec<_>>(),
            vec![Token::Int, Token::Percent, Token::Int]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn distinguishes_bitwise_boolean_and_pipeline_operators() {
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize("! != & && | || |> ^", &mut diagnostics);
        assert_eq!(
            tokens
                .into_iter()
                .filter(|token| *token != Token::Whitespace)
                .collect::<Vec<_>>(),
            vec![
                Token::Bang,
                Token::BangEqual,
                Token::BitAnd,
                Token::AndAnd,
                Token::BitOr,
                Token::OrOr,
                Token::Pipe,
                Token::BitXor,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn hash_is_the_only_line_comment_marker() {
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize("# comment\n//", &mut diagnostics);
        assert_eq!(
            tokens,
            vec![
                Token::Comment,
                Token::Whitespace,
                Token::Slash,
                Token::Slash,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn distinguishes_chained_projections_from_float_literals() {
        let mut diagnostics = Vec::new();
        let source = "pair.1.0 1.0 1.0.1 pair. 12.34";
        let (tokens, spans) = tokenize(source, &mut diagnostics);
        let significant = tokens
            .iter()
            .copied()
            .zip(spans.iter())
            .filter(|(token, _)| !matches!(token, Token::Whitespace | Token::Comment))
            .collect::<Vec<_>>();
        assert_eq!(
            significant
                .iter()
                .map(|(token, _)| *token)
                .collect::<Vec<_>>(),
            vec![
                Token::Identifier,
                Token::Dot,
                Token::Int,
                Token::Dot,
                Token::Int,
                Token::Float,
                Token::Float,
                Token::Dot,
                Token::Int,
                Token::Identifier,
                Token::Dot,
                Token::Int,
                Token::Dot,
                Token::Int,
            ]
        );
        assert_eq!(significant[2].1, &(5..6));
        assert_eq!(significant[3].1, &(6..7));
        assert_eq!(significant[4].1, &(7..8));
        assert_eq!(significant[11].1, &(25..27));
        assert_eq!(significant[12].1, &(27..28));
        assert_eq!(significant[13].1, &(28..30));
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_float_exponent_notation_without_affecting_projections() {
        let mut diagnostics = Vec::new();
        let source = "1e3 1.25e-3 1.0E+8 pair.1.0 pair.1e2";
        let (tokens, spans) = tokenize(source, &mut diagnostics);
        let significant = tokens
            .iter()
            .copied()
            .zip(spans.iter())
            .filter(|(token, _)| !matches!(token, Token::Whitespace | Token::Comment))
            .map(|(token, span)| (token, &source[span.clone()]))
            .collect::<Vec<_>>();
        assert_eq!(
            significant,
            vec![
                (Token::Float, "1e3"),
                (Token::Float, "1.25e-3"),
                (Token::Float, "1.0E+8"),
                (Token::Identifier, "pair"),
                (Token::Dot, "."),
                (Token::Int, "1"),
                (Token::Dot, "."),
                (Token::Int, "0"),
                (Token::Identifier, "pair"),
                (Token::Dot, "."),
                (Token::Float, "1e2"),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn contextualizes_only_direct_struct_and_enum_type_initializers() {
        let source = "type User = struct {name: String}; type Maybe(A) = enum {'None, 'Some(A)}; struct('None, {}); type Legacy = struct {}";
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(source, &mut diagnostics);
        let significant = tokens
            .iter()
            .copied()
            .zip(spans.iter())
            .filter(|(token, _)| !matches!(token, Token::Whitespace | Token::Comment))
            .filter(|(_, span)| matches!(&source[(*span).clone()], "struct" | "enum"))
            .map(|(token, span)| (token, &source[span.clone()]))
            .collect::<Vec<_>>();
        assert_eq!(
            significant,
            vec![
                (Token::StructInitializer, "struct"),
                (Token::EnumInitializer, "enum"),
                (Token::Identifier, "struct"),
                (Token::StructInitializer, "struct"),
            ]
        );
        assert!(diagnostics.is_empty());

        let document = crate::document::DocumentText::new(source);
        let mut document_diagnostics = Vec::new();
        assert_eq!(
            tokenize_document(&document, &mut document_diagnostics),
            (tokens, spans)
        );
        assert!(document_diagnostics.is_empty());
    }

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
    fn recognizes_bare_and_indexed_placeholders_as_dedicated_tokens() {
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(r"f\(_, _1, _0, _name)", &mut diagnostics);
        assert!(diagnostics.is_empty());
        assert_eq!(
            tokens,
            vec![
                Token::Identifier,
                Token::SectionLParen,
                Token::Placeholder,
                Token::Comma,
                Token::Whitespace,
                Token::IndexedPlaceholder,
                Token::Comma,
                Token::Whitespace,
                Token::IndexedPlaceholder,
                Token::Comma,
                Token::Whitespace,
                Token::Identifier,
                Token::RParen,
            ]
        );
        assert_eq!(spans[2], 3..4);
        assert_eq!(spans[5], 6..8);
        assert_eq!(spans[11], 14..19);
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
