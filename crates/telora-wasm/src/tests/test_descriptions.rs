use super::*;

#[test]
fn test_descriptions_preserve_identity_and_defer_callbacks() {
    let source = include_str!("../../tests/fixtures/test-descriptions.telora");
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
    let word = |bytes: &[u8], offset: usize| {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    };
    let memory = session.memory.data(&session.store);
    let id = word(memory, value + 16) as usize;
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
            session.memory.data(&session.store)[result + 16..result + 24]
                .try_into()
                .unwrap()
        ),
        42
    );
}

#[test]
fn test_description_rejects_empty_expectation_before_callback() {
    let source = include_str!("../../tests/fixtures/test-description-empty.telora");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    let failure = session.initialize().unwrap_err();
    assert!(failure.contains("should_fail_with requires a nonempty expectation"));
    let diagnostics = session.diagnostics().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].subjects[0][1] as usize,
        source.rfind("\"\"").unwrap()
    );
}
