use super::*;
use crate::data_packet::{DataPacket, Value};

#[test]
fn portable_data_roundtrip_preserves_integer_bits_aliases_and_rejects_bad_edges() {
    let mir = graph(include_str!("../../tests/fixtures/entry.telora"));
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let bytes = crate::compile_executable(&mir.seal_export(export).unwrap()).unwrap();
    let mut sources = mir.sources;
    let id = sources.add(
        "bundled.yaml",
        "number: 42\nmax: 9223372036854775807\nbase: &a [1, 2]\ncopy: *a\n",
    );
    let plan = telora_core::data_plan::parse_registered(
        &sources,
        id,
        telora_core::data_plan::Format::Yaml,
    )
    .unwrap();
    let packet = DataPacket::from_plan(&plan).unwrap();
    let serialized = serde_json::to_vec(&packet).unwrap();
    let packet: DataPacket = serde_json::from_slice(&serialized).unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.register_data_sources(&sources, &plan).unwrap();
    let symbol = session.manifest.data_modules[0].symbol;
    assert!(crate::bundle::build(&bytes, &sources, &[]).is_err());
    let bundled = crate::bundle::build(&bytes, &sources, &[(symbol, plan.clone())]).unwrap();
    assert!(crate::bundle::build(&bundled, &sources, &[(symbol, plan.clone())]).is_err());
    drop(plan);
    drop(sources);
    for bad in [u32::MAX, packet.root] {
        let mut invalid = packet.clone();
        invalid.nodes[invalid.root as usize].value = Value::Array(vec![bad]);
        assert!(session.inject_data_packet(symbol, &invalid).is_err());
    }
    let mut invalid = packet.clone();
    invalid.nodes[invalid.root as usize].origin[0] = u32::MAX;
    assert!(session.inject_data_packet(symbol, &invalid).is_err());
    session.inject_data_packet(symbol, &packet).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([
            {"number":42,"max":i64::MAX,"base":[1,2],"copy":[1,2]},42
        ])
    );
    drop(session);
    let mut loaded = crate::session::Session::load(&bundled, 2_000_000).unwrap();
    loaded.initialize().unwrap();
    assert_eq!(
        loaded.eval().unwrap(),
        serde_json::json!([
            {"number":42,"max":i64::MAX,"base":[1,2],"copy":[1,2]},42
        ])
    );
}
