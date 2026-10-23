use crate::{
    session::Session,
    transform_service::{SourceInput, TransformSession},
};
use telora_core::data_plan::Format;

fn artifact() -> Vec<u8> {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/transform-service.telora"
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
    crate::compile_service(&mir.seal_export(export).unwrap()).unwrap()
}

fn initialize(service: &mut TransformSession) {
    let result = service
        .initialize(&[
            SourceInput {
                name: "a",
                data: b"42",
                format: Format::Json,
            },
            SourceInput {
                name: "b",
                data: b"null",
                format: Format::Json,
            },
        ])
        .unwrap();
    assert!(result.success, "{}", result.diagnostics);
    assert_eq!(result.diagnostics, serde_json::json!([]));
    service.seal_initialization().unwrap();
}

#[test]
fn actor_roots_survive_requests_and_garbage_is_collected_without_truncation() {
    let mut service =
        TransformSession::new(Session::load(&artifact(), 100_000_000).unwrap()).unwrap();
    initialize(&mut service);
    service.reset().unwrap();
    service.session_mut().collect_garbage().unwrap();
    let baseline = service.session_mut().gc_stats().unwrap().managed_bytes;
    let request = serde_json::to_vec(&"request content é🦀".repeat(8192)).unwrap();
    let mut memory_plateau = None;
    for iteration in 0..12 {
        let response = service.transform(&request).unwrap();
        let response: serde_json::Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["ok"][0], 42);
        assert_eq!(
            response["ok"][1],
            serde_json::from_slice::<serde_json::Value>(&request).unwrap()
        );
        let before_cleanup = service.session_mut().gc_stats().unwrap().managed_bytes;
        service.reset().unwrap();
        // Calling cleanup does not restore/truncate an allocation prefix.
        assert_eq!(
            service.session_mut().gc_stats().unwrap().managed_bytes,
            before_cleanup
        );
        service.session_mut().collect_garbage().unwrap();
        assert!(service.session_mut().gc_stats().unwrap().managed_bytes < before_cleanup);
        let failed = service.transform(br#""warnings""#).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&failed).unwrap()["error"],
            true
        );
        service.reset().unwrap();
        service.session_mut().collect_garbage().unwrap();
        // Table buffers can retain capacity, but the request's large raw input
        // and diagnostic objects cannot remain reachable from the actor.
        assert!(service.session_mut().gc_stats().unwrap().managed_bytes < baseline + 4096);
        if iteration >= 3 {
            let bytes = service.session().memory.data_size(&service.session().store);
            if let Some(plateau) = memory_plateau {
                assert_eq!(bytes, plateau);
            }
            memory_plateau = Some(bytes);
        }
    }
}

#[test]
fn source_readers_grow_reuse_and_enforce_exact_byte_limit() {
    use crate::transform_service::SourceReader;
    use std::io::Cursor;
    let bytes = artifact();
    // a survives overwrite by a larger b; b forces more than one realloc.
    let first = br#"{"plain":"retained","escaped":"a\nb"}"#;
    let mut second = vec![b' '; 32768];
    second.extend_from_slice(b"null");
    for limit in [second.len(), second.len() - 1] {
        let mut service =
            TransformSession::new(Session::load(&bytes, 100_000_000).unwrap()).unwrap();
        let result = service.initialize_readers(
            [
                Ok(SourceReader {
                    name: "a".into(),
                    reader: Box::new(Cursor::new(first)),
                    format: Format::Json,
                }),
                Ok(SourceReader {
                    name: "b".into(),
                    reader: Box::new(Cursor::new(&second)),
                    format: Format::Json,
                }),
            ],
            limit,
        );
        if limit < second.len() {
            assert!(result.err().unwrap().contains("file_size limit"));
            assert!(service.seal_initialization().is_err());
        } else {
            assert!(result.unwrap().success);
            service.seal_initialization().unwrap();
            service.reset().unwrap();
            let reply = transform(&mut service, b"7").unwrap();
            assert_eq!(
                reply["ok"],
                serde_json::json!([
                    {"plain":"retained","escaped":"a\nb"}, 7
                ])
            );
        }
    }
}

#[test]
fn parser_diagnostics_preserve_individual_errors_and_relative_coordinates() {
    let bytes = artifact();
    let mut service = TransformSession::new(Session::load(&bytes, 10_000_000).unwrap()).unwrap();
    initialize(&mut service);
    let output = transform(&mut service, br#"{"x":1,"x":2,"y":1,"y":2}"#).unwrap();
    assert_eq!(output["error"], true);
    let errors = output["diagnostics"].as_array().unwrap();
    assert_eq!(errors.len(), 2, "{output}");
    for error in errors {
        let locs = error["locs"].as_array().unwrap();
        assert!(!locs.is_empty(), "{error}");
        assert!(
            locs.iter().all(|loc| loc["source"] == "<input>"
                && loc["start"]["line"] == 0
                && loc["end"]["line"] == 0),
            "{error}"
        );
        assert!(!error["message"].as_str().unwrap().contains("input range"));
    }
    assert_eq!(
        transform(&mut service, b"7").unwrap()["ok"],
        serde_json::json!([42, 7])
    );
}

#[test]
fn source_parser_diagnostics_keep_registered_source_ranges() {
    let bytes = artifact();
    for (format, input) in [
        (Format::Json, "{\n \"x\": 1, \"x\": 2\n}"),
        (Format::Yaml, "x: 1\nx: 2\n"),
        (Format::Toml, "x = 1\nx = 2\n"),
    ] {
        let mut service =
            TransformSession::new(Session::load(&bytes, 10_000_000).unwrap()).unwrap();
        let result = service
            .initialize(&[
                SourceInput {
                    name: "a",
                    data: input.as_bytes(),
                    format,
                },
                SourceInput {
                    name: "b",
                    data: b"null",
                    format: Format::Json,
                },
            ])
            .unwrap();
        assert!(!result.success);
        let errors = result.diagnostics.as_array().unwrap();
        assert!(!errors.is_empty());
        let locs = errors[0]["locs"].as_array().unwrap();
        assert!(!locs.is_empty(), "{}", result.diagnostics);
        assert!(locs.iter().all(|loc| loc["source"] == "@service/a"));
        assert!(locs.iter().any(|loc| loc["start"]["line"] == 2));
        assert!(service.seal_initialization().is_err());
    }
}

#[test]
fn static_service_initializes_and_captures_each_request() {
    let bytes = artifact();
    assert_eq!(bytes, artifact(), "entry codegen must be deterministic");
    let mut service = TransformSession::new(Session::load(&bytes, 10_000_000).unwrap()).unwrap();
    assert_eq!(service.sources(), ["a", "b"]);
    initialize(&mut service);
    for input in ["1", "null", "\"warnings\"", "2"] {
        service.reset().unwrap();
        let output = transform(&mut service, input.as_bytes()).unwrap();
        assert_eq!(output["schema"], "telora.service/v1");
        if input == "\"warnings\"" {
            assert_eq!(output["error"], true);
            let diagnostics = output["diagnostics"].as_array().unwrap();
            assert_eq!(diagnostics.len(), 3);
            assert_eq!(diagnostics[0]["message"], "first\nwarning é");
            assert_eq!(diagnostics[1]["message"], "second warning");
            for diagnostic in &diagnostics[..2] {
                assert_eq!(diagnostic["severity"], "Warning");
                assert!(diagnostic["locs"].as_array().unwrap().len() >= 2);
            }
            assert_eq!(diagnostics[2]["severity"], "Error");
        } else if input == "null" {
            assert_eq!(output["diagnostics"][0]["message"], "missing query");
            let locs = output["diagnostics"][0]["locs"].as_array().unwrap();
            assert!(locs.iter().any(|loc| loc["source"] == "<input>"
                && loc["start"] == serde_json::json!({"line": 0, "offset": 0})
                && loc["end"] == serde_json::json!({"line": 0, "offset": 4})));
        } else {
            assert_eq!(
                output["ok"],
                serde_json::json!([42, input.parse::<i64>().unwrap()])
            );
        }
    }
}

#[test]
fn reset_restores_initialized_service_after_fuel_and_memory_traps() {
    let bytes = artifact();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    // Include paged heap, root metadata and collection working memory. The
    // grow branch still exceeds this fixed engine boundary by a wide margin.
    session.memory_limit = 2 * 1024 * 1024;
    let mut service = TransformSession::new(session).unwrap();
    initialize(&mut service);
    for input in ["ok", "loop", "ok", "grow", "ok"] {
        service.session_mut().fuel_budget = 100_000_000;
        service.reset().unwrap();
        service.session_mut().fuel_budget = if input == "loop" {
            100_000
        } else {
            100_000_000
        };
        if input == "loop" {
            service.session_mut().store.set_fuel(100_000).unwrap();
        }
        let result = transform(&mut service, format!("\"{input}\"").as_bytes());
        match input {
            "loop" => assert!(result.unwrap_err().contains("fuel")),
            "grow" => assert!(result.unwrap_err().contains("growth")),
            _ => assert_eq!(
                result.unwrap_or_else(|e| panic!("request {input}: {e}"))["ok"],
                serde_json::json!([42, "ok"])
            ),
        }
    }
    let mut size = None;
    for _ in 0..256 {
        service.reset().unwrap();
        service.session_mut().collect_garbage().unwrap();
        let current = service.session().memory.data_size(&service.session().store);
        if let Some(expected) = size {
            assert_eq!(current, expected);
        }
        size = Some(current);
        assert_eq!(
            transform(&mut service, b"42").unwrap()["ok"],
            serde_json::json!([42, 42])
        );
    }
}

#[test]
fn source_parse_failure_returns_guest_diagnostics_and_cannot_publish_a_baseline() {
    let mut service =
        TransformSession::new(Session::load(&artifact(), 10_000_000).unwrap()).unwrap();
    let result = service
        .initialize(&[
            SourceInput {
                name: "a",
                data: b"{bad}",
                format: Format::Json,
            },
            SourceInput {
                name: "b",
                data: b"null",
                format: Format::Json,
            },
        ])
        .unwrap();
    assert!(!result.success);
    assert!(result.diagnostics.to_string().contains("@service/a"));
    assert!(service.seal_initialization().is_err());
    assert!(transform(&mut service, b"42").is_err());
}

fn transform(service: &mut TransformSession, input: &[u8]) -> Result<serde_json::Value, String> {
    let bytes = service.transform(input)?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

#[test]
fn forced_gc_survives_service_initialization_snapshot_requests_and_reset() {
    let bytes = artifact();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    let config = crate::gc::GcConfig {
        allocation_interval: 1,
        heap_limit: 4 * 1024 * 1024,
    };
    session.configure_gc(config).unwrap();
    let mut service = TransformSession::new(session).unwrap();
    initialize(&mut service);
    assert_eq!(service.session().gc_config, config);
    for _ in 0..4 {
        service.reset().unwrap();
        assert_eq!(
            transform(&mut service, b"\"ok\"").unwrap()["ok"],
            serde_json::json!([42, "ok"])
        );
        assert_eq!(service.session().gc_config, config);
    }
    service.session_mut().fuel_budget = 10_000;
    service.session_mut().store.set_fuel(10_000).unwrap();
    assert!(
        transform(&mut service, b"\"loop\"")
            .unwrap_err()
            .contains("fuel")
    );
    assert!(
        service
            .session_mut()
            .eval()
            .unwrap_err()
            .contains("terminated after trap")
    );
    service.session_mut().fuel_budget = 100_000_000;
    service.reset().unwrap();
    assert_eq!(service.session().gc_config, config);
    assert_eq!(
        transform(&mut service, b"\"ok\"").unwrap()["ok"],
        serde_json::json!([42, "ok"])
    );
}
