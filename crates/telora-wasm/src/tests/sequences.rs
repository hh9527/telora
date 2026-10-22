#[test]
fn ordinary_generic_plans_preserve_types_through_functions_and_branches() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-plans.telora"
    ))
    .unwrap();
    let mir = super::graph(&source);
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    mir.seal_export(export).unwrap();
}

#[test]
fn fused_sequence_collects_and_counts_in_one_traversal() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-plans.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 20_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.eval().unwrap(), serde_json::json!([[3, 4], 2]));
}

#[test]
fn sequences_preserve_immutable_values_and_all_consumer_results() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-semantics.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 50_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([
            [6, 8],
            [9, 12],
            2,
            10,
            [9, 1, 2, 3, 4],
            true,
            true,
            3,
            null,
            [[10, 20, 30, 40], [[3, 4], [11, 12]]],
            [1, 2, 3, 4],
            0,
            true,
            []
        ])
    );
}

#[test]
fn shared_prefix_has_ordered_diagnostics_independent_stops_and_immediate_failure() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-order.telora"
    ))
    .unwrap();
    for (export, expected) in [
        (
            "answer",
            serde_json::json!([
                [1, true],
                [
                    "source", "left", "right", "source", "right", "source", "right"
                ]
            ]),
        ),
        (
            "failed",
            serde_json::json!(["source", "left", "right", "source", "left", "stop"]),
        ),
    ] {
        let bytes = super::compile_export(&source, export).unwrap();
        let mut session = crate::session::Session::load(&bytes, 50_000_000).unwrap();
        session.initialize().unwrap();
        assert_eq!(session.eval().unwrap(), expected, "{export}");
    }
}

#[test]
fn scalar_consumers_do_not_allocate_per_element_framework_objects() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-allocation.telora"
    ))
    .unwrap();
    for export in ["answer", "single", "separate", "eager"] {
        let bytes = super::compile_export(&source, export).unwrap();
        let mut measured = Vec::new();
        let mut scratch = Vec::new();
        for n in [16, 256, 1024] {
            let mut session = crate::session::Session::load(&bytes, 200_000_000).unwrap();
            session.initialize().unwrap();
            let input = serde_json::json!((0..n).collect::<Vec<_>>());
            let callable = crate::transport::Value {
                pointer: session.entry().unwrap(),
                ty: session.manifest.entry_type,
            };
            let input_ty = session.manifest.types[callable.ty as usize].arguments[0];
            let input = session.input_value(input_ty, &input).unwrap();
            let input_pin = session.pin_value(input).unwrap();
            let callable_pin = session.pin_value(callable).unwrap();
            let before = session.gc_stats().unwrap().allocated_total;
            let result = session.invoke_values(callable, &[input]).unwrap();
            let result = session.output_value(result).unwrap();
            session.unpin_value(input_pin).unwrap();
            session.unpin_value(callable_pin).unwrap();
            let after = session.gc_stats().unwrap().allocated_total;
            let expected = if export == "single" {
                serde_json::json!(n)
            } else {
                serde_json::json!([n, n * (n + 1) / 2])
            };
            assert_eq!(result, expected);
            measured.push(after - before);
            let metric = session
                .instance
                .get_typed_func::<(), u32>(&session.store, "telora_frame_bytes")
                .unwrap();
            scratch.push(session.guest_call(metric, ()).unwrap());
        }
        eprintln!("{export} execution allocations: {measured:?}");
        eprintln!("{export} retained scratch capacity: {scratch:?}");
        if export != "eager" {
            assert_eq!(
                measured[2], measured[0],
                "framework allocation grows: {measured:?}"
            );
            assert_eq!(scratch[2], scratch[0], "scratch grows: {scratch:?}");
        }
    }
}

#[test]
fn forced_gc_keeps_captures_partial_outputs_and_completed_branch_answers() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-gc.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let items = serde_json::json!((0..12).map(|i| serde_json::json!({"text": format!("a long source string needing content storage {i}"), "children": [format!("a long child string {i}")], "labels": {"key": format!("a long dictionary value {i}")}})).collect::<Vec<_>>());
    let evaluate = |interval| {
        let mut session = crate::session::Session::load(&bytes, 500_000_000).unwrap();
        session
            .configure_gc(crate::gc::GcConfig {
                allocation_interval: interval,
                ..Default::default()
            })
            .unwrap();
        session.initialize().unwrap();
        session.call(&[items.clone()]).unwrap()
    };
    assert_eq!(evaluate(0), evaluate(1));
}

#[test]
fn manually_forged_plan_contracts_are_rejected_before_emitting_wasm() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-invalid.telora"
    ))
    .unwrap();
    for export in ["wrong_source", "wrong_result", "wrong_identity"] {
        let error = super::compile_export(&source, export).unwrap_err();
        assert!(
            error.contains("sequence") || error.contains("std::sink"),
            "{export}: {error}"
        );
    }
}

#[test]
fn sequence_execution_and_consumer_callbacks_are_fuel_bounded() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-allocation.telora"
    ))
    .unwrap();
    for export in ["answer", "looping_callback"] {
        let bytes = super::compile_export(&source, export).unwrap();
        let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
        session.initialize().unwrap();
        let callable = crate::transport::Value {
            pointer: session.entry().unwrap(),
            ty: session.manifest.entry_type,
        };
        let input_ty = session.manifest.types[callable.ty as usize].arguments[0];
        let input = session
            .input_value(input_ty, &serde_json::json!((0..256).collect::<Vec<_>>()))
            .unwrap();
        session.fuel_budget = 1_000;
        session.store.set_fuel(1_000).unwrap();
        assert!(
            session
                .invoke_values(callable, &[input])
                .unwrap_err()
                .contains("fuel"),
            "{export}"
        );
        session.set_request_fuel(10_000_000);
        assert!(
            session
                .call(&[serde_json::json!([])])
                .unwrap_err()
                .contains("terminated after trap")
        );
    }
}

#[test]
fn growing_sequence_outputs_respect_the_managed_heap_limit() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-allocation.telora"
    ))
    .unwrap();
    let bytes = super::compile_export(&source, "growing_output").unwrap();
    let mut session = crate::session::Session::load(&bytes, 200_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            heap_limit: 256 * 1024,
            ..Default::default()
        })
        .unwrap();
    session.initialize().unwrap();
    let error = session
        .call(&[serde_json::json!((0..256).collect::<Vec<_>>())])
        .unwrap_err();
    assert!(
        error.contains("unreachable") || error.contains("growth") || error.contains("memory"),
        "{error}"
    );
    assert!(
        session
            .call(&[serde_json::json!([])])
            .unwrap_err()
            .contains("terminated after trap")
    );
}

#[test]
fn stored_plans_survive_service_compaction_snapshot_reset_and_traps() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-service.telora"
    ))
    .unwrap();
    let mir = super::graph(&source);
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let bytes = crate::compile_service(&mir.seal_export(export).unwrap()).unwrap();
    let mut session = crate::session::Session::load(&bytes, 200_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            ..Default::default()
        })
        .unwrap();
    let mut service = crate::transform_service::TransformSession::new(session).unwrap();
    assert!(service.initialize(&[]).unwrap().success);
    service.seal_initialization().unwrap();
    for _ in 0..2 {
        service.reset().unwrap();
        let output = service.transform(b"1").unwrap();
        let output: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(output["ok"], serde_json::json!([[11, 12, 13], 3]));
    }
    service.session_mut().fuel_budget = 10_000;
    service.session_mut().store.set_fuel(10_000).unwrap();
    assert!(service.transform(b"null").unwrap_err().contains("fuel"));
    service.session_mut().fuel_budget = 200_000_000;
    service.reset().unwrap();
    let output: serde_json::Value =
        serde_json::from_slice(&service.transform(b"1").unwrap()).unwrap();
    assert_eq!(output["ok"], serde_json::json!([[11, 12, 13], 3]));
}

#[test]
fn plans_are_ordinary_values_and_fold_reuse_preserves_the_initial_value() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/seq-values.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            ..Default::default()
        })
        .unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([
            [9],
            [9, 1, 2, 3],
            [9, 1, 2, 3],
            [1, 2, 3],
            [11, 12, 13],
            ["plan"]
        ])
    );
}
