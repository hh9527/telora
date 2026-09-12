use super::*;
use telora_core::{
    mir::ModuleKind,
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};

fn graph(source: &str) -> (Mir, HirId) {
    graph_with(source, &[])
}
fn graph_with(source: &str, dependencies: &[(&str, &str)]) -> (Mir, HirId) {
    let mut inputs = vec![
        ("@src/main", source),
        (
            "std/prelude",
            include_str!("../../../telora-core/modules/std/prelude.telora"),
        ),
    ];
    for &(name, source) in dependencies {
        if !inputs.iter().any(|(existing, _)| *existing == name) {
            inputs.push((name, source));
        }
    }
    let inventory = inputs
        .iter()
        .map(|(name, _)| ModuleSpec {
            name: (*name).into(),
            kind: ModuleKind::Source,
            native: static_sources::native_module(name),
            implicit_imports: if *name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        Ok(inputs.iter().find(|(n, _)| *n == name).unwrap().1.into())
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    let declaration = mir
        .symbols
        .iter()
        .find(|s| s.name == "answer")
        .unwrap()
        .declarations[0];
    let root = child(&mir, declaration, Role::Value).unwrap();
    (mir, root)
}

#[test]
fn native_reflection_reads_sealed_types_without_materializing_them() {
    for expression in [
        "match td.kind(Never.type) { td.TypeDescKind.Never => True, _ => False }",
        "match td.kind(Int.type) { td.TypeDescKind.Int => True, _ => False }",
        "td.fields(Rec.type)[0].index == 0",
        "match td.fields(Rec.type)[0].name { \"a\" => True, _ => False }",
        "td.variants(Option(Int).type)[1].index == 1",
        "match td.variants(Option(Int).type)[1].payload { Some(t) => match td.kind(t) { td.TypeDescKind.Int => True, _ => False }, _ => False }",
        "match td.kind(td.children(Array(Int).type)[0]) { td.TypeDescKind.Int => True, _ => False }",
        "match td.opaque_name(Int.type) { None => True, _ => False }",
        "match td.resolve(Rec.type) { Ok(t) => match td.kind(t) { td.TypeDescKind.Struct => True, _ => False }, _ => False }",
    ] {
        let source = format!("import \"std/type-desc\" as td; type Rec = struct {{ a: Int }}; export def answer = {expression};");
        let (mir, root) = graph_with(&source, &[("std/type-desc", include_str!("../../../telora-core/modules/std/type-desc.telora"))]);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap_or_else(|error| panic!("{expression}: {error}"));
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        let value = compiled.call(&mut context, &[]).unwrap();
        assert_eq!(value.words()[2], 1, "{expression}");
    }
}

#[test]
fn machine_code_returns_materialized_scalar_and_unit() {
    for (source, data) in [
        ("export def answer = 42;", Some(42)),
        ("export def answer = 1.25;", Some(1.25f64.to_bits())),
        ("export def answer = ();", None),
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let value = compiled.call(&mut CallContext::default(), &[]).unwrap();
        assert_eq!(value.type_key(), compiled.output());
        assert_eq!(
            value.origin(),
            Origin::from_loc(Some(mir.hir[root.index()].location))
        );
        assert_eq!(value.words().get(2).copied(), data);
    }
}

#[test]
fn machine_code_branches_on_argument_and_preserves_selected_value_origin() {
    let (mir, root) = graph(
        "export def answer: Fn(Bool, Int, Int) -> Int = fn(condition, yes, no) { if condition { yes } else { no } };",
    );
    let compiled = compile(&mir.seal().unwrap(), root).unwrap();
    let ids = compiled.arguments();
    let origin = Origin::from_loc(Some(mir.hir[root.index()].location));
    let yes = compiled.layouts().value(ids[1], origin, &[41]).unwrap();
    let no = compiled
        .layouts()
        .value(ids[2], Origin::default(), &[42])
        .unwrap();
    for (condition, expected) in [(1, &yes), (0, &no)] {
        let condition = compiled
            .layouts()
            .value(ids[0], Origin::default(), &[condition])
            .unwrap();
        assert_eq!(
            &compiled
                .call(
                    &mut CallContext::default(),
                    &[condition, yes.clone(), no.clone()]
                )
                .unwrap(),
            expected
        );
    }
    assert!(compiled.call(&mut CallContext::default(), &[]).is_err());
    assert!(
        compiled
            .call(&mut CallContext::default(), &[yes.clone(), yes.clone(), no])
            .is_err()
    );
}

#[test]
fn unused_bindings_still_execute_and_propagate_failure_once() {
    let (mir, root) = graph("export def answer: Fn() -> Int = fn() { let unused = [1][2]; 42 };");
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert!(context.diagnostics()[0].message.contains("out of bounds"));
}

#[test]
fn failed_call_never_decodes_the_result_and_invalid_success_is_rejected() {
    // The wrapper is also the boundary used for generated calls to failing
    // helpers. A Failed status must not inspect the zeroed/unwritten result.
    unsafe extern "C" fn failed(
        _: *mut CallContext,
        _: *const u64,
        _: *mut u64,
        _: *const u64,
    ) -> u32 {
        1
    }
    unsafe extern "C" fn unwritten(
        _: *mut CallContext,
        _: *const u64,
        _: *mut u64,
        _: *const u64,
    ) -> u32 {
        0
    }
    let (mir, root) = graph("export def answer = 42;");
    let mut compiled = compile(&mir.seal().unwrap(), root).unwrap();
    compiled.entry = failed;
    assert_eq!(
        compiled.call(&mut CallContext::default(), &[]).unwrap_err(),
        "native execution failed"
    );
    compiled.entry = unwritten;
    // Zeroed data has no valid stamp for this graph's concrete Int type.
    assert!(compiled.call(&mut CallContext::default(), &[]).is_err());
}
#[test]
fn machine_code_constructs_native_objects_without_old_vm() {
    for (source, kind) in [
        (
            "export def answer = \"a long string from generated machine code\";",
            "string",
        ),
        ("export def answer = [7, 8];", "array"),
        (
            "export def answer = {label: \"long shared record text\", count: 7};",
            "record",
        ),
        ("export def answer: Dict(Int) = {z: 8, a: 7};", "dict"),
    ] {
        let (mir, root) = graph(source);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut ctx = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        let value = compiled
            .call(&mut ctx, &[])
            .unwrap_or_else(|e| panic!("{e}: {:?}", ctx.diagnostics()));
        let rt = ctx.runtime().unwrap();
        match kind {
            "string" => assert_eq!(
                rt.text(value.as_ref()).unwrap().as_str(),
                "a long string from generated machine code"
            ),
            "array" => assert_eq!(rt.scalar_bits(rt.array_get(&value, 0).unwrap()).unwrap(), 7),
            "record" => assert_eq!(rt.scalar_bits(rt.field(&value, 0).unwrap()).unwrap(), 7),
            "dict" => {
                let (key, found) = rt.dict_entry(&value, 0).unwrap();
                assert_eq!(rt.text(key).unwrap().as_str(), "a");
                assert_eq!(rt.scalar_bits(found).unwrap(), 7);
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn generated_index_reads_published_main_and_reports_work_bounds_errors() {
    let (mir, root) = graph(
        "export def answer: Fn(Array(Int), Int) -> Int = fn(values, index) { values[index] };",
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let array_ty = compiled.arguments()[0];
    let int = compiled.arguments()[1];
    let origin = Origin::from_loc(Some(mir.hir[root.index()].location));
    let mut rt = crate::runtime::Runtime::new(&sealed).unwrap();
    let item = rt.scalar(int, origin.words(), 42).unwrap();
    let array = rt.array(array_ty, origin.words(), &[item]).unwrap();
    let roots = rt.publish(&[array]).unwrap();
    let mut ctx = CallContext::with_runtime(rt);
    let zero = compiled
        .layouts()
        .value(int, Origin::default(), &[0])
        .unwrap();
    let value = compiled.call(&mut ctx, &[roots[0].clone(), zero]).unwrap();
    assert_eq!(value.words()[2], 42);
    assert_eq!(value.origin(), origin);
    let bad_index = compiled
        .layouts()
        .value(int, Origin::default(), &[1])
        .unwrap();
    assert!(
        compiled
            .call(&mut ctx, &[roots[0].clone(), bad_index])
            .is_err()
    );
    assert_eq!(ctx.diagnostics().len(), 1);
    assert!(ctx.diagnostics()[0].message.contains("out of bounds"));
    assert_ne!(ctx.diagnostics()[0].origin, Origin::default());
}

#[test]
fn direct_functions_have_independent_frames_and_support_recursion() {
    for source in [
        "def choose: Fn(Bool, Int, Int) -> Int = fn(flag, x, y) { if flag {x} else {y} }; export def answer = do { let left = 41; let right = 42; choose(False, left, right) };",
        "def recur: Fn(Bool) -> Int = fn(flag) { if flag { recur(False) } else { 42 } }; export def answer = recur(True);",
        "export def answer = do { def local: Fn(Int) -> Int = fn(value) { let saved = value; saved }; local(42) };",
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let result = compiled.call(&mut CallContext::default(), &[]).unwrap();
        assert_eq!(result.words()[2], 42);
    }
}
#[test]
fn scalar_machine_code_handles_recursion_and_checked_arithmetic() {
    let (mir, root) = graph(include_str!("../../tests/fixtures/factorial.telora"));
    let compiled = compile(&mir.seal().unwrap(), root).unwrap();
    assert_eq!(
        compiled
            .call(&mut CallContext::default(), &[])
            .unwrap()
            .words()[2],
        120
    );
    for (source, message) in [
        ("export def answer = 9223372036854775807 + 1;", "overflowed"),
        (
            "def divide: Fn(Int, Int) -> Int = fn(a, b) { a / b }; export def answer = divide(1, 0);",
            "division by zero",
        ),
        ("export def answer = 1.0 / 0.0;", "non-finite"),
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let mut ctx = CallContext::default();
        assert!(compiled.call(&mut ctx, &[]).is_err());
        assert_eq!(ctx.diagnostics().len(), 1);
        assert!(
            ctx.diagnostics()[0].message.contains(message),
            "{:?}",
            ctx.diagnostics()
        );
        assert_ne!(ctx.diagnostics()[0].origin, Origin::default());
    }
    for source in [
        "export def answer = False && [True][1];",
        "export def answer = True || [False][1];",
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let mut ctx = CallContext::default();
        compiled.call(&mut ctx, &[]).unwrap(); // RHS requires a runtime and would fail if executed.
        assert!(ctx.diagnostics().is_empty());
    }
}
#[test]
fn generic_calls_consume_closed_instances_without_substituting_types_at_runtime() {
    let (mir, root) = graph(include_str!("../../tests/fixtures/generics.telora"));
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let result = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime().unwrap();
    assert_eq!(rt.scalar_bits(rt.field(&result, 0).unwrap()).unwrap(), 42);
    assert_eq!(
        rt.text(rt.field(&result, 1).unwrap()).unwrap().as_str(),
        "native generic text"
    );
    let array = rt.field(&result, 2).unwrap().to_owned();
    assert_eq!(rt.scalar_bits(rt.array_get(&array, 0).unwrap()).unwrap(), 7);
}
#[test]
fn generated_enums_preserve_inline_and_boxed_payloads_through_publication() {
    let (mir, root) = graph(include_str!("../../tests/fixtures/enums.telora"));
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let value = compiled
        .call(&mut context, &[])
        .unwrap_or_else(|e| panic!("{e}: {:?}", context.diagnostics()));
    let roots = context.runtime_mut().unwrap().publish(&[value]).unwrap();
    let rt = context.runtime().unwrap();
    let tree = rt.field(&roots[0], 0).unwrap().to_owned();
    assert_eq!(
        rt.variant_name(tree.type_key(), rt.enum_tag(&tree).unwrap())
            .unwrap(),
        "Children"
    );
    let array = rt.enum_payload(&tree).unwrap().unwrap().to_owned();
    let leaf = rt.array_get(&array, 0).unwrap().to_owned();
    assert_eq!(
        rt.scalar_bits(rt.enum_payload(&leaf).unwrap().unwrap())
            .unwrap(),
        42
    );
    let empty = rt.array_get(&array, 1).unwrap().to_owned();
    assert!(rt.enum_payload(&empty).unwrap().is_none());
    let link = rt.field(&roots[0], 1).unwrap().to_owned();
    let end = rt.enum_payload(&link).unwrap().unwrap().to_owned();
    assert_eq!(
        rt.variant_name(end.type_key(), rt.enum_tag(&end).unwrap())
            .unwrap(),
        "End"
    );
    assert!(rt.enum_payload(&end).unwrap().is_none());
}
#[test]
fn property_queries_execute_provider_chains_and_cache_results() {
    let source = "import \"std/type-property\" { get_type_prop }; @property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { { value: 21 + match previous { Some(p) => p.value, None => 0 } } }; @mark @mark type Item = struct { x: Int }; export def answer = match get_type_prop(Item.type, Mark.type) { Some(p) => p.value, None => 0 };";
    let (mir, root) = graph_with(source, static_sources::BUILTINS);
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);
    context.runtime_mut().unwrap().publish(&[]).unwrap();
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);
    assert!(context.diagnostics().is_empty());
    let configured = "import \"std/type-property\" { get_type_prop }; @property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Int) -> Fn(Type, Option(Mark)) -> Mark = fn(base) { fn(owner, previous) { { value: base + match previous { Some(p) => p.value, None => 0 } } } }; @mark(20) @mark(22) type Item = struct { x: Int }; export def answer = match get_type_prop(Item.type, Mark.type) { Some(p) => p.value, None => 0 };";
    let (mir, root) = graph_with(configured, static_sources::BUILTINS);
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);
}

#[test]
fn data_modules_are_injected_before_initialization_without_old_values() {
    let mut mir = crate::test_support::graph_with_data(
        "import \"@src/data\" as input; export def answer = input.data;",
        static_sources::BUILTINS,
        &["@src/data"],
    );
    let root = crate::test_support::value_node(&mir, "answer");
    let source = mir.sources.add("data.json", "{\"answer\":42}");
    let plan = telora_core::data_plan::parse_registered(
        &mir.sources,
        source,
        telora_core::data_plan::Format::Json,
    )
    .unwrap();
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let symbol = compiled.data_modules()[0].symbol;
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.inject_data(&mut context, symbol, &plan).unwrap();
    assert!(compiled.inject_data(&mut context, symbol, &plan).is_err());
    compiled.initialize(&mut context).unwrap();
    let value = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime().unwrap();
    let dict = rt.enum_payload(&value).unwrap().unwrap().to_owned();
    let value = rt.dict_entry(&dict, 0).unwrap().1.to_owned();
    assert_eq!(rt.enum_payload(&value).unwrap().unwrap().words()[2], 42);
    let mut missing = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.initialize(&mut missing).is_err());
    assert_eq!(missing.diagnostics().len(), 1);
    assert!(!missing.runtime().unwrap().is_published());
}

#[test]
fn member_property_contexts_use_sealed_names_indices_and_payload_types() {
    for (source, expected, name) in [
        (
            "import \"std/type-property\" { FieldPropertyCtx, get_field_prop }; @property(PropertyTarget.Field) type Mark = struct { value: Int, label: String }; def mark: Fn(FieldPropertyCtx, Option(Mark)) -> Mark = fn(ctx, previous) { { value: if ctx.ty == Int.type { 42 + ctx.index } else { 0 }, label: ctx.name } }; type Owner = struct { @mark a: Int }; export def answer = match get_field_prop(Owner.type, 0, Mark.type) { Some(p) => (p.value, p.label), None => (0, \"missing\") };",
            42,
            "a",
        ),
        (
            "import \"std/type-property\" { VariantPropertyCtx, get_variant_prop }; @property(PropertyTarget.Variant) type Mark = struct { value: Int, label: String }; def mark: Fn(VariantPropertyCtx, Option(Mark)) -> Mark = fn(ctx, previous) { { value: match ctx.payload { Some(t) => if t == Int.type { 41 + ctx.index } else { 0 }, None => 24 }, label: ctx.name } }; type Owner = enum { @mark Empty, @mark Value(Int) }; export def answer = match get_variant_prop(Owner.type, 1, Mark.type) { Some(p) => (p.value, p.label), None => (0, \"missing\") };",
            42,
            "Value",
        ),
    ] {
        let (mir, root) = graph_with(source, static_sources::BUILTINS);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        let result = compiled.call(&mut context, &[]).unwrap();
        let rt = context.runtime().unwrap();
        assert_eq!(rt.field(&result, 0).unwrap().words()[2], expected);
        assert_eq!(
            rt.text(rt.field(&result, 1).unwrap()).unwrap().as_str(),
            name
        );
        compiled.initialize(&mut context).unwrap();
        assert!(context.runtime().unwrap().is_published());
        if name == "Value" {
            let source = source.replace("Owner.type, 1, Mark.type", "Owner.type, 0, Mark.type");
            let (mir, root) = graph_with(&source, static_sources::BUILTINS);
            let sealed = mir.seal().unwrap();
            let compiled = compile(&sealed, root).unwrap();
            let mut context =
                CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
            let result = compiled.call(&mut context, &[]).unwrap();
            let rt = context.runtime().unwrap();
            assert_eq!(rt.field(&result, 0).unwrap().words()[2], 24);
            assert_eq!(
                rt.text(rt.field(&result, 1).unwrap()).unwrap().as_str(),
                "Empty"
            );
        }
    }
}

#[test]
fn generic_property_chains_consume_separate_closed_witnesses() {
    let source = "import \"std/type-property\" { get_type_prop }; @property(PropertyTarget.Type) type Mark(T) = struct { witness: TypeOf(T), count: Int }; def mark: for(T) Fn(TypeOf(T)) -> Fn(Type, Option(Mark(T))) -> Mark(T) = fn(witness) { fn(owner, previous) { { witness: witness, count: 1 + match previous { Some(p) => p.count, None => 0 } } } }; @mark(T.type) @mark(T.type) type Box(T) = struct { value: T }; export def answer = do { let a = match get_type_prop(Box(Int).type, Mark(Int).type) { Some(p) => p, None => fail!(\"missing Int\") }; let b = match get_type_prop(Box(String).type, Mark(String).type) { Some(p) => p, None => fail!(\"missing String\") }; if a.witness == Int.type && b.witness == String.type { a.count + b.count } else { 0 } };";
    let (mir, root) = graph_with(source, static_sources::BUILTINS);
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let result = compiled
        .call(&mut context, &[])
        .unwrap_or_else(|e| panic!("{e}: {:?}", context.diagnostics()));
    assert_eq!(result.words()[2], 4);
    compiled.initialize(&mut context).unwrap();
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 4);
}

#[test]
fn property_evidence_and_capability_rejection_follow_the_sealed_plan() {
    let source = "import \"std/type-property\" { evidence }; @property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { { value: 42 } }; @mark type Item = struct { x: Int }; export def answer = evidence(Item.type, Mark.type).value;";
    let (mir, root) = graph_with(source, static_sources::BUILTINS);
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);

    let rejected = source.replace("PropertyTarget.Type", "PropertyTarget.Field");
    let (mir, root) = graph_with(&rejected, static_sources::BUILTINS);
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert!(
        context.diagnostics()[0]
            .message
            .contains("does not support this decorator target")
    );

    let (mir, root) = graph_with(
        "import \"std/type-property\" { get_type_prop }; @property(PropertyTarget.Type) type Mark = struct {}; export def answer = match get_type_prop(Int.type, Mark.type) { None => 42, Some(_) => 0 };",
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);
    assert!(context.diagnostics().is_empty());
}

#[test]
fn property_and_global_cycles_fail_once_and_unused_properties_initialize() {
    let source = "import \"std/type-property\" { get_type_prop }; @property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { { value: number } }; @mark type Item = struct { x: Int }; def number: Int = match get_type_prop(Item.type, Mark.type) { Some(p) => p.value, None => 0 }; export def answer = number;";
    let (mir, root) = graph_with(source, static_sources::BUILTINS);
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    for _ in 0..2 {
        assert!(compiled.call(&mut context, &[]).is_err());
    }
    assert_eq!(context.diagnostics().len(), 1);
    assert!(
        context.diagnostics()[0]
            .message
            .contains("dependency cycle")
    );
    assert!(context.runtime_mut().unwrap().publish(&[]).is_err());

    let source = "@property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { fail!(\"unused property failed\") }; @mark type Item = struct { x: Int }; export def answer = 42;";
    let (mir, root) = graph(source);
    let module = mir.hir[root.index()].module;
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.initialize(&mut context).is_err());
    assert!(!context.runtime().unwrap().is_published());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "unused property failed");
}

#[test]
fn property_marker_factory_returns_a_reducing_native_provider() {
    for (source, expected) in [
        (
            "def mark = property(PropertyTarget.Type); export def answer = mark(Int.type, None).bits;",
            1,
        ),
        (
            "def mark = property(PropertyTarget.Type); def previous = property(PropertyTarget.Field)(Int.type, None); export def answer = mark(Int.type, Some(previous)).bits;",
            17,
        ),
    ] {
        let (mir, root) = graph(source);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        assert_eq!(
            compiled.call(&mut context, &[]).unwrap().words()[2],
            expected
        );
        context.runtime_mut().unwrap().publish(&[]).unwrap();
        assert_eq!(
            compiled.call(&mut context, &[]).unwrap().words()[2],
            expected
        );
    }
}

#[test]
fn native_map_calls_captured_and_nested_language_callbacks() {
    let dependencies = [(
        "std/array",
        include_str!("../../../telora-core/modules/std/array.telora"),
    )];
    for source in [
        "import \"std/array\" { map }; export def answer = do { let base = 40; map([1, 2], fn(x) { base + x }) };",
        "import \"std/array\" as array; export def answer = array.map([1, 2], fn(x) { x + 40 });",
        "import \"std/array\" { map }; export def answer = map([1, 2], fn(x) { map([40], fn(y) { x + y })[0] });",
    ] {
        let (mir, root) = graph_with(source, &dependencies);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        let result = compiled.call(&mut context, &[]).unwrap();
        let rt = context.runtime_mut().unwrap();
        assert_eq!(rt.array_get(&result, 0).unwrap().words()[2], 41);
        assert_eq!(rt.array_get(&result, 1).unwrap().words()[2], 42);
        let roots = rt.publish(&[result]).unwrap();
        assert_eq!(rt.array_get(&roots[0], 1).unwrap().words()[2], 42);
    }
    let (mir, root) = graph_with(
        "import \"std/array\" { map }; def callback: Fn(Int) -> Int = fn(x) { if x == 2 { fail!(\"callback failed\") } else { x } }; export def answer = map([1, 2, 3], callback);",
        &dependencies,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "callback failed");
    for (source, expected_width) in [
        (
            "import \"std/array\" { map }; export def answer = map([1, 2], fn(x) { () });",
            2,
        ),
        (
            "import \"std/array\" { map }; export def answer = map([1, 2], fn(x) { \"long callback text that lives in heap\" });",
            4,
        ),
    ] {
        let (mir, root) = graph_with(source, &dependencies);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        let result = compiled.call(&mut context, &[]).unwrap();
        let rt = context.runtime_mut().unwrap();
        let roots = rt.publish(&[result]).unwrap();
        let value = rt.array_get(&roots[0], 1).unwrap();
        assert_eq!(value.words().len(), expected_width);
        if expected_width == 4 {
            assert_eq!(
                rt.text(value).unwrap().as_str(),
                "long callback text that lives in heap"
            );
        }
    }
}

#[test]
fn native_folds_use_sealed_accumulators_and_sorted_dictionary_order() {
    let (mir, root) = graph_with(include_str!("../../tests/fixtures/fold.telora"), static_sources::BUILTINS);
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let result = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime().unwrap();
    assert_eq!(rt.scalar_bits(rt.field(&result, 0).unwrap()).unwrap(), 234);
    assert_eq!(rt.scalar_bits(rt.field(&result, 1).unwrap()).unwrap(), 923);
    assert_eq!(rt.field(&result, 2).unwrap().words(), rt.field(&result, 3).unwrap().words());
    let initial = rt.field(&result, 2).unwrap().to_owned();
    let folded = rt.field(&result, 4).unwrap().to_owned();
    assert_eq!(rt.scalar_bits(rt.field(&folded, 0).unwrap()).unwrap(), 45);
    assert_eq!(rt.field(&initial, 1).unwrap().words(), rt.field(&folded, 1).unwrap().words());
    assert_eq!(context.call_depth(), 0);
    for source in [
        "import \"std/array\" { fold }; export def answer = fold([1], 0, fn(state, value) -> Int { fail!(\"fold failed\") });",
        "import \"std/dict\" { fold }; export def answer = fold({ a: 1 }, 0, fn(state, key, value) -> Int { fail!(\"fold failed\") });",
    ] {
        let (mir, root) = graph_with(source, static_sources::BUILTINS);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        assert!(compiled.call(&mut context, &[]).is_err());
        assert_eq!(context.diagnostics().len(), 1);
        assert_eq!(context.diagnostics()[0].message, "fold failed");
        assert_eq!(context.call_depth(), 0);
    }
}

#[test]
fn native_dictionary_pair_roundtrip_and_merge_keep_right_hand_values() {
    let (mir, root) = graph_with(
        "import \"std/dict\" as dict; export def answer = do { let left = dict.from_pairs([(\"z\", 1), (\"a\", 2)]); let right = dict.from_pairs([(\"z\", 3), (\"b\", 4)]); let merged = dict.merge(left, right); (right, merged, dict.from_pairs(dict.pairs(merged))) };",
        static_sources::BUILTINS,
    );
    let module = mir.hir[root.index()].module;
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    let result = compiled.export(&mut context, mir.exports[module.index()][0]).unwrap();
    let rt = context.runtime().unwrap();
    let right = rt.field(&result, 0).unwrap().to_owned();
    let merged = rt.field(&result, 1).unwrap().to_owned();
    let roundtrip = rt.field(&result, 2).unwrap().to_owned();
    assert_eq!(rt.dict_len(&merged).unwrap(), 3);
    for (index, (name, bits)) in [("a", 2), ("b", 4), ("z", 3)].into_iter().enumerate() {
        let (key, value) = rt.dict_entry(&merged, index).unwrap();
        assert_eq!(rt.text(key).unwrap().as_str(), name);
        assert_eq!(rt.scalar_bits(value).unwrap(), bits);
        assert_eq!(rt.dict_entry(&roundtrip, index).unwrap().1.words(), value.words());
    }
    let (key, value) = rt.dict_entry(&merged, 2).unwrap();
    let (original_key, original_value) = rt.dict_entry(&right, 1).unwrap();
    assert_eq!(key.words(), original_key.words());
    assert_eq!(value.words(), original_value.words());

    let (mir, root) = graph_with("import \"std/dict\" { from_pairs }; export def answer = from_pairs([(\"a\", 1), (\"a\", 2)]);", static_sources::BUILTINS);
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "std/dict.from_pairs contains duplicate field \"a\"");
    assert_eq!(context.call_depth(), 0);
}

#[test]
fn native_array_predicates_short_circuit_and_keep_selected_origins() {
    let (mir, root) = graph_with(include_str!("../../tests/fixtures/array-predicates.telora"), static_sources::BUILTINS);
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let result = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime().unwrap();
    let original = rt.field(&result, 0).unwrap().to_owned();
    let selected = rt.field(&result, 1).unwrap().to_owned();
    assert_eq!(rt.array_len(&selected).unwrap(), 2);
    for i in 0..2 { assert_eq!(rt.array_get(&selected, i).unwrap().words(), rt.array_get(&original, i + 2).unwrap().words()); }
    for (i, expected) in [1, 0, 0, 1].into_iter().enumerate() { assert_eq!(rt.scalar_bits(rt.field(&result, i + 2).unwrap()).unwrap(), expected); }
    assert_eq!(context.call_depth(), 0);
    assert!(context.diagnostics().is_empty());
}

#[test]
fn native_array_predicate_failure_propagates_once() {
    for operation in ["filter", "any", "all"] {
        let source = format!("import \"std/array\" as array; export def answer = array.{operation}([1, 2], fn(value) -> Bool {{ fail!(\"predicate failed\") }});");
        let (mir, root) = graph_with(&source, static_sources::BUILTINS);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        assert!(compiled.call(&mut context, &[]).is_err());
        assert_eq!(context.call_depth(), 0);
        assert_eq!(context.diagnostics().len(), 1);
        assert_eq!(context.diagnostics()[0].message, "predicate failed");
    }
}

#[test]
fn native_dictionary_reads_preserve_sorted_columns_and_aliases_after_publication() {
    let (mir, root) = graph_with(
        "import \"std/dict\" as dict; export def answer = do { let d: Dict(String) = { z: \"last long heap-backed value\", a: \"first long heap-backed value\" }; (d, dict.keys(d), dict.values(d), dict.get(d, \"a\"), dict.get(d, \"missing\")) };",
        static_sources::BUILTINS,
    );
    let module = mir.hir[root.index()].module;
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    let result = compiled.export(&mut context, mir.exports[module.index()][0]).unwrap();
    let rt = context.runtime().unwrap();
    let dict = rt.field(&result, 0).unwrap().to_owned();
    let keys = rt.field(&result, 1).unwrap().to_owned();
    let values = rt.field(&result, 2).unwrap().to_owned();
    let found = rt.field(&result, 3).unwrap().to_owned();
    let missing = rt.field(&result, 4).unwrap().to_owned();
    assert_eq!(keys.words()[2] as u32, dict.words()[2] as u32);
    assert_eq!(values.words()[2] as u32, dict.words()[3] as u32);
    assert_eq!(rt.text(rt.array_get(&keys, 0).unwrap()).unwrap().as_str(), "a");
    assert_eq!(rt.text(rt.array_get(&keys, 1).unwrap()).unwrap().as_str(), "z");
    assert_eq!(rt.enum_payload(&found).unwrap().unwrap().words(), rt.array_get(&values, 0).unwrap().words());
    assert!(rt.enum_payload(&missing).unwrap().is_none());
}

#[test]
fn native_call_depth_unwinds_direct_and_indirect_failures() {
    for source in [
        "def recur: Fn(Int) -> Int = fn(n) { recur(n + 1) }; export def answer = recur(0);",
        "def apply: Fn(Fn(Int) -> Int, Int) -> Int = fn(f, n) { f(n) }; def recur: Fn(Int) -> Int = fn(n) { apply(recur, n + 1) }; export def answer = recur(0);",
    ] {
        let (mir, root) = graph(source);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap()).with_call_depth_limit(8);
        assert!(compiled.call(&mut context, &[]).is_err());
        assert_eq!(context.call_depth(), 0);
        assert_eq!(context.diagnostics().len(), 1);
        assert_eq!(context.diagnostics()[0].message, "native call depth limit exceeded");
        assert_ne!(context.diagnostics()[0].origin, Origin::default());
    }
    for source in ["export def answer = 42;", "export def answer: Int = fail!(\"failure\");"] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let mut context = CallContext::default().with_call_depth_limit(1);
        let _ = compiled.call(&mut context, &[]);
        assert_eq!(context.call_depth(), 0);
        let mut denied = CallContext::default().with_call_depth_limit(0);
        assert!(compiled.call(&mut denied, &[]).is_err());
        assert_eq!(denied.call_depth(), 0);
        assert_eq!(denied.diagnostics().len(), 1);
    }
}

#[test]
fn native_fuel_is_shared_across_calls_and_stops_recursion_once() {
    let (mir, root) = graph("export def answer = 42;");
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::default().with_fuel(2);
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);
    assert_eq!(context.remaining_fuel(), Some(1));
    compiled.call(&mut context, &[]).unwrap();
    assert_eq!(context.remaining_fuel(), Some(0));
    assert!(compiled.call(&mut context, &[]).is_err());
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].origin, Origin::from_loc(Some(mir.hir[root.index()].location)));

    let (mir, root) = graph("def recur: Fn(Int) -> Int = fn(n) { if n == 0 { 0 } else { recur(n - 1) } }; export def answer = recur(10000);");
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap()).with_fuel(50);
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "native execution fuel exhausted");
    assert_eq!(context.remaining_fuel(), Some(0));
}

#[test]
fn native_fuel_survives_initialization_publication_and_entry_calls() {
    let (mir, root) = graph("def base = 40 + 2; export def answer: Fn() -> Int = fn() { base };");
    let module = mir.hir[root.index()].module;
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap()).with_fuel(100);
    compiled.initialize(&mut context).unwrap();
    assert!(context.runtime().unwrap().is_published());
    let initialized = context.remaining_fuel().unwrap();
    assert!(initialized < 100);
    let closure = compiled.export(&mut context, mir.exports[module.index()][0]).unwrap();
    assert_eq!(context.remaining_fuel(), Some(initialized));
    let result = compiled.call_closure(&mut context, &closure, &[]).unwrap();
    assert_eq!(result.words()[2], 42);
    assert!(context.remaining_fuel().unwrap() < initialized);
    let mut exhausted = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap()).with_fuel(0);
    assert!(compiled.initialize(&mut exhausted).is_err());
    assert!(!exhausted.runtime().unwrap().is_published());
    assert_eq!(exhausted.diagnostics().len(), 1);
}

#[test]
fn native_interpolation_consumes_sealed_display_calls() {
    for (source, expected) in [
        (r#"export def answer = if "中" == "中" && "long heap-backed comparison" != "different heap-backed string" { "ok" } else { "bad" };"#, "ok"),
        (r#"export def answer = `n=\{42}, text=\{"中"}`;"#, "n=42, text=中"),
        (r#"import "std/fmt" as fmt; type Item = struct {value: Int}; impl fmt.Display for Item { display: fn(value) { fmt.from_string("item") } }; def value: Item = {value: 1}; export def answer = `\{value}`;"#, "item"),
        (r#"import "std/fmt" as fmt; def render: for(T: fmt.Display) Fn(T) -> String = fn(value) { `\{value}` }; export def answer = render(42);"#, "42"),
    ] {
        let (mir, root) = graph_with(source, static_sources::BUILTINS);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        let value = compiled.call(&mut context, &[]).unwrap_or_else(|e| panic!("{source}: {e}: {:?}", context.diagnostics()));
        assert_eq!(context.runtime().unwrap().text(value.as_ref()).unwrap().as_str(), expected);
    }
}

#[test]
fn native_codec_does_not_silently_ignore_property_driven_encoding() {
    let (mir, root) = graph_with(
        "import \"std/codec\" { encode, Value }; import \"std/_codec\" { rename_all, RenameCase }; @rename_all(RenameCase.CamelCase) type Rec = struct { some_field: Int }; export def answer = do { let value: Rec = { some_field: 42 }; encode(Value.type, value) };",
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "native codec property execution is not yet linked");
}

#[test]
fn native_codec_encodes_enum_payloads_and_empty_collections() {
    for (expression, expected) in [
        ("Choice.Payload(42)", "{\"Payload\":42}"),
        ("Choice.Empty", "\"Empty\""),
        ("()", "[]"),
        ("do { let d: Dict(Int) = { z: 2, a: 1 }; d }", "{\"a\":1,\"z\":2}"),
        ("do { let xs: Array(Int) = []; xs }", "[]"),
    ] {
        let source = format!("import \"std/codec\" {{ encode, Value }}; type Choice = enum {{ Empty, Payload(Int) }}; export def answer = encode(Value.type, {expression});");
        let (mir, root) = graph_with(&source, static_sources::BUILTINS);
        let sealed = mir.seal().unwrap();
        let contract = crate::runtime::DataContract::from_mir(&sealed).unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        let value = compiled.call(&mut context, &[]).unwrap_or_else(|e| panic!("{expression}: {e}: {:?}", context.diagnostics()));
        assert_eq!(context.runtime().unwrap().semantic_json(&contract, &value).unwrap(), expected);
    }
}

#[test]
fn native_codec_encodes_solved_aggregates_and_reuses_string_storage() {
    let (mir, root) = graph_with(
        "import \"std/codec\" { encode, Value }; type Rec = struct { a: Int, text: String }; export def answer = do { let text = \"long heap-backed codec input string\"; let rec: Rec = { a: 42, text }; (text, encode(Value.type, (rec, [True, False], Some(3), None))) };",
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let contract = crate::runtime::DataContract::from_mir(&sealed).unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let result = compiled.call(&mut context, &[]).unwrap_or_else(|error| panic!("{error}: {:?}", context.diagnostics()));
    let rt = context.runtime().unwrap();
    let encoded = rt.field(&result, 1).unwrap().to_owned();
    assert_eq!(rt.semantic_json(&contract, &encoded).unwrap(), "[{\"a\":42,\"text\":\"long heap-backed codec input string\"},[true,false],3,null]");
    let outer = rt.enum_payload(&encoded).unwrap().unwrap().to_owned();
    let record = rt.array_get(&outer, 0).unwrap().to_owned();
    let object = rt.enum_payload(&record).unwrap().unwrap().to_owned();
    let (_, text) = rt.dict_entry(&object, 1).unwrap();
    let text = text.to_owned();
    assert_eq!(rt.enum_payload(&text).unwrap().unwrap().words(), rt.field(&result, 0).unwrap().words());
}

#[test]
fn native_array_find_short_circuits_and_preserves_selected_descriptor() {
    let (mir, root) = graph_with(
        "import \"std/array\" { find }; export def answer = do { let xs = [\"long heap-backed selected string\", \"unreachable\"]; (xs, find(xs, fn(x) { match x { \"unreachable\" => fail!(\"visited too far\"), _ => True } })) };",
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let result = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime().unwrap();
    let array = rt.field(&result, 0).unwrap().to_owned();
    let option = rt.field(&result, 1).unwrap().to_owned();
    assert_eq!(rt.enum_payload(&option).unwrap().unwrap().words(), rt.array_get(&array, 0).unwrap().words());
    assert!(context.diagnostics().is_empty());
    for source in [
        "import \"std/array\" { find }; export def answer = find([1, 2], fn(x) { False });",
        "import \"std/array\" { find }; export def answer = do { let xs: Array(Int) = []; find(xs, fn(x) { fail!(\"empty callback\") }) };",
    ] {
        let (mir, root) = graph_with(source, static_sources::BUILTINS);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        let value = compiled.call(&mut context, &[]).unwrap();
        assert!(context.runtime().unwrap().enum_payload(&value).unwrap().is_none());
        assert!(context.diagnostics().is_empty());
    }
}

#[test]
fn native_dict_map_keeps_sorted_shared_keys_and_propagates_callback_failure() {
    let (mir, root) = graph_with(
        "import \"std/dict\" { map_values }; export def answer = do { let d: Dict(Int) = { z: 2, a: 40 }; let offset = 1; (d, map_values(d, fn(x) { x + offset })) };",
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let result = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime().unwrap();
    let original = rt.field(&result, 0).unwrap().to_owned();
    let mapped = rt.field(&result, 1).unwrap().to_owned();
    assert_eq!(original.words()[2], mapped.words()[2]);
    for (index, (key, expected)) in [("a", 41), ("z", 3)].into_iter().enumerate() {
        let (name, value) = rt.dict_entry(&mapped, index).unwrap();
        assert_eq!(rt.text(name).unwrap().as_str(), key);
        assert_eq!(rt.scalar_bits(value).unwrap(), expected);
    }
    let (mir, root) = graph_with(
        "import \"std/dict\" { map_values }; export def answer = do { let d: Dict(Int) = { a: 1 }; map_values(d, fn(x) -> Int { fail!(\"map failed\") }) };",
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "map failed");
}

#[test]
fn native_abi_links_resolved_aliases_and_generic_function_values() {
    let dependencies = [(
        "std/array",
        include_str!("../../../telora-core/modules/std/array.telora"),
    )];
    for source in [
        "import \"std/array\" { length as count }; export def answer = count([40, 2]);",
        "import \"std/array\" { length }; def apply: Fn(Fn(Array(Int)) -> Int, Array(Int)) -> Int = fn(f, a) { f(a) }; export def answer = apply(length@[Int], [40, 2]);",
    ] {
        let (mir, root) = graph_with(source, &dependencies);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 2);
    }
    let (mir, root) =
        graph("native length: Fn(Array(Int)) -> Int; export def answer = length([1]);");
    assert!(
        compile(&mir.seal().unwrap(), root)
            .err()
            .unwrap()
            .contains("admitted ABI module")
    );
    let (mir, root) = graph_with(
        "import \"std/string\" { length as count }; export def answer = count(\"中é🙂\");",
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 3);

    let (mir, root) = graph_with(
        "import \"std/array\" { length }; export def answer = length([1, 2]);",
        &[(
            "std/array",
            "native length: Fn(Array(Int)) -> Int; export { length };",
        )],
    );
    let symbol = *mir
        .exports
        .iter()
        .flatten()
        .find(|s| mir.symbols[s.index()].name == "length")
        .unwrap();
    let module = mir.symbols[symbol.index()].module.unwrap();
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[root]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    let function = compiled.export(&mut context, symbol).unwrap();
    context.runtime().unwrap().function_id(&function).unwrap();
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 2);
}

#[test]
fn metadata_uses_sealed_type_ids_and_survives_publication() {
    let (mir, root) = graph("export def answer = (Int.type, Array(String).type, Unit.type);");
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let value = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime_mut().unwrap();
    let before = (0..3)
        .map(|i| rt.represented_type(rt.field(&value, i).unwrap()).unwrap())
        .collect::<Vec<_>>();
    let roots = rt.publish(&[value]).unwrap();
    for (index, expected) in before.into_iter().enumerate() {
        assert_eq!(
            rt.represented_type(rt.field(&roots[0], index).unwrap())
                .unwrap(),
            expected
        );
    }
    let field = rt.field(&roots[0], 0).unwrap().to_owned();
    let wrong = rt
        .represented_type(rt.field(&roots[0], 1).unwrap())
        .unwrap();
    assert!(rt.metadata(field.type_key(), [1, 0, 1], wrong).is_err());

    for source in [
        "export def answer = Int.type != String.type;",
        "type Alias = Int; export def answer = Int.type == Alias.type;",
        "def pass: Fn(Type) -> Type = fn(x) { x }; export def answer = pass(Int.type) == Int.type;",
        "def get: Fn() -> Type = fn() { Int.type }; export def answer = get() == Int.type;",
        "def get: Fn() -> Type = fn() { return Int.type; }; export def answer = get() == Int.type;",
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        assert_eq!(
            compiled
                .call(&mut CallContext::default(), &[])
                .unwrap()
                .words()[2],
            1
        );
    }
}

#[test]
fn dynamic_values_preserve_sealed_identity_and_shared_payloads() {
    let (mir, root) = graph_with(
        include_str!("../../tests/fixtures/dynamic.telora"),
        static_sources::BUILTINS,
    );
    let module = mir.hir[root.index()].module;
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    let value = compiled
        .export(&mut context, mir.exports[module.index()][0])
        .unwrap();
    let runtime = context.runtime().unwrap();
    let boxed = runtime.field(&value, 0).unwrap().to_owned();
    let payload = runtime.dynamic_value(&boxed).unwrap();
    assert_eq!(
        runtime
            .represented_type(runtime.field(&value, 1).unwrap())
            .unwrap(),
        payload.type_id()
    );
    let some = runtime.field(&value, 2).unwrap().to_owned();
    assert_eq!(
        runtime.enum_payload(&some).unwrap().unwrap().words(),
        payload.words()
    );
    let none = runtime.field(&value, 3).unwrap().to_owned();
    assert!(runtime.enum_payload(&none).unwrap().is_none());
    let checked = runtime.field(&value, 4).unwrap().to_owned();
    assert_eq!(
        runtime.enum_payload(&checked).unwrap().unwrap().words()[2],
        42
    );
    let mismatch = runtime.field(&value, 5).unwrap().to_owned();
    assert!(runtime.enum_payload(&mismatch).unwrap().is_none());
    for (index, expected) in [(6, "Array"), (7, "Tagged"), (8, "Atom")] {
        let kind = runtime.field(&value, index).unwrap().to_owned();
        assert_eq!(
            runtime
                .variant_name(kind.type_id(), runtime.enum_tag(&kind).unwrap())
                .unwrap(),
            expected
        );
    }
    for index in [9, 10] {
        let result = runtime.field(&value, index).unwrap().to_owned();
        assert_eq!(
            runtime
                .variant_name(result.type_id(), runtime.enum_tag(&result).unwrap())
                .unwrap(),
            "Ok"
        );
        let dynamic = runtime.enum_payload(&result).unwrap().unwrap().to_owned();
        assert_eq!(runtime.dynamic_value(&dynamic).unwrap().words()[2], 42);
    }
    for index in [11, 12] {
        let result = runtime.field(&value, index).unwrap().to_owned();
        assert_eq!(
            runtime
                .variant_name(result.type_id(), runtime.enum_tag(&result).unwrap())
                .unwrap(),
            "Err"
        );
    }
    let success = |index| {
        let result = runtime.field(&value, index).unwrap().to_owned();
        assert_eq!(
            runtime
                .variant_name(result.type_id(), runtime.enum_tag(&result).unwrap())
                .unwrap(),
            "Ok"
        );
        runtime.enum_payload(&result).unwrap().unwrap().to_owned()
    };
    let fields = success(13);
    assert_eq!(runtime.array_len(&fields).unwrap(), 2);
    for (index, name) in [(0, "a"), (1, "b")] {
        let pair = runtime.array_get(&fields, index).unwrap().to_owned();
        assert_eq!(
            runtime
                .text(runtime.field(&pair, 0).unwrap())
                .unwrap()
                .as_str(),
            name
        );
    }
    for (index, count) in [(14, 1), (15, 2)] {
        let items = success(index);
        assert_eq!(runtime.array_len(&items).unwrap(), count);
        let first = runtime.array_get(&items, 0).unwrap().to_owned();
        assert_eq!(runtime.dynamic_value(&first).unwrap().words()[2], 42);
    }
    assert_eq!(runtime.text(success(16).as_ref()).unwrap().as_str(), "Some");
    let some = success(17);
    let boxed = runtime.enum_payload(&some).unwrap().unwrap().to_owned();
    assert_eq!(runtime.dynamic_value(&boxed).unwrap().words()[2], 42);
    assert!(runtime.enum_payload(&success(18)).unwrap().is_none());
    let failed = runtime.field(&value, 19).unwrap().to_owned();
    assert_eq!(
        runtime
            .variant_name(failed.type_id(), runtime.enum_tag(&failed).unwrap())
            .unwrap(),
        "Err"
    );
    assert_eq!(runtime.text(success(20).as_ref()).unwrap().as_str(), "True");
    let field = runtime.field(&value, 21).unwrap().to_owned();
    assert_eq!(runtime.dynamic_value(&field).unwrap().words()[2], 42);
    assert_eq!(runtime.field(&value, 22).unwrap().words()[2], 1);
    let some = runtime.field(&value, 23).unwrap().to_owned();
    let child = runtime.enum_payload(&some).unwrap().unwrap().to_owned();
    assert_eq!(runtime.dynamic_value(&child).unwrap().words()[2], 42);
    assert!(
        runtime
            .enum_payload(&runtime.field(&value, 24).unwrap().to_owned())
            .unwrap()
            .is_none()
    );
    let array = payload.to_owned();
    assert_eq!(runtime.array_get(&array, 0).unwrap().words()[2], 42);
}

#[test]
fn native_string_operations_keep_unicode_newlines_and_shared_slices() {
    let (mir, root) = graph_with(
        include_str!("../../tests/fixtures/string.telora"),
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[mir.hir[root.index()].module], &[root]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    let value = compiled.call(&mut context, &[]).unwrap();
    let runtime = context.runtime().unwrap();
    for (index, expected) in [
        (0, "first long line|第二行|"),
        (1, "a\nb"),
        (6, "值-b-值"),
        (7, "  a\r\n\n  b"),
        (8, "\n"),
        (9, "a\r\nb\n  c"),
    ] {
        assert_eq!(
            runtime
                .text(runtime.field(&value, index).unwrap())
                .unwrap()
                .as_str(),
            expected
        );
    }
    for index in [3, 4, 5] {
        assert_eq!(runtime.field(&value, index).unwrap().words()[2], 1);
    }
    for (index, expected) in [
        (2, vec!["", "你", "好", ""]),
        (10, vec!["first long line", "第二行", ""]),
    ] {
        let array = runtime.field(&value, index).unwrap().to_owned();
        let texts = (0..runtime.array_len(&array).unwrap())
            .map(|i| {
                runtime
                    .text(runtime.array_get(&array, i).unwrap())
                    .unwrap()
                    .as_str()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(texts, expected);
    }
    let array = runtime.field(&value, 10).unwrap().to_owned();
    let first = runtime.array_get(&array, 0).unwrap();
    let second = runtime.array_get(&array, 1).unwrap();
    assert_eq!(first.words()[2] >> 32, second.words()[2] >> 32);
    assert!(
        runtime
            .string_slice(&second.to_owned(), 1, 2, [0, 0, 0])
            .is_err()
    );

    let (mir, root) = graph_with(
        include_str!("../../tests/fixtures/string-invalid.telora"),
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[mir.hir[root.index()].module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.initialize(&mut context).is_err());
    assert_eq!(context.diagnostics().len(), 2);
    assert!(!context.runtime().unwrap().is_published());
}

#[test]
fn native_regex_resources_validate_capture_contracts_and_publish_aliases() {
    let (mir, root) = graph_with(
        include_str!("../../tests/fixtures/regex.telora"),
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[mir.hir[root.index()].module], &[root]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    let value = compiled.call(&mut context, &[]).unwrap();
    let runtime = context.runtime().unwrap();
    assert_eq!(runtime.field(&value, 1).unwrap().words()[2], 1);
    assert_eq!(runtime.field(&value, 2).unwrap().words()[2], 0);
    let property = runtime.field(&value, 3).unwrap().to_owned();
    assert_eq!(
        runtime.field(&value, 0).unwrap().words(),
        runtime.field(&property, 0).unwrap().words()
    );

    let (mir, root) = graph_with(
        include_str!("../../tests/fixtures/regex-invalid.telora"),
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[mir.hir[root.index()].module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.initialize(&mut context).is_err());
    assert_eq!(context.diagnostics().len(), 3);
    assert!(!context.runtime().unwrap().is_published());
}

#[test]
fn dynamic_failure_messages_keep_subject_origins_and_fail_once() {
    let source = include_str!("../../tests/fixtures/failure-subjects.telora");
    let (mir, root) = graph(source);
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[mir.hir[root.index()].module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.initialize(&mut context).is_err());
    let diagnostic = &context.diagnostics()[0];
    assert_eq!(diagnostic.message, "computed message");
    assert_eq!(
        diagnostic.origin.words()[1] as usize,
        source.find("fail!").unwrap()
    );
    assert_eq!(diagnostic.subjects.len(), 2);
    assert_eq!(
        diagnostic.subjects[0].words()[1] as usize,
        source.find("42").unwrap()
    );
    assert_eq!(
        diagnostic.subjects[1].words()[1] as usize,
        source.find("\"subject\"").unwrap()
    );
    assert!(compiled.initialize(&mut context).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert!(!context.runtime().unwrap().is_published());
}

#[test]
fn native_format_nodes_publish_and_render_shared_inputs() {
    // Expose the module's private native primitive only in this test inventory.
    let fmt_source = format!(
        "{}\nexport {{prepare}};",
        include_str!("../../../telora-core/modules/std/fmt.telora")
    );
    let dependencies = static_sources::BUILTINS
        .iter()
        .map(|&(name, source)| {
            (
                name,
                if name == "std/fmt" {
                    fmt_source.as_str()
                } else {
                    source
                },
            )
        })
        .collect::<Vec<_>>();
    let (mir, root) = graph_with(
        include_str!("../../tests/fixtures/format.telora"),
        &dependencies,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[mir.hir[root.index()].module], &[root]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    let value = compiled.call(&mut context, &[]).unwrap();
    let runtime = context.runtime().unwrap();
    assert_eq!(
        runtime
            .text(runtime.field(&value, 1).unwrap())
            .unwrap()
            .as_str(),
        "[42, 1.25, text]"
    );
    let template = runtime.field(&value, 2).unwrap().to_owned();
    for (column, expected) in [
        (0, vec!["{", "}:", "", ""]),
        (1, vec!["name", "other", "name"]),
    ] {
        let values = runtime.field(&template, column).unwrap().to_owned();
        let texts = (0..runtime.array_len(&values).unwrap())
            .map(|index| {
                runtime
                    .text(runtime.array_get(&values, index).unwrap())
                    .unwrap()
                    .as_str()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(texts, expected);
    }
    let (mir, root) = graph_with(
        include_str!("../../tests/fixtures/format-invalid.telora"),
        &dependencies,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[mir.hir[root.index()].module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.initialize(&mut context).is_err());
    assert_eq!(context.diagnostics().len(), 3);
    assert!(!context.runtime().unwrap().is_published());
}

#[test]
fn invalid_dynamic_indices_fail_once_and_block_publication() {
    let (mir, root) = graph_with(
        include_str!("../../tests/fixtures/dynamic-invalid.telora"),
        static_sources::BUILTINS,
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[mir.hir[root.index()].module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.initialize(&mut context).is_err());
    assert_eq!(context.diagnostics().len(), 3);
    assert!(!context.runtime().unwrap().is_published());
    assert!(compiled.initialize(&mut context).is_err());
    assert_eq!(context.diagnostics().len(), 3);
}

#[test]
fn enum_constructors_are_first_class_closed_functions() {
    let (mir, root) = graph("export def answer: Fn(Int) -> Option(Int) = Some;");
    let module = mir.hir[root.index()].module;
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    let closure = compiled
        .export(&mut context, mir.exports[module.index()][0])
        .unwrap();
    let argument_ty = mir.types[closure.type_key().index()].arguments[0];
    let argument = compiled
        .layouts()
        .value(
            TypeKey::try_from(argument_ty).unwrap(),
            Origin::default(),
            &[42],
        )
        .unwrap();
    let value = compiled
        .call_closure(&mut context, &closure, &[argument])
        .unwrap();
    assert_eq!(
        context
            .runtime()
            .unwrap()
            .enum_payload(&value)
            .unwrap()
            .unwrap()
            .words()[2],
        42
    );
}

#[test]
fn host_calls_published_closures_with_closed_signatures() {
    let (mir, root) = graph(
        "decl answer: Fn(Int) -> Int; export def answer = do { let captured = 40; fn(value) { captured + value } };",
    );
    let module = mir.hir[root.index()].module;
    let declaration = mir.exports[module.index()][0];
    let TypeState::Known(ty) = mir.ty_slots[mir.symbol_types[declaration.index()].index()] else {
        panic!("closed function signature")
    };
    let closure_ty = TypeKey::try_from(ty).unwrap();
    let argument_ty = mir.types[closure_ty.index()].arguments[0];
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    let closure = compiled.export(&mut context, declaration).unwrap();
    let argument = compiled
        .layouts()
        .value(
            TypeKey::try_from(argument_ty).unwrap(),
            Origin::default(),
            &[2],
        )
        .unwrap();
    let result = compiled
        .call_closure(&mut context, &closure, &[argument])
        .unwrap();
    assert_eq!(result.words()[2], 42);
    assert!(compiled.call_closure(&mut context, &closure, &[]).is_err());
    let forged = context
        .runtime_mut()
        .unwrap()
        .closure(closure_ty, [0, 0, 0], u32::MAX, &[])
        .unwrap();
    assert!(compiled.call_closure(&mut context, &forged, &[]).is_err());
}

#[test]
fn whole_graph_initializes_prelude_and_application() {
    let (mir, root) = graph("export def answer = 42;");
    let modules = mir
        .hir
        .iter()
        .map(|node| node.module)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &modules, &[root]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);
}

#[test]
fn module_initialization_consumes_closed_generic_instances() {
    for (source, expected) in [
        (
            include_str!("../../tests/fixtures/generics.telora"),
            "native generic text",
        ),
        (
            include_str!("../../tests/fixtures/generic-initialization.telora"),
            "selected",
        ),
    ] {
        let (mir, root) = graph(source);
        let module = mir.hir[root.index()].module;
        let sealed = mir.seal().unwrap();
        let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        compiled.initialize(&mut context).unwrap();
        let symbol = *mir.exports[module.index()]
            .iter()
            .find(|s| mir.symbols[s.index()].name == "answer")
            .unwrap();
        let value = compiled.export(&mut context, symbol).unwrap();
        let runtime = context.runtime().unwrap();
        assert_eq!(runtime.field(&value, 0).unwrap().words()[2], 42);
        assert_eq!(
            runtime
                .text(runtime.field(&value, 1).unwrap())
                .unwrap()
                .as_str(),
            expected
        );
        assert!(
            compiled
                .demands
                .iter()
                .any(|(key, _)| matches!(key, crate::runtime::DemandKey::Instance(_)))
        );
        compiled.initialize(&mut context).unwrap();
        let again = compiled.call(&mut context, &[]).unwrap();
        let runtime = context.runtime().unwrap();
        assert_eq!(runtime.field(&again, 0).unwrap().words()[2], 42);
        assert_eq!(
            runtime
                .text(runtime.field(&again, 1).unwrap())
                .unwrap()
                .as_str(),
            expected
        );
    }
}

#[test]
fn module_initialization_includes_unused_values_and_publishes_once() {
    let (mir, root) = graph("def unused = [1, 2]; def base = 40; export def answer = base + 2;");
    let module = mir.hir[root.index()].module;
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    compiled.initialize(&mut context).unwrap();
    assert!(context.runtime().unwrap().is_published());
    let symbol = *mir.exports[module.index()]
        .iter()
        .find(|s| mir.symbols[s.index()].name == "answer")
        .unwrap();
    assert_eq!(
        compiled.export(&mut context, symbol).unwrap().words()[2],
        42
    );
    assert_eq!(compiled.demands.len(), 3);
    compiled.initialize(&mut context).unwrap();

    let (mir, root) = graph("def unused: Int = fail!(\"unused failed\"); export def answer = 42;");
    let module = mir.hir[root.index()].module;
    let sealed = mir.seal().unwrap();
    let compiled = compile_modules(&sealed, &[module], &[]).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.initialize(&mut context).is_err());
    assert!(!context.runtime().unwrap().is_published());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "unused failed");
    assert!(compiled.initialize(&mut context).is_err());
    assert_eq!(context.diagnostics().len(), 1);
}

#[test]
fn generated_global_reads_initialize_once_and_propagate_cycles() {
    let (mir, root) = graph(
        "def items = [40, 2]; def total = items[0] + items[1]; export def answer = (items, items, total);",
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let result = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime_mut().unwrap();
    assert_eq!(
        rt.field(&result, 0).unwrap().words(),
        rt.field(&result, 1).unwrap().words()
    );
    assert_eq!(rt.field(&result, 2).unwrap().words()[2], 42);
    let roots = rt.publish(&[result]).unwrap();
    let again = compiled.call(&mut context, &[]).unwrap();
    let rt = context.runtime().unwrap();
    assert_eq!(
        rt.field(&roots[0], 0).unwrap().words(),
        rt.field(&again, 0).unwrap().words()
    );

    let (mir, root) = graph("def a: Int = b; def b: Int = a; export def answer = a;");
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert!(
        context.diagnostics()[0]
            .message
            .contains("dependency cycle")
    );
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert!(context.runtime_mut().unwrap().publish(&[]).is_err());

    let (mir, root) = graph(
        "def make: Fn(Int) -> Fn(Int) -> Int = fn(base) { fn(x) { base + x } }; def add = make(40); export def answer = add(2);",
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);
    context.runtime_mut().unwrap().publish(&[]).unwrap();
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);
}

#[test]
fn one_code_plan_runs_initialization_then_calls_published_closures() {
    let (mir, consumer) = graph(
        "def make: Fn(Int) -> Fn(Int) -> Int = fn(base) { fn(x) { base + x } }; export def answer: Fn(Fn(Int) -> Int, Int) -> Int = fn(f, x) { f(x) };",
    );
    let make = crate::test_support::value_node(&mir, "make");
    let sealed = mir.seal().unwrap();
    let compiled = compile_roots(&sealed, &[consumer, make, make]).unwrap();
    assert_eq!(compiled.entries.len(), 2);
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    let int = compiled.root_signature(make).unwrap().0[0];
    let base = compiled
        .layouts()
        .value(int, Origin::default(), &[40])
        .unwrap();
    let closure = compiled.call_root(make, &mut context, &[base]).unwrap();
    let published = context
        .runtime_mut()
        .unwrap()
        .publish(&[closure.clone()])
        .unwrap();
    let two = compiled
        .layouts()
        .value(int, Origin::default(), &[2])
        .unwrap();
    assert!(
        compiled
            .call_root(consumer, &mut context, &[closure, two.clone()])
            .is_err()
    );
    let result = compiled
        .call_root(consumer, &mut context, &[published[0].clone(), two])
        .unwrap();
    assert_eq!(result.words()[2], 42);
    assert!(context.diagnostics().is_empty());
}

#[test]
fn machine_code_reads_lexical_closure_environments() {
    let (mir, root) = graph(
        "export def answer = do { let base = 40; let add: Fn(Int) -> Int = fn(x) { base + x }; add(2) };",
    );
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    assert_eq!(compiled.call(&mut context, &[]).unwrap().words()[2], 42);
}

#[test]
fn indirect_calls_consume_closed_signatures_and_returned_environments() {
    for source in [
        "def apply: Fn(Fn(Int) -> Int, Int) -> Int = fn(f, x) { f(x) }; export def answer = apply(fn(x) { x + 2 }, 40);",
        "def inc: Fn(Int) -> Int = fn(x) { x + 2 }; def apply: Fn(Fn(Int) -> Int, Int) -> Int = fn(f, x) { f(x) }; export def answer = apply(inc, 40);",
        "def identity: for(T) Fn(T) -> T = fn(x) { x }; def apply: for(T) Fn(Fn(T) -> T, T) -> T = fn(f, x) { f(x) }; export def answer = apply(identity@[Int], 42);",
        "def make: Fn(Int) -> Fn(Int) -> Int = fn(base) { fn(x) { base + x } }; export def answer = do { let f = make(40); f(2) };",
        "export def answer = do { let a: Fn(Int) -> Int = fn(x) { x + 2 }; let b: Fn(Int) -> Int = fn(x) { x + 3 }; let f = if False { b } else { a }; f(40) };",
    ] {
        let (mir, root) = graph(source);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        assert_eq!(
            compiled.call(&mut context, &[]).unwrap().words()[2],
            42,
            "{source}"
        );
    }
}

#[test]
fn runtime_rejects_a_different_executable_plan() {
    let (mir, root) = graph("export def answer = 42;");
    let sealed = mir.seal().unwrap();
    let first = compile(&sealed, root).unwrap();
    let second = compile(&sealed, root).unwrap();
    let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
    first.call(&mut context, &[]).unwrap();
    assert!(
        second
            .call(&mut context, &[])
            .unwrap_err()
            .contains("another code plan")
    );
    assert!(context.diagnostics().is_empty());
    first.call(&mut context, &[]).unwrap();
}

#[test]
fn indirect_unknown_function_reports_one_failure_without_reading_result() {
    let (mir, root) = graph("export def answer: Fn(Fn(Int) -> Int) -> Int = fn(f) { f(42) };");
    let sealed = mir.seal().unwrap();
    let compiled = compile(&sealed, root).unwrap();
    let mut runtime = crate::runtime::Runtime::new(&sealed).unwrap();
    let invalid = runtime
        .closure(compiled.arguments()[0], [1, 0, 1], u32::MAX, &[])
        .unwrap();
    let mut context = CallContext::with_runtime(runtime);
    assert!(compiled.call(&mut context, &[invalid]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert!(
        context.diagnostics()[0]
            .message
            .contains("closed call signature")
    );
}

#[test]
fn patterns_select_payloads_and_preserve_diverging_paths() {
    for source in [
        "export def answer = match 2 { 1 => 0, 2 => 42, _ => 7 };",
        "export def answer = match Some(42) { Some(x) => x, None => 0 };",
        "export def answer = match (2, 40) { (a, b) => a + b };",
        "export def answer = if let Some(x) = Some(42) { x } else { 0 };",
        "export def answer = match \"yes\" { \"no\" => 0, \"yes\" => 42, _ => 7 };",
        "export def answer = match 2 { 1 => fail!(\"wrong\"), _ => 42 };",
    ] {
        let (mir, root) = graph(source);
        let sealed = mir.seal().unwrap();
        let compiled = compile(&sealed, root).unwrap();
        let mut context = CallContext::with_runtime(crate::runtime::Runtime::new(&sealed).unwrap());
        assert_eq!(
            compiled.call(&mut context, &[]).unwrap().words()[2],
            42,
            "{source}"
        );
        assert!(context.diagnostics().is_empty());
    }
}

#[test]
fn never_paths_do_not_allocate_values_or_force_a_join_result() {
    for source in [
        "export def answer: Fn(Bool) -> Int = fn(flag) { if flag { return 42; } else { 7 } };",
        "export def answer: Fn(Bool) -> Int = fn(flag) { if flag { return 42; } else { return 7; } };",
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        for (flag, expected) in [(1, 42), (0, 7)] {
            let flag = compiled
                .layouts()
                .value(compiled.arguments()[0], Origin::default(), &[flag])
                .unwrap();
            assert_eq!(
                compiled
                    .call(&mut CallContext::default(), &[flag])
                    .unwrap()
                    .words()[2],
                expected
            );
        }
    }
    let (mir, root) = graph(
        "def explode: Fn() -> Never = fn() { fail!(\"boom\") }; export def answer = explode();",
    );
    let compiled = compile(&mir.seal().unwrap(), root).unwrap();
    assert!(compiled.layouts().is_never(compiled.output()).unwrap());
    let mut context = CallContext::default();
    assert!(compiled.call(&mut context, &[]).is_err());
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "boom");
    for source in [
        "export def answer = True || fail!(\"unreachable\");",
        "export def answer = False && fail!(\"unreachable\");",
    ] {
        let (mir, root) = graph(source);
        let compiled = compile(&mir.seal().unwrap(), root).unwrap();
        let mut context = CallContext::default();
        compiled.call(&mut context, &[]).unwrap();
        assert!(context.diagnostics().is_empty());
    }
}
