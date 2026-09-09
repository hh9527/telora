fn check_continuation(candidate: Val, return_result: bool) -> Box<ConstructionContinuation> {
    Box::new(ConstructionContinuation {
        return_result,
        candidate,
        return_target: ReturnTarget::Root,
        call_function: Arc::new(BytecodeFunction::new("check-test", 0, vec![], vec![])),
        call_pc: 0,
        trace_frame: RuntimeFrame {
            function: "@check".into(),
            instruction: 0,
            origin: None,
        },
    })
}

fn check_result(tag: BuiltinAtom, payload: CodecNode) -> CodecNode {
    CodecNode::Tagged {
        tag: Box::new(CodecNode::Atom(tag, None)),
        payload: Box::new(payload),
        loc: None,
    }
}

#[test]
fn construction_result_success_preserves_candidate() {
    let mut current = Heap::work();
    let background = Heap::main();
    let mut account = QuotaAccount::new(Quota::new(1_000, usize::MAX, u64::MAX));
    let candidate = Val::unknown(DecodedValue::Int(42));
    let result = materialize_codec_node(
        check_result(BuiltinAtom::Ok, CodecNode::Tuple(Vec::new(), None)),
        &mut current,
        &background,
    );
    for return_result in [false, true] {
        let action = check_continuation(candidate, return_result)
            .resume(result, &mut current, &background, &mut account)
            .unwrap();
        let VmAction::Return { value, .. } = action else {
            panic!("check must return its candidate");
        };
        if return_result {
            let view = HeapView {
                current: &current,
                background: Some(&background),
            };
            let (tag, payload) = (ValueRef { value, view }).tagged_parts().unwrap();
            assert_eq!(tag.as_atom().unwrap(), "Ok");
            assert_eq!(payload.runtime(), candidate);
        } else {
            assert_eq!(value, candidate);
        }
    }
}

#[test]
fn construction_result_rejection_preserves_error() {
    let mut current = Heap::work();
    let background = Heap::main();
    let mut account = QuotaAccount::new(Quota::new(1_000, usize::MAX, u64::MAX));
    let candidate = Val::unknown(DecodedValue::Int(42));
    let continuation = check_continuation(candidate, true);
    let blame = decode_blame(
        "candidate rejected".into(),
        vec![candidate],
        &continuation.call_function,
        0,
        &mut current,
        &mut account,
    )
    .unwrap();
    let result = materialize_codec_node(
        check_result(BuiltinAtom::Err, CodecNode::Existing(blame)),
        &mut current,
        &background,
    );
    let action = continuation
        .resume(result, &mut current, &background, &mut account)
        .unwrap();
    let VmAction::Return { value, .. } = action else {
        panic!("codec check must return its rejection");
    };
    let view = HeapView {
        current: &current,
        background: Some(&background),
    };
    let (tag, payload) = (ValueRef { value, view }).tagged_parts().unwrap();
    assert_eq!(tag.as_atom().unwrap(), "Err");
    assert_eq!(payload.runtime(), blame);
    let failure = check_continuation(candidate, false)
        .resume(result, &mut current, &background, &mut account)
        .err()
        .expect("ordinary construction must raise its rejection");
    assert_eq!(failure.kind, RuntimeErrorKind::RaisedBlame);
    assert_eq!(failure.message, "candidate rejected");

    let legacy = materialize_codec_node(
        check_result(BuiltinAtom::Some, CodecNode::Existing(blame)),
        &mut current,
        &background,
    );
    for return_result in [false, true] {
        let failure = check_continuation(candidate, return_result)
            .resume(legacy, &mut current, &background, &mut account)
            .err()
            .expect("legacy Some(BlameError) must not be a rejection result");
        assert_eq!(failure.kind, RuntimeErrorKind::TypeMismatch);
    }
}

#[test]
fn construction_result_rejects_malformed_values() {
    let mut current = Heap::work();
    let background = Heap::main();
    let mut account = QuotaAccount::new(Quota::new(1_000, usize::MAX, u64::MAX));
    let candidate = Val::unknown(DecodedValue::Int(42));
    for result in [
        CodecNode::Atom(BuiltinAtom::None, None),
        CodecNode::Atom(BuiltinAtom::Ok, None),
        CodecNode::Tuple(Vec::new(), None),
        check_result(BuiltinAtom::Some, CodecNode::Existing(candidate)),
        check_result(BuiltinAtom::Ok, CodecNode::Existing(candidate)),
        check_result(BuiltinAtom::Ok, CodecNode::Array(Vec::new(), None)),
        check_result(
            BuiltinAtom::Ok,
            CodecNode::Tuple(vec![CodecNode::Existing(candidate)], None),
        ),
        check_result(BuiltinAtom::Err, CodecNode::Existing(candidate)),
    ] {
        let value = materialize_codec_node(result, &mut current, &background);
        for return_result in [false, true] {
            let failure = check_continuation(candidate, return_result)
                .resume(value, &mut current, &background, &mut account)
                .err()
                .expect("malformed check result must be rejected");
            assert_eq!(failure.kind, RuntimeErrorKind::TypeMismatch);
            assert!(failure.message.contains("Result((), BlameError)"));
        }
    }
}
