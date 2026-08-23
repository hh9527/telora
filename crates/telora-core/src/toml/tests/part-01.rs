    #[test]
    fn direct_materialization_does_not_touch_the_target_on_validation_failure() {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("invalid.toml", "value = []\nvalue = 1\n");
        let mut heap = Heap::main();
        let before = heap.allocation_count();
        assert!(materialize_toml_registered(&sources, source_id, &mut heap).is_err());
        assert_eq!(heap.allocation_count(), before);
    }

    #[test]
    fn lowers_tables_arrays_inline_values_and_temporal_tags() {
        let parsed = parse(
            r#"title = "Telora"
when = 1979-05-27 07:32:00+00:00
local = 1979-05-27T07:32:00
dates = [1979-05-27, 07:32:00.1200]
point = { x = 1, y = 2 }
[owner]
name = 'Ada'
[[products]]
name = "one"
[[products]]
name = "two"
"#,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(
            parsed.value.unwrap().value.to_string(),
            "{dates: ['LocalDate(\"1979-05-27\"), 'LocalTime(\"07:32:00.1200\")], local: 'LocalDateTime(\"1979-05-27T07:32:00\"), owner: {name: \"Ada\"}, point: {x: 1, y: 2}, products: [{name: \"one\"}, {name: \"two\"}], title: \"Telora\", when: 'OffsetDateTime(\"1979-05-27T07:32:00Z\")}"
        );
    }

    #[test]
    fn rejects_invalid_dates_and_duplicate_keys() {
        let date = parse("when = 2025-02-29\n");
        assert!(date.value.is_none());
        assert!(date.diagnostics[0].message.contains("day"));

        let duplicate = parse("a = 1\na = 2\n");
        assert!(duplicate.value.is_none());
        assert_eq!(duplicate.diagnostics[0].labels.len(), 2);
    }

    #[test]
    fn decodes_toml_strings_numbers_and_rejects_table_conflicts() {
        let parsed = parse(
            "escaped = \"line\\n\\u5F62\"\nfolded = \"\"\"\nfirst\\\n  second\"\"\"\nhex = 0xDEAD_BEEF\nfloat = 1_000.50\n",
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(
            parsed.value.unwrap().value.to_string(),
            "{escaped: \"line\\n形\", float: 1000.5, folded: \"firstsecond\", hex: 3735928559}"
        );

        for source in [
            "value = 1__0\n",
            "value = 01\n",
            "a = {b = 1}\na.c = 2\n",
            "a = 1\n[a]\nb = 2\n",
            "[a]\nb = 1\n[a]\nc = 2\n",
            "a = []\n[[a]]\nb = 1\n",
            "a.b = 1\n[a]\nc = 2\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.value.is_none(), "accepted invalid TOML: {source}");
            assert!(!parsed.diagnostics.is_empty(), "{source}");
        }

        let implicit_header = parse("[a.b]\nvalue = 1\n[a]\nname = \"ok\"\n");
        assert!(
            implicit_header.diagnostics.is_empty(),
            "{:?}",
            implicit_header.diagnostics
        );
    }

    #[test]
    fn covers_toml_1_0_string_and_numeric_boundaries() {
        let parsed = parse(
            "four = \"\"\"one\"\"\"\"\nfive = '''two'''''\r\nlines = \"\"\"a\r\nb\"\"\"\r\nempty = \"\"\nquoted.key = 1\n\"quoted.key\" = 2\n",
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(
            parsed.value.unwrap().value.to_string(),
            "{empty: \"\", five: \"two''\", four: \"one\\\"\", lines: \"a\\nb\", quoted: {key: 1}, quoted.key: 2}"
        );

        for source in [
            "value = +0x1\n",
            "value = -0o7\n",
            "value = 1.\n",
            "value = 1.e2\n",
            "value = 1e\n",
            "value = 1e+\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.value.is_none(), "accepted invalid TOML: {source}");
        }
    }

    #[test]
    fn rejects_non_finite_float_values() {
        for source in [
            "value = inf\n",
            "value = -inf\n",
            "value = nan\n",
            "value = 1.0e9999\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.value.is_none(), "accepted {source}");
            assert!(parsed.diagnostics[0].message.contains("must be finite"));
        }

        let overflow = parse("value = 9223372036854775808\n");
        assert!(overflow.value.is_none());
        assert!(
            overflow.diagnostics[0]
                .message
                .contains("outside the i64 range")
        );
    }
