use crate::{session::Session, transport::Value};

#[test]
fn decoder_array_diagnostic_paths_survive_collection_before_child_handoff() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-codec-array-path.telora"
    ))
    .unwrap();
    for export in ["answer", "rejected"] {
        let bytes = super::compile_export(&source, export).unwrap();
        let evaluate = |interval| {
            let mut session = Session::load(&bytes, 100_000_000).unwrap();
            session
                .configure_gc(crate::gc::GcConfig {
                    allocation_interval: interval,
                    ..Default::default()
                })
                .unwrap();
            session.initialize().unwrap();
            let result = session.call(&[]);
            if export == "answer" {
                assert_eq!(
                    result.unwrap(),
                    serde_json::json!({"field_roles": [
                        {"title": "first long title"}, {"title": "second long title"}
                    ]})
                );
            } else {
                let error = result.unwrap_err();
                assert!(error.contains("$.field_roles[0].title"), "{error}");
                assert!(error.contains("expected String"), "{error}");
            }
        };
        evaluate(0);
        evaluate(1);
    }
}

#[test]
fn host_pins_follow_collection_and_reject_stale_or_foreign_capabilities() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-root-lifetimes.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    let entry_type = session.manifest.entry_type;
    let input_type = session.manifest.types[entry_type as usize].arguments[0];
    let input_json = serde_json::json!([
        {"name": "one".repeat(100), "children": ["first child".repeat(100), "second child".repeat(100)]},
        {"name": "two".repeat(100), "children": ["third child".repeat(100)]}
    ]);
    let input = session.input_value(input_type, &input_json).unwrap();
    let input_pin = session.pin_value(input).unwrap();
    let audit = session
        .instance
        .get_typed_func::<i32, i32>(&session.store, "telora_roots_audit")
        .unwrap();
    audit.call(&mut session.store, 1).unwrap();
    session.initialize().unwrap();
    let input = session.pinned_value(input_pin).unwrap();
    assert_eq!(session.output_value(input).unwrap(), input_json);
    let closure = Value {
        pointer: session.entry().unwrap(),
        ty: entry_type,
    };
    let result = session.invoke_values(closure, &[input]).unwrap();
    let expected = session.output_value(result).unwrap();
    assert_eq!(expected[0]["name"], "one".repeat(100) + "!");
    assert_eq!(expected[1]["children"][0], "third child".repeat(100) + "?");
    let result_pin = session.pin_value(result).unwrap();
    for _ in 0..4 {
        session.collect_work(&[]).unwrap();
        let input = session.pinned_value(input_pin).unwrap();
        let result = session.pinned_value(result_pin).unwrap();
        assert_eq!(session.output_value(input).unwrap(), input_json);
        assert_eq!(session.output_value(result).unwrap(), expected);
    }
    session.unpin_value(input_pin).unwrap();
    assert!(
        session
            .pinned_value(input_pin)
            .unwrap_err()
            .contains("stale")
    );
    assert!(
        session
            .unpin_value(input_pin)
            .unwrap_err()
            .contains("stale")
    );
    let result = session.pinned_value(result_pin).unwrap();
    let replacement = session.pin_value(result).unwrap();
    assert!(
        session
            .pinned_value(input_pin)
            .unwrap_err()
            .contains("stale")
    );
    let mut foreign = Session::load(&bytes, 100_000_000).unwrap();
    assert!(
        foreign
            .pinned_value(result_pin)
            .unwrap_err()
            .contains("another session")
    );
    session.unpin_value(result_pin).unwrap();
    session.unpin_value(replacement).unwrap();
    session.collect_work(&[]).unwrap();
    let depth = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_roots_depth")
        .unwrap();
    let count = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_roots_count")
        .unwrap();
    assert_eq!(depth.call(&mut session.store, ()).unwrap(), 0);
    assert_eq!(count.call(&mut session.store, ()).unwrap(), 0);
}

#[test]
fn nonmoving_sweep_reuses_pages_and_content_with_a_fixed_pinned_graph() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-root-lifetimes.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 200_000_000).unwrap();
    session.initialize().unwrap();
    let entry_type = session.manifest.entry_type;
    let input_type = session.manifest.types[entry_type as usize].arguments[0];
    let expected = serde_json::json!([
        {"name": "live".repeat(100), "children": ["child".repeat(100)]}
    ]);
    let input = session.input_value(input_type, &expected).unwrap();
    let pin = session.pin_value(input).unwrap();
    let collect = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_gc_collect")
        .unwrap();
    let committed = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_heap_committed")
        .unwrap();
    let used = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_heap_bytes")
        .unwrap();
    let mut plateau = None;
    for round in 0..12 {
        for _ in 0..32 {
            session.input_value(input_type, &expected).unwrap();
        }
        collect.call(&mut session.store, ()).unwrap();
        let current = session.pinned_value(pin).unwrap();
        assert_eq!(
            current.pointer, input.pointer,
            "sweep moved the pinned value"
        );
        assert_eq!(session.output_value(current).unwrap(), expected);
        let metrics = (
            committed.call(&mut session.store, ()).unwrap(),
            used.call(&mut session.store, ()).unwrap(),
        );
        if round >= 4 {
            assert_eq!(*plateau.get_or_insert(metrics), metrics);
        }
    }
    let before = used.call(&mut session.store, ()).unwrap();
    session.unpin_value(pin).unwrap();
    collect.call(&mut session.store, ()).unwrap();
    assert!(used.call(&mut session.store, ()).unwrap() < before);
}

#[test]
fn nonmoving_sweep_retains_captured_resources_and_reuses_vacant_table_slots() {
    use crate::abi::*;
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-resources.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 500_000_000).unwrap();
    session.initialize().unwrap();
    let entry = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let string = session.manifest.types[entry.ty as usize].arguments[0];
    let make = |session: &mut Session| {
        let input = session.input_value(string, &"^live$".into()).unwrap();
        session.invoke_values(entry, &[input]).unwrap()
    };
    make(&mut session); // Leave a hole before the retained resource IDs.
    let retained = make(&mut session);
    let pin = session.pin_value(retained).unwrap();
    let collect = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_gc_collect")
        .unwrap();
    for _ in 0..12 {
        for _ in 0..8 {
            make(&mut session);
        }
        collect.call(&mut session.store, ()).unwrap();
        for tag in [REGEXES, FORMATS] {
            assert_eq!(
                session
                    .output()
                    .word(u64::from(table_address(tag) + 4))
                    .unwrap(),
                2
            );
        }
        let callback = session.pinned_value(pin).unwrap();
        let argument = session.input_value(string, &"live".into()).unwrap();
        let result = session.invoke_values(callback, &[argument]).unwrap();
        assert_eq!(session.output_value(result).unwrap(), "42");
    }
    session.unpin_value(pin).unwrap();
    collect.call(&mut session.store, ()).unwrap();
    for tag in [REGEXES, FORMATS] {
        assert_eq!(
            session
                .output()
                .word(u64::from(table_address(tag) + 4))
                .unwrap(),
            0
        );
    }
}

#[test]
fn execution_safepoints_collect_inside_one_invocation() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-execution.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 500_000_000).unwrap();
    session.initialize().unwrap();
    let configure = session
        .instance
        .get_typed_func::<i32, i32>(&session.store, "telora_gc_interval")
        .unwrap();
    configure.call(&mut session.store, 1).unwrap();
    let result = session.call(&[50.into()]).unwrap();
    assert_eq!(
        result,
        serde_json::json!({
            "name": "a repeated live name that needs non-inline content",
            "children": ["first child needing a content slice", "second child needing a content slice"]
        })
    );
    let collections = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_gc_collections")
        .unwrap();
    assert!(collections.call(&mut session.store, ()).unwrap() > 50);
}

#[test]
fn one_invocation_allocates_far_beyond_its_bounded_heap() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-execution.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 4_000_000_000).unwrap();
    session.initialize().unwrap();
    let configure = session
        .instance
        .get_typed_func::<i32, i32>(&session.store, "telora_gc_interval")
        .unwrap();
    configure.call(&mut session.store, 16_384).unwrap();
    let limit = session
        .instance
        .get_typed_func::<i32, i32>(&session.store, "telora_gc_heap_limit")
        .unwrap();
    assert_eq!(limit.call(&mut session.store, 512 * 1024).unwrap(), 0);
    let allocated = session
        .instance
        .get_typed_func::<(), u64>(&session.store, "telora_gc_allocated_total")
        .unwrap();
    let before = allocated.call(&mut session.store, ()).unwrap();
    let result = session.call(&[10_000.into()]).unwrap();
    assert_eq!(
        result,
        serde_json::json!({
            "name": "a repeated live name that needs non-inline content",
            "children": ["first child needing a content slice", "second child needing a content slice"]
        })
    );
    let collections = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_gc_collections")
        .unwrap();
    assert!(collections.call(&mut session.store, ()).unwrap() > 50);
    let volume = allocated.call(&mut session.store, ()).unwrap() - before;
    eprintln!(
        "independent-allocation churn fuel={} allocated={volume}",
        session.usage().fuel_budget - session.usage().fuel_remaining
    );
    assert!(volume > 16 * 512 * 1024, "only allocated {volume} bytes");
    let committed = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_heap_committed")
        .unwrap();
    assert!(committed.call(&mut session.store, ()).unwrap() <= 512 * 1024);
}

#[test]
fn forced_collection_preserves_codec_parse_format_and_sequence_results() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    for (file, export) in [
        ("codec-traits.telora", "answer"),
        ("gc-parse-plan.telora", "answer"),
        ("codec-decode-check-origins.telora", "answer"),
        ("string-parse-record-effects.telora", "answer"),
        ("codec-enum-collision.telora", "answer"),
        ("format.telora", "inspect"),
        ("sequences.telora", "answer"),
        ("dict-ops.telora", "answer"),
        ("records.telora", "inspect"),
        ("enums.telora", "answer"),
        ("functions.telora", "answer"),
        ("local-recursion.telora", "answer"),
        ("codec-recursive-plan.telora", "sample"),
        ("capture.telora", "answer"),
        ("capture-value.telora", "answer"),
        ("record-origins.telora", "answer"),
        ("from-dyn-fields.telora", "inspect"),
        ("from-dyn-fields.telora", "inspect_checks"),
        (
            "service-collection-witness.telora",
            "inspect_invalid_routes",
        ),
        ("service-collection-witness.telora", "inspect_sources"),
        ("service-collection-witness.telora", "inspect_plan_sources"),
    ] {
        let source = std::fs::read_to_string(format!("{fixture}{file}")).unwrap();
        let bytes = super::compile_export(&source, export).unwrap();
        let evaluate = |force: bool| {
            let mut session = Session::load(&bytes, 500_000_000).unwrap();
            session
                .configure_gc(crate::gc::GcConfig {
                    allocation_interval: u32::from(force),
                    ..Default::default()
                })
                .unwrap();
            session
                .initialize()
                .unwrap_or_else(|e| panic!("{file} init forced={force}: {e}"));
            let result = if session.manifest.types[session.manifest.entry_type as usize].kind
                == crate::artifact::Kind::Function
            {
                session.call(&[])
            } else {
                session.eval()
            };
            result.unwrap_or_else(|e| panic!("{file} eval forced={force}: {e}"))
        };
        assert_eq!(evaluate(false), evaluate(true), "{file}");
    }
}

#[test]
fn growing_live_graph_stops_at_the_heap_budget() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-growing-live.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 500_000_000).unwrap();
    session.initialize().unwrap();
    let configure = session
        .instance
        .get_typed_func::<i32, i32>(&session.store, "telora_gc_interval")
        .unwrap();
    configure.call(&mut session.store, 16_384).unwrap();
    let limit = session
        .instance
        .get_typed_func::<i32, i32>(&session.store, "telora_gc_heap_limit")
        .unwrap();
    assert_eq!(limit.call(&mut session.store, 256 * 1024).unwrap(), 0);
    let failure = session.call(&[10_000.into()]).unwrap_err();
    assert!(failure.contains("unreachable"), "{failure}");
    assert!(
        failure.contains("phase=dynamic category=managed-budget"),
        "{failure}"
    );
    assert!(failure.contains("limit=262144"), "{failure}");
    assert!(
        session.usage().fuel_remaining > 0,
        "the live graph must hit memory before fuel"
    );
    let committed = session
        .instance
        .get_typed_func::<(), i32>(&session.store, "telora_heap_committed")
        .unwrap();
    assert!(committed.call(&mut session.store, ()).unwrap() <= 256 * 1024);
}

#[test]
fn trapped_session_refuses_reentry_and_mutation() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-execution.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let ty = session.manifest.types[session.manifest.entry_type as usize].arguments[0];
    let input = session.input_value(ty, &1.into()).unwrap();
    session.set_request_fuel(50_000);
    assert!(session.call(&[10_000.into()]).unwrap_err().contains("fuel"));
    session.set_request_fuel(10_000_000);
    for result in [
        session.call(&[0.into()]).map(|_| ()),
        session.initialize(),
        session.input_value(ty, &0.into()).map(|_| ()),
        session.pin_value(input).map(|_| ()),
        session.collect_work(&[]).map(|_| ()),
        session.configure_gc(Default::default()),
        session.release_feedback(),
    ] {
        assert!(result.unwrap_err().contains("terminated after trap"));
    }
}

#[test]
fn feedback_release_preserves_future_events() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/debug.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            ..Default::default()
        })
        .unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    assert!(!session.debug_events().unwrap().is_empty());
    for _ in 0..4 {
        session.release_feedback().unwrap();
        assert!(session.debug_events().unwrap().is_empty());
        let result = session.call(&[]).unwrap();
        assert_eq!(result, serde_json::json!(vec![true; 5]));
        assert_eq!(session.take_debug_events().unwrap().len(), 6);
    }
}

#[test]
fn collection_itself_is_fuel_bounded_and_terminal_on_trap() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-execution.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    session.store.set_fuel(1).unwrap();
    assert!(session.collect_garbage().unwrap_err().contains("fuel"));
    session.store.set_fuel(10_000_000).unwrap();
    assert!(
        session
            .eval()
            .unwrap_err()
            .contains("terminated after trap")
    );
}

#[test]
fn collection_profile_records_bounded_work_and_disabled_control() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-execution.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut expected = None;
    for interval in [0, 16_384, 65_536] {
        let mut session = Session::load(&bytes, 500_000_000).unwrap();
        session
            .configure_gc(crate::gc::GcConfig {
                allocation_interval: interval,
                ..Default::default()
            })
            .unwrap();
        session.initialize().unwrap();
        let before = session.gc_stats().unwrap();
        let started = std::time::Instant::now();
        let output = session.call(&[1_000.into()]).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(expected.get_or_insert_with(|| output.clone()), &output);
        let fuel = session.usage().fuel_budget - session.usage().fuel_remaining;
        let after = session.gc_stats().unwrap();
        if interval == 0 {
            assert_eq!(after.collections, before.collections);
        } else {
            assert!(after.collections > before.collections);
        }
        eprintln!(
            "GC profile interval={interval} elapsed_ms={} fuel={fuel} allocated={} managed={} committed={} collections={} memory={}",
            elapsed.as_millis(),
            after.allocated_total - before.allocated_total,
            after.managed_bytes,
            after.words_committed,
            after.collections - before.collections,
            after.memory_bytes
        );
    }
}

#[test]
fn large_pinned_graph_amortizes_collection_during_small_object_churn() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-execution.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 500_000_000).unwrap();
    session.initialize().unwrap();
    let result = *session.manifest.types[session.manifest.entry_type as usize]
        .arguments
        .last()
        .unwrap();
    let strings = session.manifest.types[result as usize]
        .fields
        .iter()
        .find(|field| field.name == "children")
        .unwrap()
        .ty;
    let expected = serde_json::json!(vec![
        "long-lived content retained across temporary small allocations";
        10_000
    ]);
    let value = session.input_value(strings, &expected).unwrap();
    let pin = session.pin_value(value).unwrap();
    let before = session.gc_stats().unwrap();
    let output = session.call(&[1_000.into()]).unwrap();
    assert_eq!(
        output["name"],
        "a repeated live name that needs non-inline content"
    );
    let after = session.gc_stats().unwrap();
    let collections = after.collections - before.collections;
    assert!(
        collections > 0 && collections <= 8,
        "large live graph was rescanned {collections} times"
    );
    let value = session.pinned_value(pin).unwrap();
    assert_eq!(session.output_value(value).unwrap(), expected);
    eprintln!(
        "large-live GC profile collections={collections} committed={} managed={} memory={} fuel={}",
        after.words_committed,
        after.managed_bytes,
        after.memory_bytes,
        session.usage().fuel_budget - session.usage().fuel_remaining
    );
    session.unpin_value(pin).unwrap();
}

#[test]
fn consumed_parse_plan_keeps_shared_children_until_their_last_parent() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gc-parse-plan.telora"
    ))
    .unwrap();
    let bytes = super::compile(&source).unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            ..Default::default()
        })
        .unwrap();
    session.initialize().unwrap();
    let rows = session.allocate(3 * 24).unwrap();
    let pairs = session.allocate(2 * 8).unwrap();
    let packet = session.allocate(16).unwrap();
    // Node 0 is used twice by node 1 and once again by the final root, node 2.
    // Both parents require the same value after intermediate forced collections.
    for (offset, word) in [(0, 0u32), (4, 0), (8, 1), (12, 0)] {
        session
            .write(pairs as usize + offset, &word.to_le_bytes())
            .unwrap();
    }
    session.write(rows as usize, &3u32.to_le_bytes()).unwrap();
    session
        .write(rows as usize + 16, &42i64.to_le_bytes())
        .unwrap();
    for (row, entries) in [(rows + 24, pairs), (rows + 48, pairs + 8)] {
        for (offset, word) in [(0, 6u32), (16, entries), (20, 2)] {
            session
                .write((row + offset) as usize, &word.to_le_bytes())
                .unwrap();
        }
    }
    for (offset, word) in [(0, rows), (4, 3), (8, 2)] {
        session
            .write((packet + offset) as usize, &word.to_le_bytes())
            .unwrap();
    }
    let materialize = session
        .instance
        .get_typed_func::<(u32, u32), u32>(&session.store, "telora_materialize_data")
        .unwrap();
    let pointer = session.guest_call(materialize, (packet, 0)).unwrap();
    let value = Value {
        pointer,
        ty: session.manifest.value_type.unwrap(),
    };
    assert_eq!(session.output().word(packet as u64).unwrap(), 0);
    assert_eq!(session.output().word(packet as u64 + 4).unwrap(), 0);
    let pin = session.pin_value(value).unwrap();
    session.collect_garbage().unwrap();
    let value = session.pinned_value(pin).unwrap();
    assert_eq!(
        session.output_value(value).unwrap(),
        serde_json::json!([[42, 42], 42])
    );
    session.unpin_value(pin).unwrap();
}
