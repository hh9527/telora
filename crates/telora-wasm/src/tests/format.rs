use super::*;

#[test]
fn format_nodes_and_interpolation_use_fixed_rust_rt_operations() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/format.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 20_000_000).unwrap();
    session.initialize().unwrap();
    let expected = serde_json::json!([
        "[é🦀:42]",
        "-9223372036854775808",
        "3",
        "0.00125",
        "-0",
        "",
        "[é🦀:42]/[é🦀:42]",
        "n=42, f=3, s=ready",
        "nested=yes",
        "",
        "unicode é🦀",
        "01234567890123456789",
        "wrapped=9"
    ]);
    assert_eq!(session.call(&[]).unwrap(), expected);
    assert_eq!(session.call(&[]).unwrap(), expected);
    assert!(session.diagnostics().unwrap().is_empty());

    let bytes = compile(include_str!("../../tests/fixtures/format-effects.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 20_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([
            "std/fmt.concat requires strings.len == items.len + 1, got 0 and 0",
            "std/fmt value exceeds the recursive rendering limit",
            "x",
            "7:ok"
        ])
    );
    assert_eq!(
        session
            .diagnostics()
            .unwrap()
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>(),
        ["left", "right"]
    );
}
