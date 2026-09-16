//! Rust tests cover chunk scheduling, admission and stack/position contracts.
//! User-visible syntax and codec behavior live in language fixtures.
extern crate std;
use super::*;
use crate::DataLimits;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/json")
            .join(name),
    )
    .unwrap()
}

fn snapshot<'a>(source: SourceId, text: &str, chunks: impl Iterator<Item = &'a str>) -> String {
    match super::super::parse::Parser::new(source, chunks, DataLimits::default()).parse(text.len())
    {
        Ok(plan) => format!("{:?}", plan.nodes()),
        Err(error) => format!("{error:?}"),
    }
}

#[test]
fn every_chunk_boundary_preserves_values_and_error_locations() {
    for name in [
        "chunks.json",
        "bad-escape.json",
        "high-surrogate.json",
        "low-surrogate.json",
        "invalid-number.json",
    ] {
        let text = fixture(name);
        let mut sources = SourceDatabase::default();
        let id = sources.add(name, &text);
        let expected = snapshot(id, &text, core::iter::once(text.as_str()));
        let mut boundaries: Vec<_> = text.char_indices().map(|(at, _)| at).collect();
        boundaries.push(text.len());
        for &split in &boundaries {
            assert_eq!(
                snapshot(
                    id,
                    &text,
                    ["", &text[..split], "", &text[split..], ""].into_iter()
                ),
                expected,
                "{name} at {split}"
            );
        }
        assert_eq!(
            snapshot(
                id,
                &text,
                boundaries.windows(2).map(|pair| &text[pair[0]..pair[1]])
            ),
            expected,
            "{name} character chunks"
        );
    }
}

#[test]
fn construction_rejects_limits_and_accepts_exact_boundaries() {
    let text = fixture("limits.json");
    let mut sources = SourceDatabase::default();
    let id = sources.add("limits.json", &text);
    let exact = DataLimits {
        file_size: text.len(),
        nodes: 5,
        depth: 3,
        container_size: 2,
        string_len: 3,
        payloads_bytes: 5,
        ..DataLimits::default()
    };
    assert!(parse_with_limits(&sources, id, exact).is_ok());
    for (name, limits) in [
        (
            "file_size",
            DataLimits {
                file_size: text.len() - 1,
                ..exact
            },
        ),
        ("nodes", DataLimits { nodes: 4, ..exact }),
        ("depth", DataLimits { depth: 2, ..exact }),
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
            "payloads_bytes",
            DataLimits {
                payloads_bytes: 4,
                ..exact
            },
        ),
    ] {
        let errors = parse_with_limits(&sources, id, limits).unwrap_err();
        assert!(errors[0].message.contains(name), "{errors:?}");
        assert_eq!(errors[0].labels[0].location.source, id);
    }
    // Decoded UTF-8 lengths, including keys and surrogate pairs, not token widths.
    let text = fixture("chunks.json");
    let id = sources.add("chunks.json", &text);
    let plan = parse_with_limits(&sources, id, DataLimits::default()).unwrap();
    let stats = plan
        .enforce_limits(DataLimits::default(), text.len())
        .unwrap();
    let limits = DataLimits {
        string_len: stats.string_len,
        payloads_bytes: stats.payloads_bytes,
        ..DataLimits::default()
    };
    assert!(parse_with_limits(&sources, id, limits).is_ok());
    assert!(
        parse_with_limits(
            &sources,
            id,
            DataLimits {
                payloads_bytes: stats.payloads_bytes - 1,
                ..limits
            }
        )
        .is_err()
    );
}

#[test]
fn quota_failure_stops_consuming_chunks() {
    let mut sources = SourceDatabase::default();
    let id = sources.add("quota.json", "\"xxxxxxxxmust not be scanned");
    let read = core::cell::Cell::new(0);
    let chunks = ["\"", "xxxxxxxx", "must not be scanned"]
        .into_iter()
        .inspect(|_| read.set(read.get() + 1));
    let error = super::super::parse::Parser::new(
        id,
        chunks,
        DataLimits {
            string_len: 4,
            ..DataLimits::default()
        },
    )
    .parse(29)
    .unwrap_err();
    assert!(error.message.contains("string_len"));
    assert_eq!(read.get(), 2);
}

#[test]
fn deep_and_wide_inputs_construct_and_drop_on_a_small_stack() {
    std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(|| {
            let mut sources = SourceDatabase::default();
            let text = format!("{}0{}", "[".repeat(20_000), "]".repeat(20_000));
            let id = sources.add("deep.json", &text);
            let errors = parse_with_limits(&sources, id, DataLimits::default()).unwrap_err();
            assert!(errors[0].message.contains("depth"));
            let plan = parse_with_limits(
                &sources,
                id,
                DataLimits {
                    depth: 20_001,
                    ..DataLimits::default()
                },
            )
            .unwrap();
            assert_eq!(plan.nodes().len(), 20_001);
            let nodes = plan.nodes().as_ptr();
            let plan = plan.into_postorder();
            assert_eq!(
                nodes,
                plan.nodes().as_ptr(),
                "already postordered: no traversal or allocation"
            );
            drop(plan);
            let text = format!("[{}0]", "0,".repeat(100_000));
            let id = sources.add("wide.json", &text);
            assert_eq!(
                parse_with_limits(&sources, id, DataLimits::default())
                    .unwrap()
                    .nodes()
                    .len(),
                100_002
            );
            let text = "[".repeat(20_000);
            let id = sources.add("unfinished.json", &text);
            assert!(
                parse_with_limits(
                    &sources,
                    id,
                    DataLimits {
                        depth: 20_001,
                        ..DataLimits::default()
                    }
                )
                .is_err()
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
