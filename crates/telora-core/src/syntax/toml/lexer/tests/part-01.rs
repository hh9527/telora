    #[test]
    fn keeps_temporal_values_and_fractional_seconds_atomic() {
        let source = "when = 1979-05-27 07:32:00+00:00\ndates = [1979-05-27, 07:32:00.1200]\n";
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(source, &mut diagnostics);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let atoms = tokens
            .iter()
            .zip(&spans)
            .filter(|(token, _)| **token == Token::Atom)
            .map(|(_, span)| &source[span.clone()])
            .collect::<Vec<_>>();
        assert_eq!(
            atoms,
            [
                "when",
                "1979-05-27 07:32:00+00:00",
                "dates",
                "1979-05-27",
                "07:32:00.1200"
            ]
        );
    }

    #[test]
    fn chunk_bridge_matches_contiguous_toml_lexing() {
        let source = "title = \"Telora\"\ntext = \"\"\"one\ntwo\"\"\"\nvalues = [1,\n2]\n";
        let mut contiguous_diagnostics = Vec::new();
        let contiguous = tokenize(source, &mut contiguous_diagnostics);
        for split in source
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(source.len()))
        {
            let mut chunked_diagnostics = Vec::new();
            let chunked = tokenize_fragments(
                [
                    source.get(..split).expect("character boundary"),
                    source.get(split..).expect("character boundary"),
                ],
                source.len(),
                &mut chunked_diagnostics,
            );
            assert_eq!(chunked, contiguous, "split at {split}");
            assert_eq!(
                chunked_diagnostics, contiguous_diagnostics,
                "split at {split}"
            );
        }
    }
