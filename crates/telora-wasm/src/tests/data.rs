use super::*;
use telora_core::data_plan::Format;

#[test]
fn compiled_data_roundtrip_preserves_values_without_source_text() {
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
    let text = format!("# SOURCE_ONLY_COMMENT_230\n{text}");
    let id = sources.try_add_data("bundled.yaml", text.clone()).unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    let symbol = session.manifest.data_modules[0].symbol;
    assert!(crate::bundle::build(&bytes, &sources, &[]).is_err());
    let modules = [(symbol, id, Format::Yaml)];
    let bundled = crate::bundle::build(&bytes, &sources, &modules).unwrap();
    assert!(crate::bundle::build(&bundled, &sources, &modules).is_err());
    let image_start = wasmparser::Parser::new(0)
        .parse_all(&bundled)
        .find_map(|payload| {
            if let wasmparser::Payload::DataSection(reader) = payload.unwrap() {
                reader.into_iter().find_map(|segment| {
                    let segment = segment.unwrap();
                    matches!(segment.kind, wasmparser::DataKind::Passive)
                        .then_some(segment.range.end as usize - segment.data.len())
                })
            } else {
                None
            }
        })
        .unwrap();
    let mut damaged = bundled.clone();
    damaged[image_start] ^= 1;
    assert!(crate::session::Session::load(&damaged, 2_000_000).is_err());
    assert!(
        !bundled
            .windows(b"SOURCE_ONLY_COMMENT_230".len())
            .any(|bytes| bytes == b"SOURCE_ONLY_COMMENT_230")
    );
    assert!(
        !bundled
            .windows(text.len())
            .any(|bytes| bytes == text.as_bytes())
    );
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

#[test]
fn large_tooling_metadata_is_not_published_or_subject_to_data_admission() {
    let bytes = super::compile("pub def answer: Int = 42;").unwrap();
    let mut manifest = crate::artifact::Manifest::read(&bytes).unwrap();
    let primitive = manifest
        .types
        .iter()
        .find(|ty| ty.kind == crate::artifact::Kind::Int)
        .unwrap()
        .clone();
    manifest
        .types
        .extend(std::iter::repeat_n(primitive, 150_000));
    let metadata = serde_json::to_vec(&manifest).unwrap();
    assert!(telora_data::json_serde::from_slice::<crate::artifact::Manifest>(&metadata).is_err());
    let mut parts = crate::template::Parts::read(&bytes).unwrap();
    for (name, data) in &mut parts.custom {
        if name == "telora.tooling" {
            *data = metadata.clone();
        }
    }
    let bytes = parts.module();
    assert_eq!(
        crate::artifact::Manifest::read(&bytes).unwrap().types.len(),
        manifest.types.len()
    );
    let published =
        crate::publication::finish(&bytes, 64 * 1024 * 1024, 100_000_000, 100_000_000).unwrap();
    for payload in wasmparser::Parser::new(0).parse_all(&published) {
        if let wasmparser::Payload::CustomSection(section) = payload.unwrap() {
            assert_ne!(section.name(), "telora.tooling");
            if section.name() == "telora.manifest" {
                assert!(section.data().len() < 32);
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(section.data()).unwrap(),
                    serde_json::json!({"abi": crate::abi::VERSION})
                );
            }
        }
    }
}
