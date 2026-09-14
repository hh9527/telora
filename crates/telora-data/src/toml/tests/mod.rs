use super::*;
use alloc::vec::Vec;

fn parse(source: &str) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let mut sources = SourceDatabase::default();
    let id = sources.add("test.toml", source);
    validate_toml_registered(&sources, id)
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
    assert!(parsed.is_ok(), "{parsed:?}");
    assert_eq!(
        crate::data_plan_test::render(&parsed.unwrap()),
        "{dates: ['LocalDate(\"1979-05-27\"), 'LocalTime(\"07:32:00.1200\")], local: 'LocalDateTime(\"1979-05-27T07:32:00\"), owner: {name: \"Ada\"}, point: {x: 1, y: 2}, products: [{name: \"one\"}, {name: \"two\"}], title: \"Telora\", when: 'OffsetDateTime(\"1979-05-27T07:32:00Z\")}"
    );
}

#[test]
fn rejects_invalid_dates_and_duplicate_keys() {
    let date = parse("when = 2025-02-29\n");
    assert!(date.is_err());
    assert!(date.as_ref().unwrap_err()[0].message.contains("day"));

    let duplicate = parse("a = 1\na = 2\n");
    assert!(duplicate.is_err());
    assert_eq!(duplicate.as_ref().unwrap_err()[0].labels.len(), 2);
}

#[test]
fn decodes_toml_strings_numbers_and_rejects_table_conflicts() {
    let parsed = parse(
        "escaped = \"line\\n\\u5F62\"\nfolded = \"\"\"\nfirst\\\n  second\"\"\"\nhex = 0xDEAD_BEEF\nfloat = 1_000.50\n",
    );
    assert!(parsed.is_ok(), "{parsed:?}");
    assert_eq!(
        crate::data_plan_test::render(&parsed.unwrap()),
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
        assert!(parsed.is_err(), "accepted invalid TOML: {source}");
        assert!(!parsed.as_ref().unwrap_err().is_empty(), "{source}");
    }

    let implicit_header = parse("[a.b]\nvalue = 1\n[a]\nname = \"ok\"\n");
    assert!(implicit_header.is_ok(), "{:?}", implicit_header);
}

#[test]
fn covers_toml_1_0_string_and_numeric_boundaries() {
    let parsed = parse(
        "four = \"\"\"one\"\"\"\"\nfive = '''two'''''\r\nlines = \"\"\"a\r\nb\"\"\"\r\nempty = \"\"\nquoted.key = 1\n\"quoted.key\" = 2\n",
    );
    assert!(parsed.is_ok(), "{parsed:?}");
    assert_eq!(
        crate::data_plan_test::render(&parsed.unwrap()),
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
        assert!(parsed.is_err(), "accepted invalid TOML: {source}");
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
        assert!(parsed.is_err(), "accepted {source}");
        assert!(
            parsed.as_ref().unwrap_err()[0]
                .message
                .contains("must be finite")
        );
    }

    let overflow = parse("value = 9223372036854775808\n");
    assert!(overflow.is_err());
    assert!(
        overflow.as_ref().unwrap_err()[0]
            .message
            .contains("outside the i64 range")
    );
}
