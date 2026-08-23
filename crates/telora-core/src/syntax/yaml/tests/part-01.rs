    #[test]
    fn cst_is_lossless_for_yaml_comments_and_block_scalars() {
        let source = "# package\r\nname: Telora\r\ntext: |\r\n  one\r\n  two\r\n";
        let document = crate::DocumentText::new(source);
        let mut sources = crate::SourceDatabase::default();
        let source_id = sources.add_document("data.yaml", document.clone());
        let parsed = parse_document(source_id, &document);
        assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
        let mut reconstructed = String::new();
        reconstruct(&parsed.syntax, source, NodeRef::ROOT, &mut reconstructed);
        assert_eq!(reconstructed, source);
    }
