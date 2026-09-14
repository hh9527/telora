use super::*;

#[test]
fn private_template_primitive_uses_the_admitted_native_identity() {
    // Select the private declaration directly as a sealed test root, without
    // exposing it through std/fmt's public exports or changing module privacy.
    let mir = graph("import \"std/fmt\" as fmt; export def answer = 0;");
    let symbol = mir
        .symbols
        .iter()
        .find(|symbol| {
            symbol.name == "prepare"
                && symbol
                    .module
                    .is_some_and(|id| mir.modules[id.index()].name == "std/fmt")
                && matches!(
                    symbol.kind,
                    telora_core::mir::SymbolKind::Declaration(
                        telora_core::ast::BindingKind::Native
                    )
                )
        })
        .unwrap();
    let symbol = mir.hir_symbols[symbol.declarations.last().unwrap().index()].unwrap();
    let executable = mir.seal_export(symbol).unwrap();
    let bytes = crate::compile_executable(&executable).unwrap();
    let mut session = crate::session::Session::load(&bytes, 5_000_000).unwrap();
    session.initialize().unwrap();
    for (source, expected) in [
        ("", serde_json::json!([[""], []])),
        ("é🦀", serde_json::json!([["é🦀"], []])),
        ("{{a}}", serde_json::json!([["{a}"], []])),
        ("{{{a}}}", serde_json::json!([["{", "}"], ["a"]])),
        (
            "é{a}{_b2}🦀{a}",
            serde_json::json!([["é", "", "🦀", ""], ["a", "_b2", "a"]]),
        ),
        ("{{}}{{}}", serde_json::json!([["{}{}"], []])),
    ] {
        assert_eq!(
            session.call(&[serde_json::json!(source)]).unwrap(),
            expected,
            "{source}"
        );
    }
    for (source, expected) in [
        ("{", "unclosed Display template field"),
        ("{a{b}", "nested '{' in Display template field"),
        ("}", "unmatched '}' in Display template"),
        ("{}", "invalid Display template field \"\""),
        ("{1a}", "invalid Display template field \"1a\""),
        ("{a b}", "invalid Display template field \"a b\""),
        ("{é}", "invalid Display template field \"é\""),
    ] {
        let mut session = crate::session::Session::load(&bytes, 5_000_000).unwrap();
        session.initialize().unwrap();
        assert!(
            session
                .call(&[serde_json::json!(source)])
                .unwrap_err()
                .contains(expected)
        );
        let reports = session.diagnostics().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].message, expected);
    }
}

#[test]
fn format_nodes_and_interpolation_use_fixed_rust_rt_operations() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/format.telora"
        ))
        .expect("read test source"),
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

    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/format-effects.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
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
