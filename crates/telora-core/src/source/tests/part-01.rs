    #[test]
    fn compact_layout_and_checked_ranges() {
        assert_eq!(std::mem::size_of::<SourceId>(), 4);
        assert_eq!(std::mem::size_of::<TextRange>(), 8);
        assert_eq!(std::mem::size_of::<Location>(), 12);
        assert_eq!(std::mem::size_of::<Option<Location>>(), 12);
        assert!(TextRange::new(2, 1).is_err());
        assert!(TextRange::from_usize(0..usize::MAX).is_err());

        let mut sources = SourceDatabase::default();
        let first = sources.add("first", "abc");
        let second = sources.add("second", "xyz");
        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 2);
        let location = Location::from_usize(first, 1..2).unwrap();
        assert_eq!(sources.get(first).slice(location).as_deref(), Some("b"));
        assert_eq!(sources.get(second).slice(location), None);
    }

    #[test]
    fn resolves_utf8_byte_offsets_to_character_columns() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("utf8.telora", "一二\n  x");
        let location = Location::new(source, TextRange::new(9, 10).unwrap());
        let diagnostic = Diagnostic::error("bad value", location);
        assert_eq!(sources.render(&diagnostic), "utf8.telora:2:3: bad value");
        let file = sources.get(source);
        assert_eq!(file.offset(1, 1), Some(0));
        assert_eq!(file.offset(1, 2), Some(3));
        assert_eq!(file.offset(1, 3), Some(6));
        assert_eq!(file.offset(1, 4), None);
        assert_eq!(file.offset(2, 3), Some(9));
    }

    #[test]
    fn validation_diagnostic_can_label_data_and_rule_sources() {
        let mut sources = SourceDatabase::default();
        let data = sources.add("user.json", "{\"age\":\"old\"}");
        let rule = sources.add("schema.telora", "type User = Int;");
        let diagnostic = Diagnostic::error(
            "expected Int",
            Location::new(data, TextRange::new(7, 12).unwrap()),
        )
        .with_secondary(
            "required by User",
            Location::new(rule, TextRange::new(12, 15).unwrap()),
        );
        assert_eq!(
            sources.render(&diagnostic),
            "user.json:1:8: expected Int\n  schema.telora:1:13: required by User"
        );
    }
