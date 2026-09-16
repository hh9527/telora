use super::*;

#[test]
fn exact_limits_count_final_tables_and_decoded_payloads() {
    let text = fixture("limits.toml");
    let limits = DataLimits {
        file_size: text.len(),
        nodes: 5,
        depth: 4,
        container_size: 2,
        string_len: 3,
        bytes_len: 0,
        payloads_bytes: 7,
    };
    let plan = parse(&text, limits).unwrap();
    crate::data_plan::enforce_limits(&plan, limits, text.len()).unwrap();
    for (field, smaller) in [
        (
            "file_size",
            DataLimits {
                file_size: limits.file_size - 1,
                ..limits
            },
        ),
        ("nodes", DataLimits { nodes: 4, ..limits }),
        ("depth", DataLimits { depth: 3, ..limits }),
        (
            "container_size",
            DataLimits {
                container_size: 1,
                ..limits
            },
        ),
        (
            "string_len",
            DataLimits {
                string_len: 2,
                ..limits
            },
        ),
        (
            "payloads_bytes",
            DataLimits {
                payloads_bytes: 6,
                ..limits
            },
        ),
    ] {
        assert!(
            parse(&text, smaller).unwrap_err()[0]
                .message
                .contains(field),
            "{field}"
        );
    }

    for (text, depth) in [
        ("a.b.c=1", 4),
        ("a={b={c=1}}", 4),
        ("[[a]]\nx=1\n[[a]]\nx=2", 4),
        ("[[a.b]]\nx=1", 5),
        ("[[a]]\n[[a.b]]\nx=1", 6),
    ] {
        assert!(
            parse(
                text,
                DataLimits {
                    depth,
                    ..DataLimits::default()
                }
            )
            .is_ok()
        );
        assert!(
            parse(
                text,
                DataLimits {
                    depth: depth - 1,
                    ..DataLimits::default()
                }
            )
            .unwrap_err()[0]
                .message
                .contains("depth")
        );
    }
    let text = "[[a]]\nx=1\n[[a]]\nx=2";
    let limits = DataLimits {
        payloads_bytes: 3,
        nodes: 6,
        depth: 4,
        container_size: 2,
        ..DataLimits::default()
    };
    let plan = parse(text, limits).unwrap();
    crate::data_plan::enforce_limits(&plan, limits, text.len()).unwrap();
    assert!(
        parse(
            text,
            DataLimits {
                payloads_bytes: 2,
                ..limits
            }
        )
        .is_err()
    );
    for text in [
        "x='abc'",
        "x=\"\\u4e2d\"",
        "'abc'=1",
        "x='''\nabc'''",
        "x=\"\"\"\nabc\"\"\"",
    ] {
        assert!(
            parse(
                text,
                DataLimits {
                    string_len: 3,
                    ..DataLimits::default()
                }
            )
            .is_ok(),
            "{text}"
        );
        assert!(
            parse(
                text,
                DataLimits {
                    string_len: 2,
                    ..DataLimits::default()
                }
            )
            .is_err(),
            "{text}"
        );
    }
    for text in ["x=\"abcd\\q\"", "x='abcd' broken", "x=\"\"\"abcd"] {
        assert!(
            parse(
                text,
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
    let text = "x=1979-05-27 07:32:00+00:00";
    let limits = DataLimits {
        string_len: 20,
        payloads_bytes: 21,
        ..DataLimits::default()
    };
    let plan = parse(text, limits).unwrap();
    crate::data_plan::enforce_limits(&plan, limits, text.len()).unwrap();
    assert!(
        parse(
            text,
            DataLimits {
                string_len: 19,
                ..limits
            }
        )
        .is_err()
    );
    assert!(
        parse(
            text,
            DataLimits {
                payloads_bytes: 20,
                ..limits
            }
        )
        .is_err()
    );
    let escaped = parse("x=\"\\r\\n\"", DataLimits::default()).unwrap();
    assert!(escaped.nodes().iter().any(
        |n| matches!(&n.kind, DataPlanNodeKind::Scalar(DataScalar::String(s)) if s == "\r\n")
    ));
}

#[test]
fn small_stack_deep_wide_unclosed_and_cleanup() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let count = 20_000;
            let deep = format!("a={}0{}", "[".repeat(count), "]".repeat(count));
            let limits = DataLimits {
                depth: count + 2,
                ..DataLimits::default()
            };
            let plan = parse(&deep, limits).unwrap();
            assert_eq!(plan.nodes().len(), count + 2);
            crate::data_plan::enforce_limits(&plan, limits, deep.len()).unwrap();
            for (index, node) in plan.nodes().iter().enumerate() {
                if let DataPlanNodeKind::Array(items) = &node.kind {
                    assert!(items.iter().all(|id| id.index() < index));
                }
            }
            drop(plan);
            assert!(
                parse(&deep, DataLimits::default()).unwrap_err()[0]
                    .message
                    .contains("depth")
            );
            assert!(parse(&format!("a={}0", "[".repeat(count)), limits).is_err());
            let dotted = format!("{}x=1", "a.".repeat(count));
            drop(parse(&dotted, limits).unwrap());
            assert!(
                parse(&dotted, DataLimits::default()).unwrap_err()[0]
                    .message
                    .contains("depth")
            );
            let inline = format!("a={}0{}", "{x=".repeat(count), "}".repeat(count));
            drop(parse(&inline, limits).unwrap());
            let wide = format!("a=[{}0]", "0,".repeat(100_000));
            drop(parse(&wide, DataLimits::default()).unwrap());
            assert!(
                parse(
                    &wide,
                    DataLimits {
                        container_size: 100_000,
                        ..DataLimits::default()
                    }
                )
                .unwrap_err()[0]
                    .message
                    .contains("container_size")
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
