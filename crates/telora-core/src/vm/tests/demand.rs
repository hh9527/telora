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
            true,
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
                true,
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
            true,
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
#[test]
fn solved_codec_encode_preserves_string_and_existing_value_handles() {
    let mir = crate::codegen::tests::graph(r#"
        import "std/codec" as codec;
        def input = "a string deliberately longer than the inline representation capacity";
        def encoded = codec.encode(codec.Value.type, input);
        export def answer = (input, encoded, codec.encode(codec.Value.type, encoded));
    "#, "");
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new().execute_linked(linked, Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    let view = HeapView { current: &result.world.work.heap, background: Some(&result.world.main) };
    let root = ValueRef { value: result.world.work.root, view };
    let input = root.sequence_get(0).unwrap();
    let encoded = root.sequence_get(1).unwrap();
    assert_eq!(encoded.tagged_parts().unwrap().1.value.value(), input.value.value());
    assert_eq!(root.sequence_get(2).unwrap().value.value(), encoded.value.value());
}

#[test]
fn solved_dyn_member_access_preserves_payload_handles() {
    let mir = crate::codegen::tests::graph(r#"
        import "std/dyn" as dyn;
        type Box(T) = struct { value: T };
        type Item(T) = enum { Empty, Full(T) };
        def original = ["a string deliberately longer than inline storage capacity"];
        def boxed: Box(Array(String)) = {value: original};
        def field = dyn.get_field_value(dyn.pack(Box(Array(String)).type, boxed), 0);
        def candidate: Unchecked(Box(Array(String))) = {value: original};
        def candidate_field = dyn.get_field_value(dyn.pack(Unchecked(Box(Array(String))).type, candidate), 0);
        def variant = match dyn.get_variant_payload(dyn.pack(Item(Array(String)).type, Item.Full(original)), 1) { Some(value) => value, None => fail!("missing payload") };
        export def answer = (original, field, variant, candidate_field);
    "#, "");
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    drop(mir);
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new().execute_linked(linked, Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    let view = HeapView { current: &result.world.work.heap, background: Some(&result.world.main) };
    let root = result.value();
    let original = root.sequence_get(0).unwrap().value;
    for index in [1, 2, 3] {
        let DecodedValue::Dyn(handle) = root.sequence_get(index).unwrap().value.value() else { panic!("Dyn child") };
        let (_, descriptor, payload) = view.dyn_parts(handle).unwrap();
        assert_eq!(payload.value(), original.value());
        let DecodedValue::SolvedType(ty) = descriptor.value() else { panic!("solved witness") };
        assert_eq!(result.world.main.solved_types.as_ref().unwrap().types[ty.index()].constructor, crate::mir::TypeConstructor::Array);
    }
}

#[test]
fn solved_test_session_runs_cases_after_cached_and_expected_failures() {
    let mut mir = crate::codegen::tests::graph(r#"
        import "std/test" as test;
        def broken: Int = fail!("cached dependency failure");
        export def a_expected = test.should_fail_with(fn() { broken }, "cached dependency");
        export def b_cached = test.should_fail_with(fn() { broken }, "cached dependency");
        export def c_success = test.should_ok(fn() {42});
        export def d_wrong = test.should_fail(fn() {42});
        export def e_initializer: test.Test = fail!("bad initializer");
        export def f_after = test.should_ok(fn() {42});
    "#, "");
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    let crate::mir::ModuleTarget::Bound(module) = mir.roots[0] else { panic!("root"); };
    let compiled = crate::codegen::compile_tests(mir.seal().unwrap(), module).unwrap();
    let linked = crate::execution_link::link_entry(compiled.bootstrap).unwrap();
    let report = Vm::new().test_linked(linked, compiled.plan, Quota::with_fuel(10000), crate::DataLimits::default(), &mut mir.sources, crate::TestContext::default()).unwrap();
    assert!(!report.aborted, "{report:?}");
    assert_eq!(report.cases.iter().map(|case| case.passed).collect::<Vec<_>>(), [true, true, true, false, false, true], "{report:?}");
    assert!(report.cases[0].diagnostics.is_empty(), "{report:?}");
    assert!(report.cases[1].diagnostics.is_empty(), "{report:?}");
    assert_eq!(report.cases[4].phase, "initialization");
}

#[test]
fn solved_test_session_does_not_accept_terminal_failures_as_expected() {
    let mut mir = crate::codegen::tests::graph(r#"
        import "std/test" as test;
        def loop: Fn() -> Int = fn() { loop() };
        export def a_exhaust = test.should_fail(fn() {loop()});
        export def b_after = test.should_ok(fn() {42});
    "#, "");
    let crate::mir::ModuleTarget::Bound(module) = mir.roots[0] else { panic!("root"); };
    let compiled = crate::codegen::compile_tests(mir.seal().unwrap(), module).unwrap();
    let linked = crate::execution_link::link_entry(compiled.bootstrap).unwrap();
    let report = Vm::new().test_linked(linked, compiled.plan, Quota::with_fuel(1000), crate::DataLimits::default(), &mut mir.sources, crate::TestContext::default()).unwrap();
    assert!(report.aborted, "{report:?}");
    assert_eq!(report.cases.len(), 1);
    assert!(!report.cases[0].passed);
}

#[test]
fn solved_unchecked_completion_preserves_the_candidate_handle() {
    let mir = crate::codegen::tests::graph(r#"
        @check(fn(value) { Ok(()) }) type Item = struct {text: String};
        def candidate: Unchecked(Item) = {text: "shared"};
        def checked: Item = candidate;
        export def answer = (candidate, checked);
    "#, "");
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let expected = artifact.types.types[artifact.result_type.index()].arguments.clone();
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new().execute_linked(linked, Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    let candidate = result.value().sequence_get(0).unwrap();
    let checked = result.value().sequence_get(1).unwrap();
    assert_eq!(candidate.value.value(), checked.value.value());
    assert_ne!(expected[0], expected[1]);
    assert_eq!(candidate.solved_type_id(), Some(expected[0]));
    assert_eq!(checked.solved_type_id(), Some(expected[1]));
    assert_eq!(candidate.dict_get("text").unwrap().value.loc(), checked.dict_get("text").unwrap().value.loc());
}

#[derive(Default)]
struct SolvedFixtureHost {
    reads: Vec<String>,
}

impl crate::TestHost for SolvedFixtureHost {
    fn resolve(&mut self, module: &str, _: Option<&std::path::Path>, source: &str) -> Result<crate::TestSource, String> {
        assert!(!module.starts_with("@test-ctx/"), "factory origin must remain its declaring module");
        Ok(crate::TestSource { key: source.into(), format: crate::SystemDataFormat::Json })
    }
    fn read(&mut self, source: &crate::TestSource, _: usize) -> Result<String, String> {
        self.reads.push(source.key.clone());
        Ok(if source.key == "bad" { "{" } else { "42" }.into())
    }
}

fn solved_fixture_report(source: &str, host: &mut dyn crate::TestHost, limits: crate::TestLimits)
    -> (crate::test_plan::TestReport, SourceDatabase) {
    let mut mir = crate::codegen::tests::graph(source, "");
    assert!(mir.diagnostics.is_empty(), "{}", mir.diagnostics.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n"));
    let crate::mir::ModuleTarget::Bound(module) = mir.roots[0] else { panic!("root"); };
    let compiled = crate::codegen::compile_tests(mir.seal().unwrap(), module).unwrap();
    assert!(compiled.plan.fixture_type.is_some(), "fixture input must be statically known");
    let linked = crate::execution_link::link_entry(compiled.bootstrap).unwrap();
    let report = Vm::new().test_linked(linked, compiled.plan, Quota::with_fuel(100_000),
        crate::DataLimits::default(), &mut mir.sources,
        crate::TestContext { host: Some(host), limits, ..Default::default() }).unwrap();
    (report, mir.sources)
}

#[test]
fn solved_test_fixtures_cache_inputs_keep_provenance_and_continue_after_invalid_data() {
    let mut host = SolvedFixtureHost::default();
    let (report, sources) = solved_fixture_report(r#"
        import "std/test" as test;
        import "std/value" {Value};
        export def cases = test.with_fixtures(["one", "bad", "one"], fn(value) {
            test.should_ok(fn() { match value { Value.Int(n) => n + 1, _ => fail!("wrong fixture type") } })
        });
    "#, &mut host, crate::TestLimits::default());
    assert!(!report.aborted, "{report:?}");
    assert_eq!(host.reads, ["one", "bad"]);
    assert_eq!(report.cases.iter().map(|c| c.passed).collect::<Vec<_>>(), [true, false, true], "{report:?}");
    assert_eq!(report.cases[1].phase, "fixture");
    assert_eq!(report.cases[2].fixtures, [2]);
    assert_eq!(report.cases[2].sources, ["one"]);
    let label = &report.cases[1].diagnostics[0].labels[0];
    assert!(sources.get(label.location.source).name.starts_with("@test-ctx/"));
    assert!(sources.get(label.location.source).name.ends_with("/cases/1"));
}

#[test]
fn solved_test_fixtures_expand_depth_first_and_recover_from_factory_failure() {
    let mut host = SolvedFixtureHost::default();
    let (report, _) = solved_fixture_report(r#"
        import "std/test" as test;
        export def a_nested = test.with_fixtures(["outer", "outer"], fn(outer) {
            test.with_fixtures(["inner"], fn(inner) { test.should_ok(fn() { (outer, inner) }) })
        });
        export def b_failed = test.with_fixtures(["failure", "failure"], fn(value) { fail!("factory failed") });
        export def c_after = test.should_ok(fn() { 42 });
    "#, &mut host, crate::TestLimits::default());
    assert!(!report.aborted, "{report:?}");
    assert_eq!(report.cases.iter().map(|c| c.passed).collect::<Vec<_>>(), [true, true, false, false, true], "{report:?}");
    assert_eq!(report.cases[0].fixtures, [0, 0]);
    assert_eq!(report.cases[1].fixtures, [1, 0]);
    assert_eq!(report.cases[1].sources, ["outer", "inner"]);
    assert_eq!(report.cases[2].phase, "factory");
    assert_eq!(host.reads, ["outer", "inner", "inner", "failure"]);
}

#[test]
fn solved_test_fixtures_enforce_expansion_and_retained_budgets() {
    let code = r#"
        import "std/test" as test;
        def group: Fn() -> test.Test = fn() { test.with_fixtures(["one"], fn(value) { group() }) };
        export def cases = group();
    "#;
    for limits in [
        crate::TestLimits { depth: 3, ..Default::default() },
        crate::TestLimits { cases: 2, ..Default::default() },
        crate::TestLimits { fixture_bytes: 1, ..Default::default() },
    ] {
        let (report, _) = solved_fixture_report(code, &mut SolvedFixtureHost::default(), limits);
        assert!(report.aborted, "{report:?}");
        assert_eq!(report.cases.len(), 1);
        assert!(!report.cases[0].passed);
        assert!(report.cases[0].diagnostics.iter().any(|d| d.message.contains("limit") || d.message.contains("budget")), "{report:?}");
    }
}

#[test]
fn solved_test_fixtures_prepare_all_inputs_before_factories_and_keep_group_notices() {
    struct Observer(Arc<std::sync::Mutex<Vec<String>>>);
    impl crate::DebugSink for Observer {
        fn emit(&self, _: crate::DebugEvent) { self.0.lock().unwrap().push("factory".into()); }
    }
    impl crate::TestHost for Observer {
        fn resolve(&mut self, _: &str, _: Option<&std::path::Path>, source: &str) -> Result<crate::TestSource, String> {
            Ok(crate::TestSource { key: source.into(), format: crate::SystemDataFormat::Json })
        }
        fn read(&mut self, source: &crate::TestSource, _: usize) -> Result<String, String> {
            self.0.lock().unwrap().push(source.key.clone());
            Ok("42".into())
        }
    }
    let mut mir = crate::codegen::tests::graph(r#"
        import "std/test" as test;
        export def cases = test.with_fixtures(["a", "b", "a"], fn(value) {
            let observed = dbg!(value);
            let warning: Option(Int) = warn!("factory warning");
            test.should_ok(fn() { observed })
        });
    "#, "");
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let crate::mir::ModuleTarget::Bound(module) = mir.roots[0] else { panic!("root"); };
    let compiled = crate::codegen::compile_tests(mir.seal().unwrap(), module).unwrap();
    let linked = crate::execution_link::link_entry(compiled.bootstrap).unwrap();
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut host = Observer(events.clone());
    let report = Vm::new().with_debug_sink(Arc::new(Observer(events.clone())))
        .test_linked(linked, compiled.plan, Quota::with_fuel(100_000), crate::DataLimits::default(),
            &mut mir.sources, crate::TestContext { host: Some(&mut host), ..Default::default() }).unwrap();
    assert!(!report.aborted && report.cases.iter().all(|c| c.passed), "{report:?}");
    assert_eq!(*events.lock().unwrap(), ["a", "b", "factory", "factory", "factory"]);
    assert_eq!(report.notices.len(), 3, "{report:?}");
    for (index, notice) in report.notices.iter().enumerate() {
        assert_eq!(notice.before_case, index);
        assert_eq!(notice.context.fixtures, [index]);
        assert_eq!(notice.context.phase, "factory");
        assert!(notice.context.diagnostics.iter().any(|d| d.message.contains("factory warning")));
        assert!(report.cases[index].diagnostics.is_empty());
    }
}

#[test]
fn solved_cast_preserves_data_handles_and_nominal_identity() {
    let mir = crate::codegen::tests::graph(r#"
        type Item = struct {text: String};
        def raw = {text: "original"};
        def typed: Item = {text: "checked"};
        export def answer = (raw, raw.cast!(Item), typed, typed.cast!(Item), [raw].cast!(Array(Item)));
    "#, "");
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let expected = artifact.types.types[artifact.result_type.index()].arguments[2];
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new().execute_linked(linked, Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    let raw = result.value().sequence_get(0).unwrap();
    let cast = result.value().sequence_get(1).unwrap().tagged_parts().unwrap().1;
    let typed = result.value().sequence_get(2).unwrap();
    let same = result.value().sequence_get(3).unwrap().tagged_parts().unwrap().1;
    let nested = result.value().sequence_get(4).unwrap().tagged_parts().unwrap().1.sequence_get(0).unwrap();
    assert_eq!(raw.value.value(), cast.value.value());
    assert_eq!(typed.value.value(), same.value.value());
    assert_eq!(raw.value.value(), nested.value.value());
    assert_eq!(cast.solved_type_id(), Some(expected));
    assert_eq!(nested.solved_type_id(), Some(expected));
    let raw_text = raw.dict_get("text").unwrap();
    for value in [cast, nested] {
        let text = value.dict_get("text").unwrap();
        assert_eq!(text.value.value(), raw_text.value.value());
        assert_eq!(text.value.loc(), raw_text.value.loc());
    }
}

#[test]
fn solved_codec_construction_rejection_retains_the_original_blame_handle() {
    let mir = crate::codegen::tests::graph(r#"
        import "std/codec" as codec;
        def original = blame!("rejected", 0);
        @check(fn(value) { Err(original) }) type Item = struct(Int);
        export def answer = (original, codec.decode(Item.type, codec.Value.Int(0)));
    "#, "");
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new().execute_linked(linked, Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    let original = result.value().sequence_get(0).unwrap();
    let (tag, rejection) = result.value().sequence_get(1).unwrap().tagged_parts().unwrap();
    assert_eq!(tag.as_atom().unwrap().as_str(), "Err");
    assert_eq!(original.value.value(), rejection.value.value());
    assert!(result.world.work.heap.solved_failures.is_empty());
}

#[test]
fn solved_string_parse_reuses_whole_input_captures_and_error_subjects() {
    let source = r#"
        import "std/string" as string; import "std/regex" as regex;
        @regex.parse_by(regex.compile(r"^(?P<value>.*)$")) type Item = struct { value: String };
        def input = "a string deliberately longer than inline storage capacity";
        def parsed = match string.parse(Item.type, input) { Ok(value) => value, Err(_) => fail!("parse") };
        def rejected = match string.parse(Int.type, input) { Err(error) => error, Ok(_) => fail!("expected rejection") };
        export def answer = (input, parsed, rejected);
    "#;
    let mir = crate::codegen::tests::graph(source, "");
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    drop(mir);
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new().execute_linked(linked, Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    let input = result.value().sequence_get(0).unwrap().value;
    for index in [1, 2] {
        let value = result.value().sequence_get(index).unwrap().dict_get("value").unwrap().value;
        assert_eq!(value.value(), input.value());
        let loc = value.loc().expect("original input literal location");
        assert_eq!(&source[loc.start as usize..loc.end as usize], "\"a string deliberately longer than inline storage capacity\"");
    }
}

#[test]
fn solved_codec_display_calls_fail_per_value_without_poisoning_provider() {
    let mir = crate::codegen::tests::graph(r#"
        import "std/codec" as codec; import "std/_rt" as rt;
        import "std/string" as string; import "std/fmt" as fmt;
        def broken: Fn(Type, Option(fmt.DisplayBy)) -> fmt.DisplayBy = fn(owner, previous) {
            {template: {strings: [], fields: []}, display: fn(value) { fail!("display value failed") }}
        };
        @string.decode_by_parse @string.encode_by_display @broken type Item = struct { value: Int };
        def attempt = rt.with_diagnostics(fn(n: Int) { let item: Item = {value: n}; codec.encode(codec.Value.type, item) });
        export def answer = (attempt(1), attempt(2));
    "#, "");
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new().execute_linked(linked, Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    assert_eq!(result.world.work.heap.solved_failures.len(), 2);
    for index in [0, 1] {
        let (tag, reports) = result.value().sequence_get(index).unwrap().tagged_parts().unwrap();
        assert_eq!(tag.as_atom().unwrap().as_str(), "Err");
        assert_eq!(reports.sequence_len(), Some(1));
    }
}

#[test]
fn solved_codec_failed_property_is_not_retried_or_reported_twice() {
    for source in [r#"
        import "std/codec" as codec;
        import "std/_rt" as rt;
        def broken: Fn(Type, Option(codec.JsonUntagged)) -> codec.JsonUntagged = fn(owner, previous) { fail!("codec property failed") };
        @broken type Item = enum { One(Int) };
        def attempt = rt.with_diagnostics(fn(n: Int) { codec.encode(codec.Value.type, Item.One(n)) });
        export def answer = (attempt(1), attempt(2));
    "#, r#"
        import "std/codec" as codec;
        import "std/_rt" as rt;
        def broken: Fn(Type, Option(codec.JsonRenameAll)) -> codec.JsonRenameAll = fn(owner, previous) { fail!("decode property failed") };
        @broken type Item = struct { some_value: Int };
        def attempt = rt.with_diagnostics(fn(n: Int) { codec.decode(Item.type, codec.Value.Object({someValue: codec.Value.Int(n)})) });
        export def answer = (attempt(1), attempt(2));
    "#, r#"
        import "std/codec" as codec;
        import "std/_rt" as rt;
        def broken: Fn(Type, Option(codec.JsonRenameAll)) -> codec.JsonRenameAll = fn(owner, previous) { fail!("nested decode property failed") };
        @broken type Item = struct { some_value: Int };
        import "std/json" as json;
        @json.untagged type Choice = enum { Plain(Dict(Int)), Broken(Item) };
        def attempt = rt.with_diagnostics(fn(n: Int) { codec.decode(Choice.type, codec.Value.Object({someValue: codec.Value.Int(n)})) });
        export def answer = (attempt(1), attempt(2));
    "#, r#"
        import "std/codec" as codec; import "std/_rt" as rt;
        import "std/string" as string; import "std/fmt" as fmt;
        def broken: Fn(Type, Option(fmt.DisplayBy)) -> fmt.DisplayBy = fn(owner, previous) { fail!("display provider failed") };
        @string.decode_by_parse @string.encode_by_display @broken type Item = struct { value: Int };
        def attempt = rt.with_diagnostics(fn(n: Int) { let item: Item = {value: n}; codec.encode(codec.Value.type, item) });
        export def answer = (attempt(1), attempt(2));
    "#, r#"
        import "std/codec" as codec; import "std/_rt" as rt; import "std/regex" as regex;
        import "std/string" as string; import "std/json" as json;
        def broken: Fn(Type, Option(regex.ParseBy)) -> regex.ParseBy = fn(owner, previous) { fail!("parse provider failed") };
        @string.decode_by_parse @string.encode_by_display @broken type Item = struct { value: Int };
        @json.untagged type Choice = enum { Plain(String), Parsed(Item) };
        def attempt = rt.with_diagnostics(fn(text: String) { codec.decode(Choice.type, codec.Value.String(text)) });
        export def answer = (attempt("1"), attempt("2"));
    "#] {
    let mir = crate::codegen::tests::graph(source, "");
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new().execute_linked(linked, Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    assert_eq!(result.world.work.heap.solved_failures.len(), 1);
    for index in [0, 1] {
        let (tag, reports) = result.value().sequence_get(index).unwrap().tagged_parts().unwrap();
        assert_eq!(tag.as_atom().unwrap().as_str(), "Err");
        assert_eq!(reports.sequence_len(), Some(if index == 0 { 1 } else { 0 }));
    }
    }
}
#[test]
fn solved_codec_decode_and_encode_share_dictionary_shapes_and_scalar_payloads() {
    let mir = crate::codegen::tests::graph(r#"
        import "std/codec" as codec;
        def input = "a string deliberately longer than inline storage capacity";
        def source = codec.Value.Object({long_dictionary_key: codec.Value.String(input)});
        def decoded = codec.decode(Dict(String).type, source).unwrap!();
        export def answer = (input, source, decoded, codec.encode(codec.Value.type, decoded));
    "#, "");
    let artifact = crate::codegen::compile(mir.seal().unwrap(), crate::codegen::tests::entry(&mir)).unwrap();
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = Vm::new().execute_linked(linked, Quota::with_fuel(10000), crate::DataLimits::default(), &mut SourceDatabase::default()).unwrap();
    let view = HeapView { current: &result.world.work.heap, background: Some(&result.world.main) };
    let root = ValueRef { value: result.world.work.root, view };
    let source = root.sequence_get(1).unwrap().tagged_parts().unwrap().1;
    let decoded = root.sequence_get(2).unwrap();
    let encoded = root.sequence_get(3).unwrap().tagged_parts().unwrap().1;
    let shape = |value: ValueRef<'_>| {
        let DecodedValue::Dict(handle) = value.value.value() else { panic!("Dict"); };
        let Object::Dict { shape, .. } = view.object(handle).unwrap() else { panic!("Dict object"); };
        *shape
    };
    assert_eq!(shape(source), shape(decoded));
    assert_eq!(shape(source), shape(encoded));
    assert_eq!(decoded.dict_get("long_dictionary_key").unwrap().value.value(), root.sequence_get(0).unwrap().value.value());
}
