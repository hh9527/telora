use crate::{abi::*, gc::GcConfig, session::Session, transport::Value};

#[test]
fn sparse_filter_allocates_by_matches_and_publishes_empty_and_single() {
    let source = r#"
use std::array;
pub def answer: Fn(Array(Int)) -> (Array(Int), Array(Int)) = fn(xs) {
    (array::filter(xs, fn(x) { x < 0 }), array::filter(xs, fn(x) { x == 0 }))
};
"#;
    let bytes = super::compile(source).unwrap();
    let mut costs = Vec::new();
    let mut retained = Vec::new();
    for count in [32, 4096] {
        let mut session = Session::load(&bytes, 100_000_000).unwrap();
        session
            .configure_gc(GcConfig {
                allocation_interval: 0,
                ..Default::default()
            })
            .unwrap();
        session.initialize().unwrap();
        let signature = session.manifest.types[session.manifest.entry_type as usize]
            .arguments
            .clone();
        let input = session
            .input_value(
                signature[0],
                &serde_json::json!((0..count).collect::<Vec<_>>()),
            )
            .unwrap();
        let before = session.gc_stats().unwrap().allocated_total;
        let entry = Value {
            pointer: session.entry().unwrap(),
            ty: session.manifest.entry_type,
        };
        let result = session.invoke_values(entry, &[input]).unwrap();
        costs.push(session.gc_stats().unwrap().allocated_total - before);
        assert_eq!(
            session.output_value(result).unwrap(),
            serde_json::json!([[], [0]])
        );
        let output = session.output();
        let desc = &session.manifest.types[result.ty as usize];
        let (base, _) = output
            .aggregate_payload(result.pointer.into(), desc)
            .unwrap();
        for (field, kind) in desc.fields.iter().zip([ARRAY_EMPTY, ARRAY_SINGLE]) {
            let pointer = base + u64::from(field.offset);
            assert_eq!(output.word(pointer + DATA + 12).unwrap(), kind);
            if kind == ARRAY_EMPTY {
                assert_eq!(output.word(pointer + DATA).unwrap(), 0);
            }
        }
        let pin = session.pin_value(result).unwrap();
        session.collect_garbage().unwrap();
        let result = session.pinned_value(pin).unwrap();
        assert_eq!(
            session.output_value(result).unwrap(),
            serde_json::json!([[], [0]])
        );
        retained.push(session.gc_stats().unwrap().managed_bytes);
        session.collect_work(&[]).unwrap();
        let result = session.pinned_value(pin).unwrap();
        assert_eq!(
            session.output_value(result).unwrap(),
            serde_json::json!([[], [0]])
        );
    }
    assert!(
        retained[1] <= retained[0] + 1024,
        "Single retained its large input: {retained:?}"
    );
    assert!(
        costs[1] <= costs[0] + 1024,
        "sparse filter output grew with input: {costs:?}"
    );
}

#[test]
fn singleton_reference_elements_survive_forced_gc_and_branching_push() {
    let source = r#"
use std::array;
use std::seq;
pub def answer: Fn(Array(String)) -> (Array(String), Array(String), Array(String), Array(String)) = fn(xs) {
    let single = array::filter(xs, fn(x) { x == "the selected long string" });
    let left = array::push(single, "left branch long string");
    let right = array::push(single, "right branch long string");
    (single, left, right, seq::collect(seq::from_array(single)))
};
"#;
    let bytes = super::compile(source).unwrap();
    for interval in [0, 1] {
        let mut session = Session::load(&bytes, 100_000_000).unwrap();
        session
            .configure_gc(GcConfig {
                allocation_interval: interval,
                ..Default::default()
            })
            .unwrap();
        session.initialize().unwrap();
        let ty = session.manifest.types[session.manifest.entry_type as usize].arguments[0];
        let input = session
            .input_value(
                ty,
                &serde_json::json!([
                    "unselected long string",
                    "the selected long string",
                    "another unselected long string"
                ]),
            )
            .unwrap();
        let entry = Value {
            pointer: session.entry().unwrap(),
            ty: session.manifest.entry_type,
        };
        let result = session.invoke_values(entry, &[input]).unwrap();
        let expected = serde_json::json!([
            ["the selected long string"],
            ["the selected long string", "left branch long string"],
            ["the selected long string", "right branch long string"],
            ["the selected long string"]
        ]);
        assert_eq!(session.output_value(result).unwrap(), expected);
        let pin = session.pin_value(result).unwrap();
        for _ in 0..3 {
            session.collect_garbage().unwrap();
            session.collect_work(&[]).unwrap();
            let result = session.pinned_value(pin).unwrap();
            assert_eq!(session.output_value(result).unwrap(), expected);
        }
    }
}

#[test]
fn push_uses_raw_end_for_nonzero_start_and_preserves_the_original_view() {
    let bytes = super::compile(
        r#"
use std::array;
pub def answer: Fn(Array(Int)) -> (Array(Int), Array(Int), Array(Int)) = fn(xs) {
    (xs, array::push(xs, 4), array::push(xs, 5))
};
"#,
    )
    .unwrap();
    let mut session = Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let ty = session.manifest.types[session.manifest.entry_type as usize].arguments[0];
    let input = session
        .input_value(ty, &serde_json::json!([1, 2, 3]))
        .unwrap();
    session
        .write(
            input.pointer as usize + (DATA + 4) as usize,
            &1u32.to_le_bytes(),
        )
        .unwrap();
    let entry = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let result = session.invoke_values(entry, &[input]).unwrap();
    assert_eq!(
        session.output_value(result).unwrap(),
        serde_json::json!([[2, 3], [2, 3, 4], [2, 3, 5]])
    );
}

#[test]
fn parser_republishes_empty_arrays_after_nonempty_iterations() {
    let bytes = super::compile(r##"
use std::codec;
use std::json;
pub def answer: Fn() -> Array(Array(String)) = fn() {
    codec::decode@[Array(Array(String))](json::parse(
        r#"[["one long reference string"],[],["second long reference string","third long reference string"],[],["last long reference string"],[]]"#
    ).unwrap!()).unwrap!()
};
"##).unwrap();
    let expected = serde_json::json!([
        ["one long reference string"],
        [],
        [
            "second long reference string",
            "third long reference string"
        ],
        [],
        ["last long reference string"],
        []
    ]);
    for interval in [0, 1, 32 * 1024] {
        let mut session = Session::load(&bytes, 100_000_000).unwrap();
        session
            .configure_gc(GcConfig {
                allocation_interval: interval,
                ..Default::default()
            })
            .unwrap();
        session.initialize().unwrap();
        assert_eq!(session.call(&[]).unwrap(), expected);
        session.collect_garbage().unwrap();
        assert_eq!(session.call(&[]).unwrap(), expected);
    }
}
