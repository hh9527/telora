use crate::{
    session::Session,
    transform_service::{SourceInput, TransformSession},
};
use telora_core::data_plan::Format;

fn artifact() -> Vec<u8> {
    artifact_from("transform-service")
}
fn artifact_from(name: &str) -> Vec<u8> {
    let source = std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}.telora",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mir = super::graph(&source);
    let export = *mir
        .exports
        .iter()
        .flatten()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    crate::compile_service(&mir.seal_export(export).unwrap()).unwrap()
}
fn initialized(bytes: &[u8]) -> TransformSession {
    let mut service = TransformSession::new(Session::load(bytes, 200_000_000).unwrap()).unwrap();
    assert!(
        service
            .initialize(&[
                SourceInput {
                    name: "a",
                    data: b"42",
                    format: Format::Json
                },
                SourceInput {
                    name: "b",
                    data: b"null",
                    format: Format::Json
                },
            ])
            .unwrap()
            .success
    );
    service
}
fn export(session: &mut Session) -> Vec<u8> {
    let result = session.guest_call(session.exports.alloc, (8, 4)).unwrap();
    session
        .guest_call(session.exports.snapshot_export, result)
        .unwrap();
    let pointer = session.output().raw_word(result as u64).unwrap();
    let length = session.output().raw_word(result as u64 + 4).unwrap();
    let image = session
        .output()
        .raw_bytes(pointer as u64, length as u64)
        .unwrap()
        .to_vec();
    session
        .guest_call(session.exports.free, (pointer, length, 1))
        .unwrap();
    session
        .guest_call(session.exports.free, (result, 8, 4))
        .unwrap();
    image
}
fn import(session: &mut Session, image: &[u8]) -> Result<(), String> {
    let pointer = session.guest_call(session.exports.alloc, (image.len() as u32, 1))?;
    session
        .memory
        .write(&mut session.store, pointer as usize, image)
        .unwrap();
    session.guest_call(
        session.exports.snapshot_import,
        (pointer, image.len() as u32),
    )?;
    session.guest_call(session.exports.free, (pointer, image.len() as u32, 1))
}
fn request(session: &mut Session) -> serde_json::Value {
    let input = session.guest_call(session.exports.alloc, (2, 1)).unwrap();
    session
        .memory
        .write(&mut session.store, input as usize, b"42")
        .unwrap();
    let result = session.guest_call(session.exports.alloc, (12, 4)).unwrap();
    session
        .guest_call(session.exports.run_service, (input, 2, 1, 0, result))
        .unwrap();
    let pointer = session.output().raw_word(result as u64).unwrap();
    let length = session.output().raw_word(result as u64 + 4).unwrap();
    let capacity = session.output().raw_word(result as u64 + 8).unwrap();
    let output = serde_json::from_slice(
        session
            .output()
            .raw_bytes(pointer as u64, length as u64)
            .unwrap(),
    )
    .unwrap();
    session
        .guest_call(session.exports.free, (pointer, capacity, 1))
        .unwrap();
    session
        .guest_call(session.exports.free, (result, 12, 4))
        .unwrap();
    session
        .guest_call(session.exports.free, (input, 2, 1))
        .unwrap();
    output
}

#[test]
fn snapshot_packs_roots_without_moving_or_consuming_the_live_actor() {
    let mut service = initialized(&artifact());
    let session = service.session_mut();
    let string = session
        .manifest
        .types
        .iter()
        .position(|t| t.kind == crate::artifact::Kind::String)
        .unwrap() as u32;
    let sentinel = session
        .input_value(
            string,
            &serde_json::json!("outside snapshot roots".repeat(1024)),
        )
        .unwrap();
    let pin = session.pin_value(sentinel).unwrap();
    let image = export(session);
    assert!(
        !image
            .windows(b"outside snapshot roots".len())
            .any(|window| window == b"outside snapshot roots"),
        "host pins preserve runtime values but are not publication roots"
    );
    assert_eq!(session.pinned_value(pin).unwrap().pointer, sentinel.pointer);
    assert_eq!(
        session.output_value(sentinel).unwrap(),
        "outside snapshot roots".repeat(1024)
    );
    assert_eq!(
        export(session),
        image,
        "packing must produce stable coordinates"
    );
    let garbage = session
        .input_value(
            string,
            &serde_json::json!("unpublished garbage".repeat(4096)),
        )
        .unwrap();
    let garbage_pin = session.pin_value(garbage).unwrap();
    assert_eq!(
        export(session),
        image,
        "allocation history is not image state"
    );
    assert_eq!(
        session.pinned_value(garbage_pin).unwrap().pointer,
        garbage.pointer
    );
    assert_eq!(request(session)["ok"], serde_json::json!([42, 42]));
    assert_eq!(session.pinned_value(pin).unwrap().pointer, sentinel.pointer);
}

#[test]
fn restored_actor_participates_in_gc_and_excludes_unreachable_initialization_objects() {
    let bytes = artifact();
    let mut service = initialized(&bytes);
    let image = export(service.session_mut());
    let mut restored = Session::load(&bytes, 200_000_000).unwrap();
    import(&mut restored, &image).unwrap();
    for _ in 0..8 {
        restored.collect_garbage().unwrap();
        assert_eq!(request(&mut restored)["ok"], serde_json::json!([42, 42]));
        restored
            .guest_call(restored.exports.reset_service, ())
            .unwrap();
    }
    let mut second = Session::load(&bytes, 200_000_000).unwrap();
    import(&mut second, &image).unwrap();
    assert_eq!(request(&mut second)["ok"], serde_json::json!([42, 42]));
}

#[test]
fn truncated_or_wrong_version_actor_images_are_rejected() {
    let bytes = artifact();
    let mut service = initialized(&bytes);
    let image = export(service.session_mut());
    let mut wrong_version = image.clone();
    wrong_version[8..12].copy_from_slice(&37u32.to_le_bytes());
    let mut wrong_origin = image.clone();
    wrong_origin[12..16].copy_from_slice(&0u32.to_le_bytes());
    let mut wrong_extent = image.clone();
    wrong_extent[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut trailing = image.clone();
    trailing.push(0);
    for broken in [
        &image[..8],
        &image[..image.len() - 1],
        &wrong_version,
        &wrong_origin,
        &wrong_extent,
        &trailing,
    ] {
        let mut session = Session::load(&bytes, 200_000_000).unwrap();
        assert!(import(&mut session, broken).is_err());
        assert!(session.ensure_live().is_err());
    }
}

#[test]
fn allocation_failures_report_capacity_and_budget_without_reading_panic_text() {
    let bytes = artifact();
    for (request, category) in [
        (u32::MAX, "capacity-overflow"),
        (64 * 1024 * 1024, "managed-budget"),
    ] {
        let mut service = initialized(&bytes);
        let session = service.session_mut();
        let allocate = session
            .instance
            .get_typed_func::<u32, u32>(&session.store, "telora_alloc")
            .unwrap();
        let error = session.guest_call(allocate, request).unwrap_err();
        assert!(
            error.contains(&format!("phase=dynamic category={category}")),
            "{error}"
        );
        assert!(error.contains(&format!("requested={request}")), "{error}");
        assert!(session.ensure_live().is_err());
    }
}

#[test]
fn guest_memory_growth_failure_is_distinct_from_managed_budget_exhaustion() {
    let bytes = super::compile("pub def answer: Int = 42;").unwrap();
    let mut session = Session::load_with_limits(&bytes, 200_000_000, 8 * 1024 * 1024).unwrap();
    session.initialize().unwrap();
    let allocate = session
        .instance
        .get_typed_func::<u32, u32>(&session.store, "telora_alloc")
        .unwrap();
    let error = session.guest_call(allocate, 16 * 1024 * 1024).unwrap_err();
    assert!(
        error.contains("phase=dynamic category=allocator-failure"),
        "{error}"
    );
    assert!(
        error.contains("requested=16777216 limit=33554432"),
        "{error}"
    );
    assert!(session.ensure_live().is_err());
}

#[test]
fn property_actor_aliases_and_recursive_environments_survive_image_and_forced_gc() {
    let bytes = artifact_from("rooted-actor-image");
    let mut service = TransformSession::new(Session::load(&bytes, 200_000_000).unwrap()).unwrap();
    assert!(service.initialize(&[]).unwrap().success);
    let session = service.session_mut();
    assert_eq!(session.diagnostics().unwrap().len(), 1);
    let temporary = b"RFC0315 garbage :: outside publication roots";
    assert!(
        session
            .memory
            .data(&session.store)
            .windows(temporary.len())
            .any(|bytes| bytes == temporary)
    );
    let image = export(session);
    assert!(
        !image
            .windows(temporary.len())
            .any(|bytes| bytes == temporary),
        "unreachable initialization temporary leaked into the image"
    );
    let mut restored = Session::load(&bytes, 200_000_000).unwrap();
    import(&mut restored, &image).unwrap();
    for session in [session, &mut restored] {
        session
            .configure_gc(crate::gc::GcConfig {
                allocation_interval: 1,
                ..Default::default()
            })
            .unwrap();
        for _ in 0..4 {
            session
                .guest_call(session.exports.reset_service, ())
                .unwrap();
            session.collect_garbage().unwrap();
            assert_eq!(request(session)["ok"], serde_json::json!(vec![true; 5]));
            assert!(
                session.diagnostics().unwrap().is_empty(),
                "property provider ran again"
            );
        }
    }
}
