//! Cache is an immutable sharing boundary even when its downstream owns a builder.
use super::collection_alias::benchmark_artifact;
use crate::{gc::GcConfig, session::Session, transport::Value};
use serde_json::Value as Json;

fn transformed(xs: impl IntoIterator<Item = i32>) -> Vec<i32> {
    xs.into_iter()
        .map(|x| x + 1)
        .filter(|x| x % 3 != 0)
        .map(|x| x * 2)
        .collect()
}
fn source(mode: &str, write: bool) -> String {
    let node = if mode == "pipeline" {
        "p::from_array(xs)|>p::map(fn(x){x+1})|>p::filter(fn(x){x%3!=0})|>p::map(fn(x){x*2})"
    } else {
        "p::alias_fused_node(xs)"
    };
    let (result, body) = if write {
        (
            "(Array(Int),Array(Int))",
            "(initial,p::alias_owned_chain(initial))",
        )
    } else {
        ("Array(Int)", "initial")
    };
    format!(
        "use std::pipeline as p;pub def answer:Fn(Array(Int))->Fn()->{result}=fn(xs){{let cached=({node})|>p::cache();fn(){{let initial=p::run(cached|>p::collect());{body}}}}};"
    )
}
fn run(bytes: &[u8], count: i32, write: bool, force_gc: bool) -> Vec<Json> {
    let mut s = Session::load_with_limits(bytes, 2_000_000_000, 256 * 1024 * 1024).unwrap();
    s.configure_gc(GcConfig {
        allocation_interval: if force_gc { 1 } else { 0 },
        heap_limit: 256 * 1024 * 1024,
    })
    .unwrap();
    s.initialize().unwrap();
    let entry = Value {
        pointer: s.entry().unwrap(),
        ty: s.manifest.entry_type,
    };
    let input_ty = s.manifest.types[entry.ty as usize].arguments[0];
    let arg = s
        .input_value(input_ty, &serde_json::json!((0..count).collect::<Vec<_>>()))
        .unwrap();
    let setup_before = s.gc_stats().unwrap().allocated_total;
    let callback = s.invoke_values(entry, &[arg]).unwrap();
    let setup_bytes = s.gc_stats().unwrap().allocated_total - setup_before;
    let callback_pin = s.pin_value(callback).unwrap();
    let mut rows = vec![];
    let expected = transformed(0..count);
    let expected = if write {
        serde_json::json!([expected, transformed(expected.clone())])
    } else {
        serde_json::json!(expected)
    };
    for phase in ["cold", "warm1", "warm2"] {
        let callback = s.pinned_value(callback_pin).unwrap();
        let before = s.gc_stats().unwrap().allocated_total;
        let fuel = s.store.get_fuel().unwrap();
        let start = std::time::Instant::now();
        let output = s.invoke_values(callback, &[]).unwrap();
        let seconds = start.elapsed().as_secs_f64();
        let fuel = fuel - s.store.get_fuel().unwrap();
        let stats = s.gc_stats().unwrap();
        assert_eq!(s.output_value(output).unwrap(), expected, "{phase}");
        let pin = s.pin_value(output).unwrap();
        s.collect_garbage().unwrap();
        let retained = s.gc_stats().unwrap().managed_bytes;
        let output = s.pinned_value(pin).unwrap();
        assert_eq!(s.output_value(output).unwrap(), expected);
        s.unpin_value(pin).unwrap();
        rows.push(serde_json::json!({"phase":phase,"seconds":seconds,"fuel":fuel,"allocated_bytes":stats.allocated_total-before,"linear_memory_bytes":stats.memory_bytes,"managed_after_gc":retained,"setup_allocated_bytes":setup_bytes}));
    }
    rows
}
#[test]
fn collection_cache_snapshots_survive_repeated_owned_writes_and_gc() {
    for mode in ["pipeline", "fused"] {
        for write in [false, true] {
            let bytes = benchmark_artifact(&source(mode, write));
            for n in [0, 1, 2, 65] {
                run(&bytes, n, write, true);
            }
        }
    }
}
#[test]
#[ignore = "cache cold/warm and downstream copy-on-write boundary survey"]
fn measure_collection_cache_space() {
    let mut rows = vec![];
    for write in [false, true] {
        for mode in ["pipeline", "fused"] {
            let bytes = benchmark_artifact(&source(mode, write));
            for count in [128, 512, 2048] {
                let samples: Vec<_> = (0..3).map(|_| run(&bytes, count, write, false)).collect();
                let row = serde_json::json!({"case":if write{"cache_then_write"}else{"cache_read"},"mode":mode,"count":count,"artifact_bytes":bytes.len(),"samples":samples});
                println!("{row}");
                rows.push(row);
                if let Some(path) = std::env::var_os("TELORA_COLLECTION_CACHE_REPORT") {
                    std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
                }
            }
        }
    }
}
