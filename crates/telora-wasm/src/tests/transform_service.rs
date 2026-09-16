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
fn parser_diagnostics_preserve_individual_errors_and_relative_coordinates() {
    let bytes = artifact();
    let mut service = TransformSession::new(Session::load(&bytes, 10_000_000).unwrap()).unwrap();
    initialize(&mut service);
    let output = service.transform(br#"{"x":1,"x":2,"y":1,"y":2}"#).unwrap();
    assert_eq!(output["error"], true);
    let errors = output["diagnostics"].as_array().unwrap();
    assert_eq!(errors.len(), 2, "{output}");
    for error in errors {
        assert!(error["labels"].as_array().unwrap().is_empty());
        assert!(error["notes"].as_array().unwrap().iter().any(|note|
            note.as_str().unwrap().contains("input range")));
    }
    assert_eq!(service.transform(b"7").unwrap()["ok"], serde_json::json!([42, 7]));
}

#[test]
fn source_parser_diagnostics_keep_registered_source_ranges() {
    let bytes = artifact();
    for (format, input) in [
        (Format::Json, "{\n \"x\": 1, \"x\": 2\n}"),
        (Format::Yaml, "x: 1\nx: 2\n"),
        (Format::Toml, "x = 1\nx = 2\n"),
    ] {
        let mut service = TransformSession::new(Session::load(&bytes, 10_000_000).unwrap()).unwrap();
        let result = service.initialize(&[
            SourceInput { name: "a", data: input.as_bytes(), format },
            SourceInput { name: "b", data: b"null", format: Format::Json },
        ]).unwrap();
        assert!(!result.success);
        let errors = result.diagnostics.as_array().unwrap();
        assert!(!errors.is_empty());
        let labels = errors[0]["labels"].as_array().unwrap();
        assert!(!labels.is_empty(), "{}", result.diagnostics);
        assert!(labels.iter().all(|label| label["location"]["source"] == "@service/a"));
        assert!(labels.iter().any(|label| label["location"]["start"]["line"] == 1));
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
    for input in ["1", "null", "2"] {
        service.reset().unwrap();
        let output = service.transform(input.as_bytes()).unwrap();
        assert_eq!(output["schema"], "telora.service/v1");
        if input == "null" {
            assert_eq!(output["diagnostics"][0]["message"], "missing query");
            assert!(
                !output["diagnostics"][0]["labels"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
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
    session.memory_limit = 1024 * 1024;
    let mut service = TransformSession::new(session).unwrap();
    initialize(&mut service);
    for input in ["ok", "loop", "ok", "grow", "ok"] {
        service.reset().unwrap();
        if input == "loop" {
            service.session_mut().store.set_fuel(100_000).unwrap();
        }
        let result = service.transform(format!("\"{input}\"").as_bytes());
        match input {
            "loop" => assert!(result.unwrap_err().contains("fuel")),
            "grow" => assert!(result.unwrap_err().contains("growth")),
            _ => assert_eq!(result.unwrap()["ok"], serde_json::json!([42, "ok"])),
        }
    }
    let mut size = None;
    for _ in 0..256 {
        service.reset().unwrap();
        let current = service.session().memory.data_size(&service.session().store);
        if let Some(expected) = size {
            assert_eq!(current, expected);
        }
        size = Some(current);
        assert_eq!(
            service.transform(b"42").unwrap()["ok"],
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
    assert!(service.transform(b"42").is_err());
}
