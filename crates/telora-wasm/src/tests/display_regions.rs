use super::*;

fn source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/display-regions.telora"
    ))
    .unwrap()
}
#[test]
fn display_regions_certify_custom_and_property_formatter_chains() {
    let mir = graph(&source());
    let symbol = *mir
        .exports
        .iter()
        .flatten()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let executable = mir.seal_export(symbol).unwrap();
    let plan = crate::plan::Plan::new(&executable).unwrap();
    let custom = plan
        .format_regions
        .functions
        .keys()
        .filter(|k| mir.modules[mir.hir[k.node.index()].module.index()].name == "@src/main")
        .count();
    assert!(
        custom > 0,
        "no custom formatter certified: {:?}",
        plan.format_regions.functions.keys().collect::<Vec<_>>()
    );
    assert!(
        plan.format_regions.functions.len() > 10,
        "property closure chain not certified"
    );
    let bytes = crate::compile_executable(&executable).unwrap();
    let mut session = crate::session::Session::load(&bytes, 5_000_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 64 * 1024 * 1024,
        })
        .unwrap();
    session.initialize().unwrap();
    let table = crate::abi::table_address(crate::abi::FORMATS) as u64;
    let before = session.output().raw_word(table + 4).unwrap();
    assert_eq!(
        session.call(&[7.into(), "é🦀".into()]).unwrap(),
        serde_json::json!(["custom(é🦀:7)", "é🦀:7", "service=é🦀:7/0.125", "é🦀:7"])
    );
    assert_eq!(
        session.output().raw_word(table + 4).unwrap(),
        before,
        "formatter nodes were still materialized"
    );
}

#[test]
fn display_regions_preserve_versions_and_reject_observing_or_captured_accumulators() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/display-region-fallbacks.telora"
    ))
    .unwrap();
    let bytes = compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 5_000_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 32 * 1024 * 1024,
        })
        .unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        assert_eq!(
            session.call(&[]).unwrap(),
            serde_json::json!(["p1/p2", "equal", "discarded", "captured", "property"])
        );
        session.collect_garbage().unwrap();
    }
}
#[test]
fn display_regions_preserve_failure_debug_order_and_repeated_requests() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/display-region-effects.telora"
    ))
    .unwrap();
    let bytes = compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 5_000_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 32 * 1024 * 1024,
        })
        .unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    for _ in 0..4 {
        assert_eq!(
            session.call(&[false.into()]).unwrap(),
            serde_json::json!("[7]")
        );
        assert_eq!(
            session
                .take_debug_events()
                .unwrap()
                .iter()
                .map(|e| e.repr.clone())
                .collect::<Vec<_>>(),
            ["7"]
        );
        assert_eq!(
            session.call(&[true.into()]).unwrap(),
            serde_json::json!("failed")
        );
        assert!(session.take_debug_events().unwrap().is_empty());
        session.collect_garbage().unwrap();
    }
}
#[test]
fn display_regions_restore_property_closures_after_staged_build() {
    let source = source().replace("pub def answer:", "def inspect:")
        + r#"
use std::build as build;
pub def answer: Fn(Int,String) -> Array(String) = build::stage(
    fn() {{runtime: inspect,information:{}}}, fn(f) {f}
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
        .publish(200_000_000, 64 * 1024 * 1024)
        .unwrap();
    for _ in 0..2 {
        let mut s = crate::staged_build::load(&bytes, 200_000_000, 64 * 1024 * 1024).unwrap();
        s.collect_garbage().unwrap();
        assert_eq!(
            s.call(&[7.into(), "é🦀".into()]).unwrap(),
            serde_json::json!(["custom(é🦀:7)", "é🦀:7", "service=é🦀:7/0.125", "é🦀:7"])
        );
    }
}

#[test]
fn display_regions_fuse_codec_display_bridge() {
    let bytes = compile_export(&source(), "codec_answer").unwrap();
    let mut session = crate::session::Session::load(&bytes, 5_000_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 64 * 1024 * 1024,
        })
        .unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!("\"a:7\""));
}

#[test]
fn display_regions_fuse_custom_and_property_interpolation() {
    let bytes = compile_export(&source(), "interpolation_answer").unwrap();
    let mut session = crate::session::Session::load(&bytes, 5_000_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 64 * 1024 * 1024,
        })
        .unwrap();
    session.initialize().unwrap();
    let table = crate::abi::table_address(crate::abi::FORMATS) as u64;
    let before = session.output().raw_word(table + 4).unwrap();
    assert_eq!(
        session.call(&[7.into(), "é🦀".into()]).unwrap(),
        serde_json::json!("values=custom(é🦀:7)/é🦀:7")
    );
    assert_eq!(session.output().raw_word(table + 4).unwrap(), before);
}

#[test]
#[ignore = "explicit formatter region performance measurement"]
fn measure_display_regions() {
    use crate::{gc::GcConfig, session::Session, transport::Value};
    let mut rows = vec![];
    for (case, expression, expected) in [
        (
            "custom",
            "fmt::to_string({name:s,count:n}.ty!(Custom))",
            "custom(row:7)",
        ),
        (
            "property",
            "fmt::to_string({name:s,count:n}.ty!(Item))",
            "row:7",
        ),
        (
            "nested_property",
            "fmt::to_string({item:{name:s,count:n},ratio:0.125}.ty!(Service))",
            "service=row:7/0.125",
        ),
        (
            "interpolation",
            r#"`value=\{{name:s,count:n}.ty!(Item)}`"#,
            "value=row:7",
        ),
        (
            "codec",
            r#"json::stringify(codec::encode({name:s,count:n}.ty!(Item)))"#,
            "\"row:7\"",
        ),
    ] {
        let program = source()
            + &format!(
                "\nuse std::array as a;\npub def bench:Fn(Array(Int))->String=fn(xs){{string::join(a::map(xs,fn(n){{let s=\"row\";{expression}}}),\"\")}};"
            );
        let bytes = compile_export(&program, "bench").unwrap();
        let count = 256;
        let input = serde_json::json!(vec![7; count]);
        let mut samples = vec![];
        for _ in 0..3 {
            let mut session =
                Session::load_with_limits(&bytes, 10_000_000_000, 256 * 1024 * 1024).unwrap();
            session
                .configure_gc(GcConfig {
                    allocation_interval: 0,
                    heap_limit: 128 * 1024 * 1024,
                })
                .unwrap();
            session.initialize().unwrap();
            let entry = Value {
                pointer: session.entry().unwrap(),
                ty: session.manifest.entry_type,
            };
            let argument_type = session.manifest.types[entry.ty as usize].arguments[0];
            let input = session.input_value(argument_type, &input).unwrap();
            let before = session.gc_stats().unwrap().allocated_total;
            let fuel = session.store.get_fuel().unwrap();
            let start = std::time::Instant::now();
            let result = session.invoke_values(entry, &[input]).unwrap();
            let seconds = start.elapsed().as_secs_f64();
            let fuel = fuel - session.store.get_fuel().unwrap();
            let stats = session.gc_stats().unwrap();
            assert_eq!(
                session.output_value(result).unwrap(),
                serde_json::json!(expected.repeat(count))
            );
            samples.push(serde_json::json!({"seconds":seconds,"fuel":fuel,"managed_allocated_bytes":stats.allocated_total-before,"linear_memory_bytes":stats.memory_bytes}));
        }
        let row = serde_json::json!({"case":case,"count":count,"artifact_bytes":bytes.len(),"samples":samples});
        println!("{row}");
        rows.push(row);
        if let Some(path) = std::env::var_os("TELORA_DISPLAY_REGION_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
        }
    }
}
