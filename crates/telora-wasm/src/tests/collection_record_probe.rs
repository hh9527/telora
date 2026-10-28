//! Field-sensitive pure predicates over records with shared String/Array children.
use super::collection_alias::benchmark_mir;
use crate::{
    collection_summaries::scalar_callbacks_safe, gc::GcConfig, plan::Plan, session::Session,
    transport::Value,
};
use serde_json::{Value as Json, json};
fn artifact(mode: &str) -> Vec<u8> {
    let source = format!(
        "use std::pipeline as p;pub def answer:Fn(Array(p::AliasProbeRow))->(Array(p::AliasProbeRow),Array(p::AliasProbeRow))=fn(rows){{(rows,p::alias_filter_records_{mode}(rows))}};"
    );
    let mir = benchmark_mir(&source);
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|s| mir.symbols[s.index()].name == "answer")
        .unwrap();
    let executable = mir.seal_export(export).unwrap();
    let plan = Plan::new(&executable).unwrap();
    if mode != "conservative" {
        assert_eq!(
            scalar_callbacks_safe(&mir, &plan, "alias_filter_array_generic_checked"),
            mode == "checked"
        );
    }
    crate::compile_executable(&executable).unwrap()
}
fn run(bytes: &[u8], count: i32, gc: bool, debug: bool) -> Json {
    let rows: Vec<_> = (0..count)
        .map(|i| json!({"key":i,"label":format!("row-{i}"),"score":i+1,"trail":[i,i+100]}))
        .collect();
    let expected = json!([
        rows,
        rows.iter()
            .filter(|r| r["score"].as_i64().unwrap() % 3 != 0)
            .cloned()
            .collect::<Vec<_>>()
    ]);
    let mut s = Session::load_with_limits(bytes, 2_000_000_000, 256 * 1024 * 1024).unwrap();
    s.configure_gc(GcConfig {
        allocation_interval: if gc { 1 } else { 0 },
        heap_limit: 256 * 1024 * 1024,
    })
    .unwrap();
    s.set_debug_enabled(debug).unwrap();
    s.initialize().unwrap();
    let entry = Value {
        pointer: s.entry().unwrap(),
        ty: s.manifest.entry_type,
    };
    let input_type = s.manifest.types[entry.ty as usize].arguments[0];
    let arg = s.input_value(input_type, &json!(rows)).unwrap();
    let before = s.gc_stats().unwrap().allocated_total;
    let fuel = s.store.get_fuel().unwrap();
    let start = std::time::Instant::now();
    let output = s.invoke_values(entry, &[arg]).unwrap();
    let seconds = start.elapsed().as_secs_f64();
    let fuel = fuel - s.store.get_fuel().unwrap();
    let stats = s.gc_stats().unwrap();
    assert_eq!(s.output_value(output).unwrap(), expected);
    let pin = s.pin_value(output).unwrap();
    s.collect_garbage().unwrap();
    let retained = s.gc_stats().unwrap().managed_bytes;
    let relocated = s.pinned_value(pin).unwrap();
    assert_eq!(s.output_value(relocated).unwrap(), expected);
    if debug {
        let events = s.take_debug_events().unwrap();
        assert_eq!(events.len(), count as usize);
        for (i, event) in events.iter().enumerate() {
            assert!(event.repr.contains(&format!("row-{i}")), "{event:?}");
        }
    }
    json!({"seconds":seconds,"fuel":fuel,"allocated_bytes":stats.allocated_total-before,"managed_after_gc":retained,"linear_memory_bytes":stats.memory_bytes})
}
#[test]
fn record_predicate_elision_preserves_old_values_nested_children_and_gc() {
    for mode in ["standard", "conservative", "checked", "debug"] {
        let bytes = artifact(mode);
        for n in [0, 1, 2, 3, 17, 65] {
            run(&bytes, n, true, mode == "debug");
        }
    }
}
#[test]
#[ignore = "record predicate snapshot comparison; run explicitly"]
fn measure_record_predicate_elision() {
    let mut rows = vec![];
    for mode in ["standard", "conservative", "checked"] {
        let bytes = artifact(mode);
        for count in [128, 512, 2048] {
            let samples: Vec<_> = (0..3).map(|_| run(&bytes, count, false, false)).collect();
            let row = json!({"mode":mode,"count":count,"samples":samples});
            println!("{row}");
            rows.push(row);
            if let Some(path) = std::env::var_os("TELORA_RECORD_PROBE_REPORT") {
                std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
            }
        }
    }
}
