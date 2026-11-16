use crate::{
    located_input::{Field, InputValue, LocatedValue, Span},
    session::Session,
    transport::Value,
};

fn checker() -> Session {
    let bytes = super::compile(
        r#"
use std::codec as codec;
use std::value::{Value};
type Intent = struct { a: String };
pub def answer: Fn(Value) -> Bool = fn(input) {
    let intent = codec::decode@[Intent](input).unwrap!();
    if intent.a == "cpu_usage" { True }
    else { fail!("unsupported projected member", intent.a) }
};
"#,
    )
    .unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    session
}

fn request(text: &str, value: InputValue) -> LocatedValue {
    let key = text.find("a:").unwrap() as u32;
    let member = text.find("x.field").unwrap() as u32;
    LocatedValue {
        span: Some(Span {
            start: 0,
            end: text.len() as u32,
        }),
        value: InputValue::Object(vec![Field {
            name: "a".into(),
            span: Some(Span {
                start: key,
                end: key + 1,
            }),
            value: LocatedValue {
                span: Some(Span {
                    start: member,
                    end: member + 7,
                }),
                value,
            },
        }]),
    }
}

#[test]
fn located_input_reports_dari_span_through_codec_and_domain_checks() {
    for bad in [false, true] {
        let mut session = checker();
        let text = "# 中文🙂\r\ndata rows = query_remote! { project({ a: x.field }) };\r\n";
        let input = request(
            text,
            if bad {
                InputValue::Int(42)
            } else {
                InputValue::String("unsupported".into())
            },
        );
        let value = session.import_located_value(&input).unwrap();
        let callable = Value {
            pointer: session.entry().unwrap(),
            ty: session.manifest.entry_type,
        };
        // Exercise GC too: imported origins and strings must remain reachable.
        let roots = session.collect_work(&[callable, value]).unwrap().0;
        assert!(session.invoke_values(roots[0], &[roots[1]]).is_err());
        let diagnostics = session.diagnostics().unwrap();
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = &diagnostics[0];
        if !bad {
            assert_eq!(diagnostic.message, "unsupported projected member");
        }
        let start = text.find("x.field").unwrap();

        let expected = [0, 0, start as u32, 0, (start + 7) as u32];
        assert!(diagnostic.locs.contains(&expected), "{diagnostic:?}");
        // The rule remains in Telora, the data subject is in the original .dari.
        assert_ne!(diagnostic.locs[0][0], 0);
        println!("{} -> analysis.dari: x.field", diagnostic.message);
    }
}

#[test]
fn located_input_accepts_valid_values_and_rejects_invalid_metadata() {
    let mut session = checker();
    let text = "# 中文\ndata a = { a: x.field };";
    let mut input = request(text, InputValue::String("cpu_usage".into()));
    let value = session.import_located_value(&input).unwrap();
    let callable = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let result = session.invoke_values(callable, &[value]).unwrap();
    assert_eq!(
        session.output_value(result).unwrap(),
        serde_json::json!(true)
    );
    input.span = Some(Span { start: 8, end: 7 });
    assert!(
        session
            .import_located_value(&input)
            .unwrap_err()
            .contains("offset")
    );
    input.span = Some(Span {
        start: 0,
        end: u32::MAX,
    });
    assert!(session.import_located_value(&input).is_err());
}

#[test]
fn located_input_preserves_nested_values_and_unknown_field_subjects() {
    let mut session = checker();
    let text = "data rows = { a: x.field };";
    let mut input = request(
        text,
        InputValue::Array(vec![
            LocatedValue {
                span: None,
                value: InputValue::Null,
            },
            LocatedValue {
                span: None,
                value: InputValue::Bool(true),
            },
            LocatedValue {
                span: None,
                value: InputValue::Bool(false),
            },
            LocatedValue {
                span: None,
                value: InputValue::Float(0.125),
            },
            LocatedValue {
                span: None,
                value: InputValue::Int(i64::MAX),
            },
            LocatedValue {
                span: None,
                value: InputValue::String("中文🙂 long host string beyond inline storage".into()),
            },
        ]),
    );
    let value = session.import_located_value(&input).unwrap();
    let value = session.collect_work(&[value]).unwrap().0[0];
    assert_eq!(
        session.output_value(value).unwrap(),
        serde_json::json!({"a": [null, true, false, 0.125, i64::MAX, "中文🙂 long host string beyond inline storage"]})
    );
    if let InputValue::Object(fields) = &mut input.value {
        fields[0].name = "unknown".into();
        fields[0].value.value = InputValue::String("cpu_usage".into());
    }
    let value = session.import_located_value(&input).unwrap();
    let callable = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    assert!(session.invoke_values(callable, &[value]).is_err());
    let diagnostics = session.diagnostics().unwrap();
    let start = text.find("x.field").unwrap() as u32;
    assert!(
        diagnostics
            .iter()
            .any(|d| d.locs.contains(&[0, 0, start, 0, start + 7])),
        "{diagnostics:?}"
    );
}

#[test]
fn located_service_preserves_offsets_recovers_and_reclaims_requests() {
    let mir = super::graph(
        r#"
use std::transform_service as service;
use std::_entry::transform as entry;
use std::codec as codec;
use std::value::{Value};
type Intent = struct { a: String };
type Model = struct {};
impl service::TransformService for Model {
    init: fn(ctx) { {}.ty!(Self) },
    transform: fn(self, input) {
        let intent = codec::decode@[Intent](input).unwrap!();
        if intent.a == "cpu_usage" { Value::True }
        else { fail!("unsupported projected member", intent.a) }
    },
};
pub def answer: entry::Plan = entry::prepare(Model.type);
"#,
    );
    let symbol = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let artifact = crate::codegen::compile_service_build(&mir.seal_export(symbol).unwrap())
        .unwrap()
        .bytes;
    let mut service = crate::transform_service::TransformSession::new(
        Session::load(&artifact, 100_000_000).unwrap(),
    )
    .unwrap();
    assert!(service.initialize(&[]).unwrap().success);
    service.seal_initialization().unwrap();
    let sources = service.session().manifest.sources.len();
    let text = "# 中文🙂\r\ndata a = { a: x.field };";
    let start = text.find("x.field").unwrap() as u32;
    for _ in 0..4 {
        for (input, expected_error) in [
            (b"TLI2".to_vec(), true),
            (
                request(text, InputValue::String("unsupported".into()))
                    .encode()
                    .unwrap(),
                true,
            ),
            (request(text, InputValue::Int(42)).encode().unwrap(), true),
            (
                request(text, InputValue::String("cpu_usage".into()))
                    .encode()
                    .unwrap(),
                false,
            ),
        ] {
            service.reset().unwrap();
            let response: serde_json::Value =
                serde_json::from_slice(&service.transform_located(&input).unwrap()).unwrap();
            assert_eq!(response["error"], expected_error, "{response}");
            if !expected_error {
                assert_eq!(response["ok"], true);
            } else if input.len() > 4 {
                let locs = response["diagnostics"][0]["locs"].as_array().unwrap();
                assert!(
                    locs.iter().any(|l| l["source"] == "<input>"
                        && l["start"]["line"] == 0
                        && l["start"]["offset"] == start
                        && l["end"]["offset"] == start + 7),
                    "{response}"
                );
            }
            assert_eq!(service.session().manifest.sources.len(), sources);
        }
    }
    service.reset().unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&service.transform(br#"{"a":"cpu_usage"}"#).unwrap()).unwrap();
    assert_eq!(json["ok"], true);
}
