use super::*;
use crate::data_packet::{DataPacket, Value};

#[test]
fn portable_data_roundtrip_preserves_integer_bits_and_rejects_bad_edges() {
    let mir = graph(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/entry.telora"
        ))
        .expect("read test source"),
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
    let id = sources.try_add_data(
        "bundled.yaml",
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/portable-values.yaml")).unwrap(),
    ).unwrap();
    let plan = telora_core::data_plan::parse_registered(
        &sources,
        id,
        telora_core::data_plan::Format::Yaml,
    )
    .unwrap();
    let packet = DataPacket::from_plan(&plan, &sources).unwrap();
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
#[test]
fn data_packet_coordinates_are_identical_across_line_endings() {
    let text = "{\n  \"é\": [\n    42\n  ]\n}\n";
    let mut packets = vec![];
    for eol in ["\n", "\r\n", "\r"] {
        let mut sources = telora_core::SourceDatabase::default();
        let id = sources.try_add_data("data.json", text.replace('\n', eol)).unwrap();
        let plan = telora_core::data_plan::parse_registered(&sources, id, telora_core::data_plan::Format::Json).unwrap();
        packets.push(serde_json::to_value(crate::data_packet::DataPacket::from_plan(&plan, &sources).unwrap()).unwrap());
    }
    assert_eq!(packets[0], packets[1]);
    assert_eq!(packets[0], packets[2]);
}
