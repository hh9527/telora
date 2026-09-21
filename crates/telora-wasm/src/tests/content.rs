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
fn shared_content_moves_once_and_short_views_release_their_parent() {
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
        baseline + text.len() as u32,
        "split shares long views and inlines short ones without appending content"
    );
    // Force growth while every result still points to its original allocation.
    for size in [1000, 10000, 100000] {
        session
            .input_value(string, &"x".repeat(size).into())
            .unwrap();
    }
    let expected = serde_json::json!(["1234567890123456", "tiny", "abcdefghijklmnop"]);
    for _ in 0..3 {
        result = session.collect_work(&[result]).unwrap().0[0];
        assert_eq!(session.output_value(result).unwrap(), expected);
        assert_eq!(
            content_len(&session),
            baseline + 38,
            "copy the live envelope once, trim head/tail, retain the interior gap"
        );
        let output = session.output();
        let (base, _) = output
            .payload(ARRAYS, output.word(result.pointer as u64 + DATA).unwrap())
            .unwrap();
        let raw = output.word(base + DATA + 8).unwrap();
        assert_eq!(
            raw,
            output
                .word(base + 2 * u64::from(STRING_BYTES) + DATA + 8)
                .unwrap()
        );
        assert_eq!(
            output
                .bytes(base + u64::from(STRING_BYTES) + DATA + 15, 1)
                .unwrap(),
            [4]
        );
    }
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
    assert_eq!(content_len(&session), baseline + 16);
    let result = session.collect_work(&[result]).unwrap().0[0];
    assert!(
        session
            .output()
            .debug_repr(result.pointer as u64, result.ty)
            .is_ok()
    );
}
