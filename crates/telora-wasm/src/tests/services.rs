use crate::session::Session;

#[test]
fn collection_keeps_initialization_locations_without_registering_request_sources() {
    use telora_core::data_plan::{Format, parse_registered};
    let mir = super::graph(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/retained-source.telora"
        ))
        .expect("read test source"),
    );
    let symbol = *mir
        .exports
        .iter()
        .flatten()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let bytes = crate::compile_executable(&mir.seal_export(symbol).unwrap()).unwrap();
    let mut sources = mir.sources;
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    let baseline = session.manifest.sources.len();
    let retained = sources.try_add_data("retained.json", "42".into()).unwrap();
    let plan = parse_registered(&sources, retained, Format::Json).unwrap();
    session.register_data_sources(&sources, &plan).unwrap();
    let value = session.materialize_value(&plan, &sources).unwrap();
    session.initialize().unwrap();
    let factory = crate::transport::Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let mut closure = session.invoke_values(factory, &[value]).unwrap();
    let mut plateau = None;
    for n in 0..256 {
        let discarded = session.input_value(value.ty, &n.into()).unwrap();
        for offset in [0, 4, 8] {
            assert_eq!(session.output().word(discarded.pointer as u64 + offset).unwrap(), 0);
        }
        let (roots, stats) = session.collect_work(&[closure]).unwrap();
        closure = roots[0];
        assert_eq!(session.manifest.sources.len(), baseline + 1);
        assert!(
            session
                .manifest
                .sources
                .iter()
                .any(|s| s.id == retained.get())
        );
        if let Some(bytes) = plateau {
            assert_eq!(stats.heap_after, bytes);
        }
        plateau = Some(stats.heap_after);
    }
    assert!(session.invoke_values(closure, &[]).is_err());
    let diagnostics = session.diagnostics().unwrap();
    assert_eq!(diagnostics[0].message, "retained input");
    assert_eq!(diagnostics[0].subjects, vec![[retained.get(), 0, 0, 0, 2]]);
}

#[test]
fn collection_preserves_shared_graphs_resources_and_interpreter_cycles() {
    let bytes = super::compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/collection.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let factory = crate::transport::Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let argument_type = session.manifest.types[factory.ty as usize].arguments[0];
    let argument = session.input_value(argument_type, &42.into()).unwrap();
    let mut closure = session.invoke_values(factory, &[argument]).unwrap();
    let result = session.invoke_values(closure, &[]).unwrap();
    let expected = session.output_value(result).unwrap();
    assert_eq!(
        expected,
        serde_json::json!([
            true,
            true,
            true,
            true,
            "[a long shared string é🦀]",
            true,
            42,
            42,
            42,
            true
        ])
    );
    let mut plateau = None;
    for _ in 0..12 {
        let (roots, stats) = session.collect_work(&[closure]).unwrap();
        closure = roots[0];
        if let Some(end) = plateau {
            assert_eq!(stats.heap_after, end);
        }
        plateau = Some(stats.heap_after);
        let result = session.invoke_values(closure, &[]).unwrap();
        assert_eq!(session.output_value(result).unwrap(), expected);
    }
}
