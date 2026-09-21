use super::*;
use telora_core::data_plan::Format;

#[test]
fn portable_data_roundtrip_preserves_original_text_and_integer_bits() {
    let mir = graph(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/entry.telora"
        ))
        .unwrap(),
    );
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let bytes = crate::compile_executable(&mir.seal_export(export).unwrap()).unwrap();
    let mut sources = mir.sources;
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/portable-values.yaml"
    ))
    .unwrap();
    let id = sources.try_add_data("bundled.yaml", text.clone()).unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    let symbol = session.manifest.data_modules[0].symbol;
    assert!(crate::bundle::build(&bytes, &sources, &[]).is_err());
    let modules = [(symbol, id, Format::Yaml)];
    let bundled = crate::bundle::build(&bytes, &sources, &modules).unwrap();
    assert!(crate::bundle::build(&bundled, &sources, &modules).is_err());
    let restored = crate::bundle::read(&bundled, &session.manifest).unwrap();
    assert_eq!(restored[0].text, text);
    let value = session
        .parse_data_source(sources.get(id), Format::Yaml)
        .unwrap()
        .unwrap();
    session.inject_data_value(symbol, value).unwrap();
    session.initialize().unwrap();
    let expected = serde_json::json!([
        {"number":42,"max":i64::MAX,"base":[1,2],"copy":[1,2]},42
    ]);
    assert_eq!(session.eval().unwrap(), expected);
    drop(session);
    drop(sources);
    let mut loaded = crate::session::Session::load(&bundled, 2_000_000).unwrap();
    loaded.initialize().unwrap();
    assert_eq!(loaded.eval().unwrap(), expected);
}
