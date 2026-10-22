//! Physical storage assertions accompany the standalone language fixture.
use crate::{abi::*, session::Session, transport::Value};

fn source() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/content-slices.telora"
    ))
    .unwrap()
}

fn content_len(session: &Session) -> u32 {
    session.output().word(u64::from(CONTENT_VIEW + 4)).unwrap()
}

#[test]
fn shared_raw_content_is_stable_during_sweep_and_compacts_at_explicit_boundaries() {
    let bytes = super::compile(&source()).unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let baseline = content_len(&session);
    let entry = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let string = session.manifest.types[entry.ty as usize].arguments[0];
    let text = "unused prefix|1234567890123456|tiny|abcdefghijklmnop|unused suffix";
    let argument = session.input_value(string, &text.into()).unwrap();
    let mut result = session.invoke_values(entry, &[argument]).unwrap();
    assert_eq!(
        content_len(&session),
        baseline + text.len() as u32 + 8,
        "split shares long views and inlines short ones without appending content"
    );
    // Force growth while every result still points to its original allocation.
    for size in [1000, 10000, 100000] {
        session
            .input_value(string, &"x".repeat(size).into())
            .unwrap();
    }
    let expected = serde_json::json!(["1234567890123456", "tiny", "abcdefghijklmnop"]);
    let pin = session.pin_value(result).unwrap();
    let raw = {
        let items = session.array_items(result).unwrap();
        session
            .output()
            .word(u64::from(items[0].pointer) + DATA)
            .unwrap()
    };
    for _ in 0..3 {
        session.collect_garbage().unwrap();
        let items = session.array_items(result).unwrap();
        assert_eq!(
            session
                .output()
                .word(u64::from(items[0].pointer) + DATA)
                .unwrap(),
            raw
        );
        assert_eq!(session.output_value(result).unwrap(), expected);
        assert_eq!(content_len(&session), baseline + text.len() as u32 + 8);
    }
    for _ in 0..3 {
        session.collect_work(&[]).unwrap();
        result = session.pinned_value(pin).unwrap();
        assert_eq!(session.output_value(result).unwrap(), expected);
        assert_eq!(
            content_len(&session),
            baseline + text.len() as u32 + 8,
            "retain the complete shared RawString including its header"
        );
        let output = session.output();
        let (base, _) = output
            .payload(ARRAYS, output.word(result.pointer as u64 + DATA).unwrap())
            .unwrap();
        let raw = output.word(base + DATA).unwrap();
        assert_eq!(
            raw,
            output
                .word(base + 2 * u64::from(STRING_BYTES) + DATA)
                .unwrap()
        );
        assert_eq!(
            output
                .bytes(base + u64::from(STRING_BYTES) + DATA + 15, 1)
                .unwrap(),
            [4]
        );
    }
    session.unpin_value(pin).unwrap();
    session.collect_work(&[]).unwrap();
    assert_eq!(content_len(&session), baseline);
}

#[test]
fn bytes_use_the_same_fifteen_byte_inline_boundary_as_strings() {
    let bytes = super::compile_export(&source(), "inline_bytes").unwrap();
    let mut session = Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let entry = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let baseline = content_len(&session);
    let result = session.invoke_values(entry, &[]).unwrap();
    let output = session.output();
    let (base, _) = output
        .payload(RECORDS, output.word(result.pointer as u64 + DATA).unwrap())
        .unwrap();
    for (index, expected) in [b"".as_slice(), b"123456789012345", b"1234567890123456"]
        .into_iter()
        .enumerate()
    {
        let pointer = base + index as u64 * u64::from(STRING_BYTES);
        assert_eq!(output.content_bytes(pointer).unwrap(), expected);
        assert_eq!(
            output.bytes(pointer + DATA + 15, 1).unwrap(),
            [expected.len() as u8]
        );
    }
    assert_eq!(content_len(&session), baseline + 24);
    let result = session.collect_work(&[result]).unwrap().0[0];
    assert!(
        session
            .output()
            .debug_repr(result.pointer as u64, result.ty)
            .is_ok()
    );
}

#[test]
fn inline_slice_does_not_retain_its_large_raw_parent() {
    let bytes = super::compile_export(&source(), "short_view").unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let baseline = content_len(&session);
    let entry = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let string = session.manifest.types[entry.ty as usize].arguments[0];
    let text = format!("{}|中文中文中|{}", "x".repeat(100_000), "y".repeat(100_000));
    let input = session.input_value(string, &text.into()).unwrap();
    let result = session.invoke_values(entry, &[input]).unwrap();
    let pin = session.pin_value(result).unwrap();
    assert!(content_len(&session) > baseline + 200_000);
    session.collect_garbage().unwrap();
    assert_eq!(session.output_value(result).unwrap(), "中文中文中");
    assert_eq!(content_len(&session), baseline);
    session.unpin_value(pin).unwrap();
}

fn append(session: &mut Session, left: Value, right: Value) -> Value {
    let pointer = session.allocate(STRING_BYTES as usize).unwrap();
    let append = session
        .instance
        .get_typed_func::<(u32, u32, u32), u32>(&session.store, "telora_content_append")
        .unwrap();
    session
        .guest_call(
            append,
            (
                pointer + DATA as u32,
                left.pointer + DATA as u32,
                right.pointer + DATA as u32,
            ),
        )
        .unwrap();
    Value {
        pointer,
        ty: left.ty,
    }
}

#[test]
fn raw_append_preserves_aliases_and_forks_frozen_and_historical_views() {
    let bytes = super::compile("pub def answer: String = \"0123456789abcdef\";").unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let original = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let suffix = session.input_value(original.ty, &"!".into()).unwrap();
    let raw = |session: &Session, value: Value| {
        session
            .output()
            .word(u64::from(value.pointer) + DATA)
            .unwrap()
    };
    let first = append(&mut session, original, suffix);
    assert_ne!(
        raw(&session, original),
        raw(&session, first),
        "frozen raw must fork"
    );
    let second = append(&mut session, first, suffix);
    assert_eq!(
        raw(&session, first),
        raw(&session, second),
        "unused tail capacity may be shared"
    );
    let other = session.input_value(original.ty, &"?".into()).unwrap();
    let branch = append(&mut session, first, other);
    assert_ne!(
        raw(&session, first),
        raw(&session, branch),
        "historical end must fork"
    );
    for (value, expected) in [
        (original, "0123456789abcdef"),
        (first, "0123456789abcdef!"),
        (second, "0123456789abcdef!!"),
        (branch, "0123456789abcdef!?"),
    ] {
        assert_eq!(session.output_value(value).unwrap(), expected);
    }
    let view = Value {
        pointer: session.allocate(STRING_BYTES as usize).unwrap(),
        ty: original.ty,
    };
    let slice = session
        .instance
        .get_typed_func::<(u32, u32, u32, u32), u32>(&session.store, "telora_content_slice")
        .unwrap();
    session
        .guest_call(
            slice,
            (
                view.pointer + DATA as u32,
                second.pointer + DATA as u32,
                2,
                16,
            ),
        )
        .unwrap();
    let grown = append(&mut session, view, other);
    assert_eq!(raw(&session, grown), raw(&session, second));
    assert_eq!(session.output_value(grown).unwrap(), "23456789abcdef!!?");
    assert_eq!(session.output_value(second).unwrap(), "0123456789abcdef!!");
}

#[test]
fn medium_slice_retains_the_complete_large_raw_until_its_last_view_dies() {
    let bytes = super::compile_export(&source(), "short_view").unwrap();
    let mut session = Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let baseline = content_len(&session);
    let entry = Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let string = session.manifest.types[entry.ty as usize].arguments[0];
    let text = format!(
        "{}|a medium long slice|{}",
        "x".repeat(100_000),
        "y".repeat(100_000)
    );
    let input = session.input_value(string, &text.clone().into()).unwrap();
    let result = session.invoke_values(entry, &[input]).unwrap();
    let pin = session.pin_value(result).unwrap();
    session.collect_garbage().unwrap();
    let result = session.pinned_value(pin).unwrap();
    assert_eq!(session.output_value(result).unwrap(), "a medium long slice");
    assert_eq!(content_len(&session), baseline + text.len() as u32 + 8);
    session.unpin_value(pin).unwrap();
    session.collect_garbage().unwrap();
    assert_eq!(content_len(&session), baseline);
}
