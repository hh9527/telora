    #[test]
    fn cst_is_lossless_for_toml_tables_comments_and_strings() {
        let source = "# package\n[package]\nname = \"telora\" # comment\nlines = '''a\nb'''\n";
        let mut sources = crate::SourceDatabase::default();
        let id = sources.add("data.toml", source);
        let parsed = parse(id, source);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }
