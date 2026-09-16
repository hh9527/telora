extern crate std;
use super::*;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/yaml")
            .join(name),
    )
    .unwrap()
}
fn parse(text: &str, limits: DataLimits) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let mut sources = SourceDatabase::default();
    let id = sources.add("test.yaml", text);
    crate::data_plan::parse_registered_with_limits(
        &sources,
        id,
        crate::data_plan::Format::Yaml,
        limits,
    )
}

#[test]
fn eol_and_chunk_boundaries_preserve_values_and_positions() {
    let text = fixture("core.yaml");
    let mut snapshots = Vec::new();
    for eol in ["\n", "\r\n", "\r"] {
        let text = text.replace('\n', eol);
        let mut sources = SourceDatabase::default();
        let id = sources.add("core.yaml", &text);
        let plan = crate::data_plan::parse_registered(&sources, id, crate::data_plan::Format::Yaml)
            .unwrap();
        snapshots.push((
            crate::data_plan_test::render(&plan),
            plan.nodes()
                .iter()
                .map(|n| plan.compact(n.location).0)
                .collect::<Vec<_>>(),
        ));
        let expected = lines::index(core::iter::once(text.as_str()));
        for at in text
            .char_indices()
            .map(|(i, _)| i)
            .chain(core::iter::once(text.len()))
        {
            assert_eq!(
                lines::index([&text[..at], "", &text[at..]].into_iter()),
                expected
            );
        }
        let mut parser = Parser::new(id, sources.get(id).text(), DataLimits::default()).unwrap();
        parser.lines = lines::index(
            text.char_indices()
                .map(|(i, ch)| &text[i..i + ch.len_utf8()]),
        );
        let plan = parser.parse().unwrap();
        assert_eq!(
            crate::data_plan_test::render(&plan),
            snapshots.last().unwrap().0
        );
    }
    assert_eq!(snapshots[0], snapshots[1]);
    assert_eq!(snapshots[0], snapshots[2]);
}

#[test]
fn precise_duplicate_key_location_in_compact_mapping() {
    let text = fixture("locations.yaml");
    let errors = parse(&text, DataLimits::default()).unwrap_err();
    assert!(errors[0].message.contains("duplicate YAML key"));
    let first = text.find("next:").unwrap();
    let second = text.rfind("next:").unwrap();
    assert_eq!(errors[0].labels[0].location.range(), second..second + 4);
    assert_eq!(errors[0].labels[1].location.range(), first..first + 4);
    let text = text[..second - 2].trim_end();
    let plan = parse(text, DataLimits::default()).unwrap();
    let key = plan
        .nodes()
        .iter()
        .find_map(|n| match &n.kind {
            crate::json::DataPlanNodeKind::Object(fields) => fields.get("é"),
            _ => None,
        })
        .unwrap();
    assert_eq!(key.key_location.range(), 2..4);
    assert_eq!(plan.node(key.value).location.range(), 6..12);
}

#[test]
fn resource_limits_apply_to_decoded_data_before_postprocessing() {
    let text = fixture("limits.yaml");
    let exact = DataLimits {
        file_size: text.len(),
        depth: 2,
        nodes: 3,
        container_size: 2,
        string_len: 3,
        bytes_len: 2,
        payloads_bytes: 7,
    };
    let plan = parse(&text, exact).unwrap();
    let pointer = plan.nodes().as_ptr();
    assert_eq!(pointer, plan.into_postorder().nodes().as_ptr());
    for (name, limits) in [
        (
            "file_size",
            DataLimits {
                file_size: text.len() - 1,
                ..exact
            },
        ),
        ("depth", DataLimits { depth: 1, ..exact }),
        ("nodes", DataLimits { nodes: 2, ..exact }),
        (
            "container_size",
            DataLimits {
                container_size: 1,
                ..exact
            },
        ),
        (
            "string_len",
            DataLimits {
                string_len: 2,
                ..exact
            },
        ),
        (
            "bytes_len",
            DataLimits {
                bytes_len: 1,
                ..exact
            },
        ),
        (
            "payloads_bytes",
            DataLimits {
                payloads_bytes: 6,
                ..exact
            },
        ),
    ] {
        let error = parse(&text, limits).unwrap_err();
        assert!(error[0].message.contains(name), "{name}: {error:?}");
        assert!(!error[0].labels.is_empty());
    }
    // Quotas count final block-scalar content, not stripped trailing newlines.
    let limits = DataLimits {
        string_len: 1,
        payloads_bytes: 2,
        ..DataLimits::default()
    };
    assert!(parse("a: |-\n  x\n\n\n", limits).is_ok());
    assert!(parse("a: |+\n  x\n\n\n", limits).is_err());
    assert!(
        parse(
            "a: abc\nb: [invalid",
            DataLimits {
                string_len: 2,
                ..DataLimits::default()
            }
        )
        .unwrap_err()[0]
            .message
            .contains("string_len")
    );
}

#[test]
fn small_stack_handles_flow_block_wide_errors_and_cleanup() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let depth = 20_000;
            let text = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            assert!(
                parse(&text, DataLimits::default()).unwrap_err()[0]
                    .message
                    .contains("depth")
            );
            let limits = DataLimits {
                depth: depth + 1,
                ..DataLimits::default()
            };
            let plan = parse(&text, limits).unwrap();
            assert_eq!(plan.nodes().len(), depth + 1);
            drop(plan.into_postorder());
            assert!(parse(&"[".repeat(depth), limits).is_err());
            let text = format!("[{}0]", "0,".repeat(100_000));
            assert_eq!(
                parse(&text, DataLimits::default()).unwrap().nodes().len(),
                100_002
            );
            let mut text = String::new();
            for i in 0..1000 {
                text.push_str(&format!("{}a:\n", " ".repeat(i)));
            }
            text.push_str(&format!("{}0", " ".repeat(1000)));
            assert!(
                parse(&text, DataLimits::default()).unwrap_err()[0]
                    .message
                    .contains("depth")
            );
            assert_eq!(parse(&text, limits).unwrap().nodes().len(), 1001);
        })
        .unwrap()
        .join()
        .unwrap();
}
