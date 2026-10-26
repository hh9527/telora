use super::*;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .expect("read memo fixture")
}

#[test]
fn type_memorizer_hits_exact_identities_and_keeps_instances_isolated() {
    let bytes = compile(&fixture("type-memorizer.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 50_000_000).unwrap();
    session.initialize().unwrap();
    let first = session.call(&[0.into()]).unwrap();
    let cold = session.usage().fuel_budget - session.usage().fuel_remaining;
    assert!(first[0].as_str().unwrap().ends_with("::First"), "{first}");
    assert_eq!(first[1], "work=200");
    assert_eq!(session.call(&[0.into()]).unwrap(), first);
    let warm = session.usage().fuel_budget - session.usage().fuel_remaining;
    assert!(
        warm * 2 < cold,
        "cache must skip computation: cold={cold}, warm={warm}"
    );
    let second = session.call(&[1.into()]).unwrap();
    assert!(
        second[0].as_str().unwrap().ends_with("::Second"),
        "{second}"
    );
    assert_ne!(
        first, second,
        "nominal types sharing storage must not share a slot"
    );
    assert_eq!(session.call(&[0.into()]).unwrap(), first);
    session.collect_garbage().unwrap();
    assert_eq!(session.call(&[1.into()]).unwrap(), second);
    assert_eq!(session.call(&[0.into()]).unwrap(), first);
    let mut fresh = crate::session::Session::load(&bytes, 50_000_000).unwrap();
    fresh.initialize().unwrap();
    assert_eq!(fresh.call(&[0.into()]).unwrap(), first);
    let fresh_cold = fresh.usage().fuel_budget - fresh.usage().fuel_remaining;
    assert!(fresh_cold > warm * 2, "memo state must be session local");
}

#[test]
fn type_memorizer_slots_trace_cached_aggregates_under_aggressive_gc() {
    let bytes = compile(&fixture("type-memorizer.telora").replace("burn(200)", "burn(1)")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 200_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 32 * 1024 * 1024,
        })
        .unwrap();
    session.initialize().unwrap();
    let first = session.call(&[0.into()]).unwrap();
    let second = session.call(&[1.into()]).unwrap();
    session.collect_garbage().unwrap();
    assert_eq!(session.call(&[0.into()]).unwrap(), first);
    assert_eq!(session.call(&[1.into()]).unwrap(), second);
    assert!(session.gc_stats().unwrap().collections > 0);
}

#[test]
fn type_memorizer_cycles_and_failures_do_not_poison_future_calls() {
    let source = fixture("type-memorizer-failures.telora");
    let bytes = compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 50_000_000).unwrap();
    session.initialize().unwrap();
    let expected = serde_json::json!([
        "type memorizer computation depends on itself",
        "type memorizer computation depends on itself",
        "calculation failed",
        "calculation failed",
        "ok"
    ]);
    assert_eq!(session.call(&[]).unwrap(), expected);
    assert_eq!(session.call(&[]).unwrap(), expected);
}

#[test]
fn type_memorizer_survives_staged_snapshot_and_retains_the_fixed_computation() {
    let source = fixture("type-memorizer-staged.telora");
    let mir = graph(&source);
    let symbol = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let executable = mir.seal_export(symbol).unwrap();
    let bytes = crate::staged_build::StagedBuild::compile(&executable)
        .unwrap()
        .publish(100_000_000, 64 * 1024 * 1024)
        .unwrap();
    for _ in 0..2 {
        let mut session = crate::staged_build::load(&bytes, 100_000_000, 64 * 1024 * 1024).unwrap();
        assert_eq!(
            session.call(&[false.into()]).unwrap(),
            serde_json::json!("integer")
        );
        assert_eq!(
            session.call(&[true.into()]).unwrap(),
            serde_json::json!("other")
        );
        session.collect_garbage().unwrap();
        assert_eq!(
            session.call(&[true.into()]).unwrap(),
            serde_json::json!("other")
        );
    }
}

#[test]
fn type_memorizer_reflection_exposes_only_the_query_not_storage() {
    let bytes = compile(&fixture("type-memorizer-opacity.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 50_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([true, true, true, true])
    );
    session.collect_garbage().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([true, true, true, true])
    );
}
