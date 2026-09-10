#[test]
fn solved_actor_state_keeps_the_original_payload_through_dyn_projection() {
    let mir = crate::codegen::tests::graph(
        r#"
        import "std/actor" as actor;
        import "std/value" {Value};
        import "std/dyn" as dyn;
        type State = struct(Array(Int));
        def input = State([42]);
        def service = actor.service(State.type, input, fn(state, event) { (state, []) });
        def transition = service.reduce((service.state, actor.Event.Request({id: "request", input: Value.None})));
        export def answer = (input, service.state, transition.0, dyn.project_with(State.type, transition.0), State.type);
    "#,
        "",
    );
    let artifact =
        crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new()
        .execute_linked(
            linked,
            Quota::with_fuel(10000),
            crate::DataLimits::default(),
            &mut SourceDatabase::default(),
        )
        .unwrap();
    let view = HeapView {
        current: &result.world.work.heap,
        background: Some(&result.world.main),
    };
    let DecodedValue::Tuple(handle) = result.world.work.root.value() else {
        panic!("tuple root");
    };
    let Object::Tuple(values) = view.object(handle).unwrap() else {
        panic!("tuple object");
    };
    for index in [1, 2] {
        let DecodedValue::Dyn(handle) = values[index].value() else {
            panic!("Dyn state");
        };
        assert!(
            values[index].type_id().is_none(),
            "Dyn must not inherit its payload's nominal stamp"
        );
        let (_, descriptor, payload) = view.dyn_parts(handle).unwrap();
        assert_eq!(descriptor.value(), values[4].value());
        assert_eq!(payload.value(), values[0].value());
        assert_eq!(payload.type_id(), values[0].type_id());
    }
    let DecodedValue::Tagged(handle) = values[3].value() else {
        panic!("Some projection");
    };
    let (_, payload) = view.tagged(handle).unwrap();
    assert_eq!(payload.value(), values[0].value());
    assert_eq!(payload.type_id(), values[0].type_id());
}

#[test]
fn solved_newtypes_preserve_nested_payload_handles_and_type_ids() {
    let mir = crate::codegen::tests::graph(
        "type Inner = struct(Array(Int)); type Outer = struct(Inner); def input = [20, 22]; def inner = Inner(input); export def answer = (input, inner, Outer(inner));",
        "",
    );
    let artifact =
        crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new()
        .execute_linked(
            linked,
            Quota::with_fuel(10000),
            crate::DataLimits::default(),
            &mut SourceDatabase::default(),
        )
        .unwrap();
    let view = HeapView {
        current: &result.world.work.heap,
        background: Some(&result.world.main),
    };
    let items = |value: Val| {
        let DecodedValue::Tuple(handle) = value.value() else {
            panic!("expected tuple container");
        };
        let Object::Tuple(items) = view.object(handle).unwrap() else {
            panic!("expected tuple object");
        };
        items
    };
    let values = items(result.world.work.root);
    let inner_payload = items(values[1])[0];
    let outer_payload = items(values[2])[0];
    assert_eq!(inner_payload.value(), values[0].value());
    assert_eq!(outer_payload.value(), values[1].value());
    assert_eq!(outer_payload.type_id(), values[1].type_id());
    assert!(
        values[1]
            .type_id()
            .and_then(crate::TypeId::solved_id)
            .is_some()
    );
    assert!(
        values[2]
            .type_id()
            .and_then(crate::TypeId::solved_id)
            .is_some()
    );
    assert_ne!(values[1].type_id(), values[2].type_id());
}

#[test]
fn solved_property_reads_reuse_vm_objects_and_failed_diagnostics() {
    use crate::execution_graph::{EvaluationError, Request};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    for fails in [false, true] {
        CALLS.store(0, Ordering::SeqCst);
        let body = if fails {
            "fail!(\"provider sentinel\")"
        } else {
            "{ value: [counted] }"
        };
        let mir = crate::codegen::tests::graph(
            &format!(
                r#"
            import "std/type-property" {{ get_type_prop as query }};
            native tick: Fn() -> Int;
            @property(PropertyTarget.Type) type Mark = struct {{ value: Array(Int) }};
            def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) {{ let counted = tick(); {body} }};
            @mark type Item = struct {{ value: Int }};
            export def answer = query(Item.type, Mark.type);
        "#
            ),
            "",
        );
        let mut artifact =
            crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir))
                .unwrap();
        let record = mir.properties.iter().find(|record| {
            matches!(mir.types[record.owner.index()].constructor,
                crate::mir::TypeConstructor::Nominal(symbol) if mir.symbols[symbol.index()].name == "Item")
        }).unwrap();
        let key = crate::execution_graph::PropertyKey {
            owner: record.owner,
            property: record.property,
            site: record.site,
        };
        let node = artifact.graph.property(key).unwrap();
        artifact.bytecode = crate::execution_link::link_with(&artifact, |_| {
            Some(NativeFunction::new("test.tick", 0, |ctx| {
                CALLS.fetch_add(1, Ordering::SeqCst);
                ctx.set_int(ctx.result(), 42)
            }))
        })
        .unwrap();
        artifact.native_links.clear();
        let linked = crate::execution_link::link_entry(artifact).unwrap();
        let mut main = Heap::main();
        main.solved_types = Some(linked.types);
        main.solved_graph = Some(linked.graph);
        let mut vm = Vm::new();
        let mut account = QuotaAccount::new(Quota::with_fuel(10000));
        let first = vm.execute_frame_with_policy(
            &main,
            &HashMap::new(),
            &linked.bytecode,
            None,
            None,
            &[],
            &[],
            &[],
            &mut account,
            false,
            0,
        );
        let (mut work, cached, failure_id) = match first {
            Ok(mut result) => {
                assert!(!fails);
                let Ok(Request::Ready(value)) = result
                    .world
                    .heap
                    .solved_evaluation
                    .as_mut()
                    .unwrap()
                    .request(node)
                else {
                    panic!("property not cached");
                };
                let cached = value.value();
                assert!(matches!(cached, DecodedValue::Dict(_)));
                (result.world.heap, Some(cached), None)
            }
            Err(mut failure) => {
                assert!(fails);
                assert!(failure.error.diagnostic().is_some());
                assert!(failure.error.to_string().contains("provider sentinel"));
                let Err(EvaluationError::Failed(id)) = failure
                    .heap
                    .solved_evaluation
                    .as_mut()
                    .unwrap()
                    .request(node)
                else {
                    panic!("failure not cached");
                };
                (failure.heap, None, Some(id))
            }
        };
        let read = BytecodeFunction::new(
            "required property read",
            3,
            vec![
                Constant::SolvedType(key.owner),
                Constant::SolvedType(key.property),
            ],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::GetTypeProp {
                    dst: Register(2),
                    owner: Register(0),
                    property: Register(1),
                },
                Instruction::Return { src: Register(2) },
            ],
        );
        for _ in 0..3 {
            match vm.execute_frame_with_policy(
                &main,
                &HashMap::new(),
                &read,
                Some(work),
                None,
                &[],
                &[],
                &[],
                &mut account,
                false,
                0,
            ) {
                Ok(result) => {
                    assert!(!fails);
                    // Exact heap handle equality, not structural value equality:
                    // each read returns the original object without relocation.
                    assert_eq!(Some(result.world.root.value()), cached);
                    work = result.world.heap;
                }
                Err(failure) => {
                    assert!(fails);
                    assert_eq!(failure.error.propagated_failure, failure_id.map(|id| id.0));
                    assert!(failure.error.diagnostic().is_none());
                    assert_eq!(failure.heap.solved_failures.len(), 1);
                    work = failure.heap;
                }
            }
            assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        }
        // A required read without static presence is malformed bytecode;
        // it must never manufacture the source language's Option.None.
        let absent = BytecodeFunction::new(
            "absent required property",
            2,
            vec![Constant::SolvedType(key.property)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::GetTypeProp {
                    dst: Register(1),
                    owner: Register(0),
                    property: Register(0),
                },
                Instruction::Return { src: Register(1) },
            ],
        );
        let failure = match vm.execute_frame_with_policy(
            &main,
            &HashMap::new(),
            &absent,
            Some(work),
            None,
            &[],
            &[],
            &[],
            &mut account,
            false,
            0,
        ) {
            Ok(_) => panic!("invalid required read succeeded"),
            Err(failure) => failure,
        };
        assert!(matches!(
            failure.error.kind,
            RuntimeErrorKind::InvalidBytecode
        ));
        assert!(failure.error.to_string().contains("proven presence"));
    }
}
#[test]
fn solved_parse_blame_retains_the_original_input_handle_and_location() {
    let mir = crate::codegen::tests::graph("import \"std/json\" as json; def input = \"{\"; export def answer = (input, json.parse(input));", "");
    let original_location = mir.hir.iter().find(|node| matches!(&node.kind, crate::mir::HirKind::String(text) if text == "{")).unwrap().location;
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let result = Vm::new().execute_linked(crate::execution_link::link_entry(artifact).unwrap(), Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    let view = HeapView { current: &result.world.work.heap, background: Some(&result.world.main) };
    let root = result.world.value();
    let input = root.sequence_get(0).unwrap().runtime();
    let blame = root.sequence_get(1).unwrap().tagged_parts().unwrap().1.runtime();
    let DecodedValue::Opaque(handle) = blame.value() else { panic!("BlameError") };
    let Object::Opaque(blame) = view.object(handle).unwrap() else { panic!("BlameError object") };
    let original = blame.traced[0];
    assert!(original.type_id().and_then(crate::TypeId::solved_id).is_some());
    let DecodedValue::Tagged(handle) = original.value() else { panic!("Value.String") };
    let (_, payload) = view.tagged(handle).unwrap();
    assert_eq!(payload.value(), input.value());
    // Reading `input` for the returned tuple can carry its own reference-site
    // location; Blame retains the parser argument's original literal provenance.
    assert_eq!(payload.loc(), Some(original_location));
    assert_eq!(original.loc(), payload.loc());
}
#[test]
fn solved_parse_obeys_the_session_data_limits() {
    let mir = crate::codegen::tests::graph("import \"std/json\" as json; export def answer = json.parse(\"123\");", "");
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let limits = crate::DataLimits { file_size: 2, ..crate::DataLimits::default() };
    let result = Vm::new().execute_linked(crate::execution_link::link_entry(artifact).unwrap(), Quota::with_fuel(10000), limits, &mut SourceDatabase::default());
    assert!(result.err().unwrap().contains("file_size limit"));
}
