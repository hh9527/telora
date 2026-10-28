use super::*;

#[test]
fn private_template_primitive_uses_the_admitted_native_identity() {
    // Select the private declaration directly as a sealed test root, without
    // exposing it through std/fmt's public exports or changing module privacy.
    let mir = graph("use std::fmt as fmt; pub def answer: Int = 0;");
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
                        telora_core::syntax::kinds::BindingKind::Native
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
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
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

#[test]
fn format_accumulator_preserves_versions_long_chains_and_gc_edges() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/format-accumulator.telora"
    ))
    .unwrap();
    let bytes = compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 128,
            heap_limit: 64 * 1024 * 1024,
        })
        .unwrap();
    session.initialize().unwrap();
    let xs: Vec<i64> = (0..2048).map(|x| x % 10).collect();
    let expected = format!("p:{}", "0123456789".repeat(204) + "01234567");
    for text in ["", "é🦀\n\t\r\0\u{001f}\"\\"] {
        let result = session
            .call(&[serde_json::json!(xs), serde_json::json!(text)])
            .unwrap();
        assert_eq!(
            result,
            serde_json::json!([
                [
                    "prefix7-0\"é\\n\\\"\"/0.125",
                    "p:",
                    expected,
                    expected,
                    "p:fork",
                    serde_json::to_string(text).unwrap()
                ],
                [true, false, false, true, true]
            ])
        );
        session.collect_garbage().unwrap();
    }
}

#[test]
fn staged_fmt_accumulator_restores_every_operand_layout() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/format-accumulator.telora"
    ))
    .unwrap();
    let source = source.replace("pub def answer:", "def inspect:")
        + r#"
use std::build as build;
pub def answer: Fn() -> String = build::stage(
    fn() { {runtime: initial, information: {}} },
    fn(f) { fn() { fmt::render(f) } }
);
"#;
    let mir = graph(&source);
    let symbol = *mir
        .exports
        .iter()
        .flatten()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let bytes = crate::staged_build::StagedBuild::compile(&mir.seal_export(symbol).unwrap())
        .unwrap()
        .publish(100_000_000, 64 * 1024 * 1024)
        .unwrap();
    let mut session = crate::staged_build::load(&bytes, 100_000_000, 64 * 1024 * 1024).unwrap();
    session.collect_garbage().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!("prefix7-0\"é\\n\\\"\"/0.125")
    );
}

#[test]
fn format_float_abi_checks_json_without_restricting_display() {
    // Language arithmetic already rejects nonfinite results. Inject values at
    // the native ABI boundary to exercise the JSON writer's defensive guard.
    for json in [false, true] {
        let operation = if json {
            "encode_json_float"
        } else {
            "write_float"
        };
        let bytes = compile(&format!("use std::fmt as fmt; pub def answer: Fn(Float) -> String = fn(v) {{fmt::render(fmt::{operation}(fmt::empty(),v))}};")).unwrap();
        for (number, text) in [
            (f64::NAN, "NaN"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
        ] {
            let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
            session.initialize().unwrap();
            let entry = crate::transport::Value {
                pointer: session.entry().unwrap(),
                ty: session.manifest.entry_type,
            };
            let ty = session.manifest.types[entry.ty as usize].arguments[0];
            let input = session.input_value(ty, &serde_json::json!(0.0)).unwrap();
            session
                .memory
                .write(
                    &mut session.store,
                    input.pointer as usize + crate::abi::DATA as usize,
                    &number.to_le_bytes(),
                )
                .unwrap();
            let result = session.invoke_values(entry, &[input]);
            if json {
                assert!(
                    result
                        .unwrap_err()
                        .contains("JSON cannot encode a non-finite Float")
                );
            } else {
                assert_eq!(
                    session.output_value(result.unwrap()).unwrap(),
                    serde_json::json!(text)
                );
            }
        }
    }
}

#[test]
fn format_float_writes_compose_inside_boolean_conditions() {
    for expression in [
        "fmt::render(fmt::write_float(fmt::empty(), -0.0)) == \"-0\"",
        "fmt::render(fmt::encode_json_float(fmt::empty(), 0.125)) == \"0.125\"",
        "fmt::render(fmt::write_float(fmt::empty(), -0.0)) == \"-0\" && fmt::render(fmt::encode_json_float(fmt::empty(), 0.125)) == \"0.125\"",
    ] {
        let bytes = compile(&format!(
            "use std::fmt as fmt; pub def answer: Fn() -> Bool = fn() {{{expression}}};"
        ))
        .unwrap();
        let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
        session.initialize().unwrap();
        let result = session.call(&[]);
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{expression}: {e}")),
            serde_json::json!(true)
        );
    }
}
