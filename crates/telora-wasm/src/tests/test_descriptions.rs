use super::*;

#[test]
fn test_boundary_recovers_language_failures_but_never_traps() {
    use crate::testing::{Description, TestSession};
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/test-execution.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    let mut testing = TestSession::new(session).unwrap();
    let entry = testing.entry().unwrap();
    let root = testing
        .session()
        .manifest
        .globals
        .iter()
        .find(|global| global.ty == entry.type_id())
        .unwrap()
        .symbol;
    let global = testing.session().initialized_global(root).unwrap();
    assert_eq!(global.pointer, entry.pointer);
    let items = testing.session().output();
    let desc = &testing.session().manifest.types[entry.type_id() as usize];
    let (base, _) = items
        .payload(
            crate::abi::RECORDS,
            items.word(entry.pointer as u64 + crate::abi::DATA).unwrap(),
        )
        .unwrap();
    let values = desc
        .fields
        .iter()
        .map(|field| crate::transport::Value {
            pointer: base as u32 + field.offset,
            ty: field.ty,
        })
        .collect::<Vec<_>>();
    let Description::ShouldFailWith(failure, expected) = testing.describe(values[0]).unwrap().kind
    else {
        panic!("expected failure");
    };
    let Description::ShouldOk(success) = testing.describe(values[1]).unwrap().kind else {
        panic!("success");
    };
    let Description::ShouldFail(diverge) = testing.describe(values[2]).unwrap().kind else {
        panic!("diverge");
    };
    let failed = testing.invoke(failure, &[]).unwrap();
    assert!(failed.value.is_none());
    assert!(failed.terminal.is_none());
    assert_eq!(failed.diagnostics.len(), 1);
    assert_eq!(failed.diagnostics[0].message, expected);
    let passed = testing.invoke(success, &[]).unwrap();
    assert!(passed.terminal.is_none());
    assert!(passed.diagnostics.is_empty());
    assert_eq!(
        testing
            .session()
            .output_value(passed.value.unwrap())
            .unwrap(),
        42
    );
    let Description::Fixtures { sources, factory } = testing.describe(values[3]).unwrap().kind
    else {
        panic!("fixtures");
    };
    assert_eq!(sources, ["first.json", "second.json"]);
    let input_ty = testing.session().manifest.types[factory.type_id() as usize].arguments[0];
    let data = testing
        .session_mut()
        .input_value(input_ty, &serde_json::json!({"n": 42}))
        .unwrap();
    let expansion = testing.invoke(factory, &[data]).unwrap();
    assert!(expansion.terminal.is_none());
    let Description::ShouldOk(callback) = testing.describe(expansion.value.unwrap()).unwrap().kind
    else {
        panic!("factory result");
    };
    let captured = testing.invoke(callback, &[]).unwrap().value.unwrap();
    assert_eq!(
        testing.session().output_value(captured).unwrap(),
        serde_json::json!({"n": 42})
    );
    let exhausted = testing.invoke(diverge, &[]).unwrap();
    assert!(exhausted.value.is_none());
    assert!(exhausted.terminal.is_some());
    assert!(
        testing
            .invoke(success, &[])
            .unwrap_err()
            .contains("terminated")
    );
}

#[test]
fn test_descriptions_preserve_identity_and_defer_callbacks() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/language/src/test/runtime-language/test-descriptions.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([true, true, true, true, true])
    );
    assert!(session.diagnostics().unwrap().is_empty());
    let bytes = compile_export(source, "captured").unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    let value = session.entry().unwrap() as usize;
    let word = |output: &crate::output::Output<'_>, offset: usize| output.word(offset as u64).unwrap();
    let output = session.output();
    let memory = &output;
    let id = word(memory, value + crate::abi::DATA as usize) as usize;
    let table = crate::abi::table_address(crate::abi::TESTS) as usize;
    let slot = word(memory, table) as usize + id * 8;
    let description = word(memory, slot) as usize;
    assert_eq!(word(memory, slot + 4), 12);
    assert_eq!(word(memory, description), 0);
    assert_eq!(word(memory, description + 4), 1);
    let callback = word(memory, description + 8);
    let invoke = session
        .instance
        .get_typed_func::<(i32, i32), i32>(&session.store, "telora_invoke")
        .unwrap();
    let result = invoke
        .call(&mut session.store, (callback as i32, 0))
        .unwrap() as usize;
    assert_ne!(result, 0);
    assert_eq!(
        i64::from_le_bytes(
            session.output().bytes(result as u64 + crate::abi::DATA, 8).unwrap()
                .try_into()
                .unwrap()
        ),
        42
    );
}

#[test]
fn test_description_rejects_empty_expectation_before_callback() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/test-description-empty.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    let failure = session.initialize().unwrap_err();
    assert!(failure.contains("should_fail_with requires a nonempty expectation"));
    let diagnostics = session.diagnostics().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        telora_core::source::SourceCoordinates(diagnostics[0].subjects[0]).start(),
        point(source, source.rfind("\"\"").unwrap())
    );
}
