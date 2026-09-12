#[test]
fn static_templates_have_only_concrete_instance_execution_tasks() {
    let mir = graph(r#"
        export def unused: for(T) Fn(T) -> T = fn(value) { value };
        def identity: for(T) Fn(T) -> T = fn(value) { value };
        export def answer = if identity@[Int] == identity@[Int] { 42 } else { 0 };
    "#, "");
    let sealed = mir.seal().unwrap();
    let execution = crate::execution_graph::ExecutionGraph::from_mir(&sealed);
    for node in execution.nodes() {
        if let crate::execution_graph::Task::Global { symbol, .. } = node.task {
            assert!(mir.symbol_generics[symbol.index()].is_empty());
        }
        assert!(!node.label.ends_with("::unused"));
    }
    assert!(execution.nodes().iter().any(|node|
        matches!(node.task, crate::execution_graph::Task::Instance { .. })));
    let artifact = compile(sealed, entry(&mir)).unwrap();
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
}

#[test]
fn property_admission_initializes_all_capabilities_and_detects_cycles() {
    for (target, query, cycle) in [
        ("fail!(\"unused capability must stay lazy\")", "get_type_prop(Int.type, Mark.type)", false),
        ("do { let value = get_type_prop(Item.type, Mark.type); PropertyTarget.Type }", "get_type_prop(Item.type, Mark.type)", true),
    ] {
        let source = format!(r#"
            import "std/type-property" {{ get_type_prop }};
            def choose: Fn() -> PropertyTarget = fn() {{ {target} }};
            @property(choose()) type Mark = struct {{value: Int}};
            def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) {{ {{value: 42}} }};
            @mark type Item = struct {{value: Int}};
            export def answer = match {query} {{ Some(p) => p.value, None => 42 }};
        "#);
        let mir = graph(&source, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let error = execute(artifact).err().expect("initialization must fail");
        assert!(error.contains(if cycle { "cyclic demand" } else { "unused capability must stay lazy" }), "{error}");
    }
}

#[test]
fn property_admission_checks_dynamic_capabilities_for_every_owner_kind() {
    for (context, declaration, query, allowed) in [
        ("Type", "@mark type Owner = struct {value: Int};", "get_type_prop(Owner.type, Mark.type)", 3),
        ("Type", "@mark type Owner = struct(Int);", "get_type_prop(Owner.type, Mark.type)", 3),
        ("Type", "@mark type Owner = enum {Value(Int)};", "get_type_prop(Owner.type, Mark.type)", 5),
        ("FieldPropertyCtx", "type Owner = struct {@mark value: Int};", "get_field_prop(Owner.type, 0, Mark.type)", 24),
        ("VariantPropertyCtx", "type Owner = enum {@mark Value(Int)};", "get_variant_prop(Owner.type, 0, Mark.type)", 40),
    ] {
        for (target, bit) in [("Type", 1), ("StructType", 2), ("EnumType", 4), ("Member", 8), ("Field", 16), ("Variant", 32)] {
            let source = format!(r#"
                import "std/type-property" {{ get_type_prop, get_field_prop, get_variant_prop, FieldPropertyCtx, VariantPropertyCtx }};
                import "std/_rt" as rt;
                def attach = property;
                def choose: Fn(Bool) -> PropertyTarget = fn(flag) {{ if flag {{ PropertyTarget.{target} }} else {{ PropertyTarget.Type }} }};
                @attach(choose(True)) type Mark = struct {{value: Int}};
                def mark: Fn({context}, Option(Mark)) -> Mark = fn(owner, previous) {{ {{value: 42}} }};
                {declaration}
                export def answer = match rt.with_diagnostics(fn(n: Int) {{ match {query} {{ Some(p) => p.value, None => 0 }} }})(0) {{
                    Ok((value, _)) => value,
                    Err(errors) => if errors[0].message == "property type does not support this decorator target" {{ -1 }} else {{ -2 }}
                }};
            "#);
            let mir = graph(&source, "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            let result = execute(artifact);
            if allowed & bit != 0 {
                assert_eq!(result.unwrap().value().as_int(), Some(42), "{source}");
            } else {
                assert!(result.is_err(), "invalid property must prevent session publication: {source}");
            }
        }
    }
}

#[test]
fn generic_property_thunks_consume_closed_owner_and_provider_types() {
    let mir = graph(r#"
        import "std/type-property" { get_type_prop, get_field_prop, get_variant_prop, FieldPropertyCtx, VariantPropertyCtx };
        @property(PropertyTarget.Type) type Mark(T) = struct { witness: TypeOf(T), count: Int };
        def mark: for(T) Fn(TypeOf(T)) -> Fn(Type, Option(Mark(T))) -> Mark(T) = fn(witness) {
            fn(owner, previous) { {witness: witness, count: 1 + match previous { Some(p) => p.count, None => 0 }} }
        };
        @property(PropertyTarget.Field) type FieldMark = struct { context: FieldPropertyCtx };
        @property(PropertyTarget.Variant) type VariantMark = struct { context: VariantPropertyCtx };
        def field: Fn(FieldPropertyCtx, Option(FieldMark)) -> FieldMark = fn(ctx, previous) { {context: ctx} };
        def variant: Fn(VariantPropertyCtx, Option(VariantMark)) -> VariantMark = fn(ctx, previous) { {context: ctx} };
        @mark(T.type) @mark(T.type) type Box(T) = struct { @field value: T };
        type Choice(T) = enum { @variant Empty, @variant Value(T) };
        export def answer = do {
            let a = match get_type_prop(Box(Int).type, Mark(Int).type) { Some(p) => p, None => fail!("missing Int mark") };
            let b = match get_type_prop(Box(String).type, Mark(String).type) { Some(p) => p, None => fail!("missing String mark") };
            let f = match get_field_prop(Box(String).type, 0, FieldMark.type) { Some(p) => p.context, None => fail!("missing field mark") };
            let v = match get_variant_prop(Choice(Int).type, 1, VariantMark.type) { Some(p) => p.context, None => fail!("missing variant mark") };
            let e = match get_variant_prop(Choice(Int).type, 0, VariantMark.type) { Some(p) => p.context, None => fail!("missing empty mark") };
            if a.witness == Int.type && b.witness == String.type && a.count == 2 && b.count == 2
                && f.owner == Box(String).type && f.ty == String.type
                && v.owner == Choice(Int).type && v.payload == Some(Int.type) && e.payload == None { 42 } else { 0 }
        };
    "#, "");
    let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
    let result = execute(artifact).unwrap();
    assert_eq!(result.value().as_int(), Some(42));
}

#[test]
fn member_properties_receive_solved_skeleton_contexts() {
    let mir = graph(
        r#"
        import "std/type-property" { get_field_prop, get_variant_prop, FieldPropertyCtx, VariantPropertyCtx };
        @property(PropertyTarget.Field) type FieldMark = struct { context: FieldPropertyCtx };
        @property(PropertyTarget.Variant) type VariantMark = struct { context: VariantPropertyCtx };
        def field: Fn(FieldPropertyCtx, Option(FieldMark)) -> FieldMark = fn(ctx, previous) { { context: ctx } };
        def variant: Fn(VariantPropertyCtx, Option(VariantMark)) -> VariantMark = fn(ctx, previous) { { context: ctx } };
        type Item = struct { @field first: Int, @field second: String };
        type Choice = enum { @variant Empty, @variant Value(Int) };
        export def answer = (get_field_prop(Item.type, 1, FieldMark.type),
            get_variant_prop(Choice.type, 0, VariantMark.type),
            get_variant_prop(Choice.type, 1, VariantMark.type), Item.type, Choice.type, String.type, Int.type);
    "#,
        "",
    );
    let result = execute(compile(mir.seal().unwrap(), entry(&mir)).unwrap()).unwrap();
    for (index, name, position, owner_index) in
        [(0, "second", 1, 3), (1, "Empty", 0, 4), (2, "Value", 1, 4)]
    {
        let (_, mark) = result
            .value()
            .sequence_get(index)
            .unwrap()
            .tagged_parts()
            .unwrap();
        let context = mark.dict_get("context").unwrap();
        assert_eq!(
            context.dict_get("name").unwrap().as_str().unwrap().as_str(),
            name
        );
        assert_eq!(context.dict_get("index").unwrap().as_int(), Some(position));
        assert_eq!(
            context.dict_get("owner").unwrap().represented_type_id(),
            result
                .value()
                .sequence_get(owner_index)
                .unwrap()
                .represented_type_id()
        );
        if index == 0 {
            assert_eq!(
                context.dict_get("ty").unwrap().represented_type_id(),
                result
                    .value()
                    .sequence_get(5)
                    .unwrap()
                    .represented_type_id()
            );
        } else if index == 1 {
            assert_eq!(
                context
                    .dict_get("payload")
                    .unwrap()
                    .as_atom()
                    .unwrap()
                    .as_str(),
                "None"
            );
        } else {
            let (_, payload) = context.dict_get("payload").unwrap().tagged_parts().unwrap();
            assert_eq!(
                payload.represented_type_id(),
                result
                    .value()
                    .sequence_get(6)
                    .unwrap()
                    .represented_type_id()
            );
        }
    }
}

#[test]
fn property_queries_reduce_once_and_share_lazy_global_dependencies() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    CALLS.store(0, Ordering::SeqCst);
    let mir = graph(
        r#"
        import "std/type-property" { get_type_prop as query };
        native tick: Fn() -> Int;
        @property(PropertyTarget.Type)
        @property(PropertyTarget.Field)
        type Mark = struct { value: Int };
        def config = tick();
        def make: Fn(Int) -> Fn(Type, Option(Mark)) -> Mark = fn(n) {
            fn(owner, previous) { let counted = tick(); { value: config + n } }
        };
        @make(1)
        @make(2)
        type Item = struct { value: Int };
        export def answer = (query(Item.type, Mark.type), (fn(read) { read(Item.type, Mark.type) })(query),
            query(Int.type, Mark.type), query(Mark.type, PropertyAttr.type));
    "#,
        "",
    );
    let mut artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    artifact.bytecode = crate::execution_link::link_with(&artifact, |_| {
        Some(crate::NativeFunction::new("test.tick", 0, |ctx| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            ctx.set_int(ctx.result(), 21)
        }))
    })
    .unwrap();
    artifact.native_links.clear();
    let result = execute(artifact).unwrap();
    for index in [0, 1] {
        let (tag, payload) = result
            .value()
            .sequence_get(index)
            .unwrap()
            .tagged_parts()
            .unwrap();
        assert_eq!(tag.as_atom().unwrap().as_str(), "Some");
        assert_eq!(payload.dict_get("value").unwrap().as_int(), Some(23));
    }
    assert_eq!(
        result
            .value()
            .sequence_get(2)
            .unwrap()
            .as_atom()
            .unwrap()
            .as_str(),
        "None"
    );
    let (_, attr) = result
        .value()
        .sequence_get(3)
        .unwrap()
        .tagged_parts()
        .unwrap();
    assert_eq!(attr.dict_get("bits").unwrap().as_int(), Some(17));
    assert!(attr.solved_type_id().is_some());
    assert_eq!(CALLS.load(Ordering::SeqCst), 3);
}

#[test]
fn property_initialization_fails_even_when_entry_queries_an_absent_property() {
    for (query, expected) in [
        ("query(Int.type, Mark.type)", Some("provider failed")),
        ("query(Item.type, Mark.type)", Some("provider failed")),
    ] {
        let source = format!(
            r#"
            import "std/type-property" {{ get_type_prop as query }};
            @property(PropertyTarget.Type) type Mark = struct {{ value: Int }};
            def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) {{ fail!("provider failed") }};
            @mark type Item = struct {{ value: Int }};
            export def answer = {query};
        "#
        );
        let mir = graph(&source, "");
        let result = execute(compile(mir.seal().unwrap(), entry(&mir)).unwrap());
        if let Some(expected) = expected {
            assert!(result.err().unwrap().contains(expected));
        } else {
            assert_eq!(result.unwrap().value().as_atom().unwrap().as_str(), "None");
        }
    }
    let mir = graph(
        r#"
        import "std/type-property" { get_type_prop as query };
        @property(PropertyTarget.Type) type Mark = struct { value: Int };
        def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { let dependency = a; { value: 1 } };
        @mark type Item = struct { value: Int };
        def a: Option(Mark) = query(Item.type, Mark.type);
        export def answer = a;
    "#,
        "",
    );
    let error = execute(compile(mir.seal().unwrap(), entry(&mir)).unwrap())
        .err()
        .unwrap();
    assert!(
        error.contains("cyclic demand") && error.contains("property(") && error.contains("::a"),
        "{error}"
    );
}
#[test]
fn type_metadata_retains_solved_ids_after_property_initialization() {
    let mir = graph(
        r#"
        @property(PropertyTarget.Type)
        type Mark = struct { value: Int };
        def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: 42} };
        @mark
        type Item = struct { next: Array(Item) };
        type Alias = Item;
        export def answer = (Int.type, Array(Int).type, Item.type, Alias.type);
    "#,
        "",
    );
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    // Returning metadata still requires full session initialization.
    let property_tasks = artifact.graph.nodes().iter().filter_map(|node| {
        if let crate::execution_graph::Task::Property { key, .. } = node.task {
            artifact.graph.property(key)
        } else { None }
    }).collect::<Vec<_>>();
    assert!(!property_tasks.is_empty());
    for task in property_tasks {
        assert!(artifact.bytecode.instructions().iter().any(|instruction|
            matches!(instruction, crate::bytecode::Opcode::InstallTask { node, .. } if *node == task)));
    }
    let types = &artifact.types;
    let expected = types.types[artifact.result_type.index()]
        .arguments
        .iter()
        .map(|id| types.types[id.index()].arguments[0])
        .collect::<Vec<_>>();
    let result = execute(artifact).unwrap();
    for (index, id) in expected.iter().enumerate() {
        let metadata = result.value().sequence_get(index).unwrap();
        assert_eq!(metadata.kind(), crate::ValueKind::Type);
        assert_eq!(metadata.represented_type_id(), Some(*id));
        assert!(metadata.as_int().is_none());
    }
    assert_eq!(expected[2], expected[3]);
    let mir = graph(
        "type Alias = Int; export def answer = if Int.type == Alias.type { 42 } else { 0 };",
        "",
    );
    assert_eq!(
        execute(compile(mir.seal().unwrap(), entry(&mir)).unwrap())
            .unwrap()
            .value()
            .as_int(),
        Some(42)
    );
}

#[test]
fn generic_metadata_uses_closed_mir_instances_through_calls_and_recursion() {
    let mir = graph(r#"
        def metadata: for(T) Fn(T) -> TypeOf(Array(T)) = fn(value) { Array(T).type };
        def apply: for(A, B) Fn(Fn(A) -> B, A) -> B = fn(f, value) { f(value) };
        def repeated: for(T) Fn(T, Int) -> TypeOf(Array(T)) = fn(value, n) {
            if n > 0 { repeated(value, n - 1) } else { metadata(value) }
        };
        export def answer = (metadata(42), apply(metadata, "ok"), repeated(True, 3));
    "#, "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let expected = artifact.types.types[artifact.result_type.index()].arguments.iter()
        .map(|ty| artifact.types.types[ty.index()].arguments[0]).collect::<Vec<_>>();
    assert!(artifact.graph.nodes().iter().any(|node| matches!(node.task, crate::execution_graph::Task::Instance { .. })));
    // Neither HIR nor the solver survives into runtime execution.
    drop(mir);
    let result = execute(artifact).unwrap();
    for (index, ty) in expected.into_iter().enumerate() {
        assert_eq!(result.value().sequence_get(index).unwrap().represented_type_id(), Some(ty));
    }
}

#[test]
fn generic_nominal_constructors_consume_instance_type_ids() {
    let mir = graph(r#"
        type Wrapped(T) = struct(T);
        def make: for(T) Fn(T) -> Wrapped(T) = fn(value) { Wrapped(T)(value) };
        export def answer = (make(42), make("ok"));
    "#, "");
    let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
    let expected = artifact.types.types[artifact.result_type.index()].arguments.clone();
    drop(mir);
    let result = execute(artifact).unwrap();
    for (index, ty) in expected.into_iter().enumerate() {
        assert_eq!(result.value().sequence_get(index).unwrap().solved_type_id(), Some(ty));
    }
}
fn execute(artifact: CompiledEntry) -> Result<crate::execution_link::SolvedExecution, String> {
    let linked = crate::execution_link::link_entry(artifact).map_err(|d| format!("{d:?}"))?;
    crate::Vm::new().execute_linked(
        linked,
        crate::Quota::with_fuel(10000),
        crate::DataLimits::default(),
        &mut crate::SourceDatabase::default(),
    )
}

#[test]
fn global_result_is_computed_once_across_repeated_reads_and_calls() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    CALLS.store(0, Ordering::SeqCst);
    let mir = graph(
        "native tick: Fn() -> Int; def cached = tick(); def read: Fn() -> Int = fn() { cached }; export def answer = cached + read();",
        "",
    );
    let mut artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    artifact.bytecode = crate::execution_link::link_with(&artifact, |_| {
        Some(crate::NativeFunction::new("test.tick", 0, |ctx| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            ctx.set_int(ctx.result(), 21)
        }))
    })
    .unwrap();
    artifact.native_links.clear();
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
}

#[test]
fn entry_codegen_initializes_unreferenced_globals_and_instances() {
    let mir = graph(r#"
        def identity: for(T) Fn(T) -> T = fn(value) { value };
        def unused: Int = identity(1 / 0);
        export def answer = 42;
    "#, "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let mut globals = 0;
    let mut instances = 0;
    for task in artifact.graph.nodes() {
        let node = match task.task {
            crate::execution_graph::Task::Global { symbol, .. }
                if mir.symbols[symbol.index()].name == "unused" => {
                globals += 1;
                artifact.graph.global(symbol).unwrap()
            }
            crate::execution_graph::Task::Instance { instance, symbol, .. }
                if mir.symbols[symbol.index()].name == "identity" => {
                instances += 1;
                artifact.graph.instance(instance).unwrap()
            }
            _ => continue,
        };
        assert!(artifact.bytecode.instructions().iter().any(|instruction|
            matches!(instruction, crate::bytecode::Opcode::InstallTask { node: installed, .. } if *installed == node)));
    }
    assert_eq!((globals, instances), (1, 1));
    drop(mir);
    assert!(execute(artifact).err().unwrap().contains("division by zero"));
}

#[test]
fn global_demands_allow_recursive_functions_and_ignore_uncalled_dependencies() {
    for source in [
        "def recurse: Fn(Int) -> Int = fn(n) { if n > 0 { recurse(n - 1) } else { 42 } }; export def answer = recurse(5);",
        "def a: Int = (fn(f) { 42 })(fn(x: Int) { b }); def b: Int = a; export def answer = a;",
        "def unused: Fn() -> Int = fn() { 1 / 0 }; export def answer = if True { 42 } else { unused() };",
    ] {
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    }
    let mir = graph("def a: Int = b; def b: Int = a; export def answer = a;", "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let error = execute(artifact)
        .err()
        .expect("actual value cycle must fail");
    assert!(error.contains("cyclic demand"), "{error}");
}
#[test]
fn execution_graph_uses_sealed_identities_and_one_slot_per_property_chain() {
    use crate::execution_graph::{ExecutionGraph, PropertyKey, Request, Task};
    let source = r#"
        import "./math" { config as first };
        import "./math" { config as second };
        @property(PropertyTarget.Type)
        type Label = struct { value: Int };
        def label: Fn(Type, Option(Label)) -> Label = fn(owner, previous) { { value: first } };
        @label
        @label
        type Item = struct { value: Int };
        export def answer = second;
    "#;
    let mut baseline = None;
    for order in [0, 1, 7] {
        let mir = graph_order(source, "export def config = 42;", order);
        let sealed = mir.seal().unwrap();
        let graph = ExecutionGraph::from_mir(&sealed);
        let symbol = |name: &str| {
            SymbolId(mir.symbols.iter().position(|s| s.name == name).unwrap() as u32)
        };
        assert_eq!(
            graph.global(symbol("first")),
            graph.global(symbol("config"))
        );
        assert_eq!(
            graph.global(symbol("second")),
            graph.global(symbol("config"))
        );
        assert!(graph.global(symbol("Item")).is_none());
        let chain = graph
            .nodes()
            .iter()
            .find_map(|node| match &node.task {
                Task::Property { key, providers } if providers.len() == 2 => {
                    Some((*key, providers))
                }
                _ => None,
            })
            .expect("two decorators reduce into one property node");
        let record = mir
            .properties
            .iter()
            .find(|p| p.providers.len() == 2)
            .unwrap();
        assert_eq!(chain.1.as_ref(), record.providers);
        assert!(
            graph
                .property(PropertyKey {
                    owner: chain.0.property,
                    ..chain.0
                })
                .is_none()
        );
        let property = graph.property(chain.0).unwrap();
        let config = graph.global(symbol("config")).unwrap();
        let mut execution = graph.evaluation();
        assert_eq!(execution.request(property), Ok(Request::Start));
        assert_eq!(execution.request(config), Ok(Request::Start));
        execution.complete(config, 42).unwrap();
        execution.complete(property, 42).unwrap();
        assert_eq!(execution.request(property), Ok(Request::Ready(&42)));
        let dump = format!("{graph:?}");
        if let Some(baseline) = &baseline {
            assert_eq!(&dump, baseline);
        } else {
            baseline = Some(dump);
        }
    }
}

pub(crate) fn graph(main: &str, math: &str) -> Mir {
    graph_order(main, math, 0)
}
fn graph_order(main: &str, math: &str, order: usize) -> Mir {
    let mut inventory = crate::static_sources::BUILTINS
        .iter()
        .map(|(name, _)| crate::module_resolve::ModuleSpec {
            name: (*name).into(),
            kind: ModuleKind::Source,
            native: crate::static_sources::native_module(name),
            implicit_imports: if *name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect::<Vec<_>>();
    for name in ["@src/main", "@src/math"] {
        inventory.push(crate::module_resolve::ModuleSpec {
            name: name.into(),
            kind: ModuleKind::Source,
            native: None,
            implicit_imports: vec!["std/prelude".into()],
        });
    }
    if order > 0 {
        inventory.reverse();
        let length = inventory.len();
        inventory.rotate_left(order % length);
    }
    let mut mir =
        crate::module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
            Ok(if name == "@src/main" {
                main.into()
            } else if name == "@src/math" {
                math.into()
            } else {
                crate::static_sources::BUILTINS
                    .iter()
                    .find(|(n, _)| *n == name)
                    .unwrap()
                    .1
                    .into()
            })
        });
    crate::symbol_resolve::resolve(&mut mir);
    crate::type_resolve::resolve(&mut mir);
    mir
}
pub(crate) fn entry(mir: &Mir) -> SymbolId {
    let ModuleTarget::Bound(module) = mir.roots[0] else {
        panic!("root");
    };
    *mir.exports[module.index()]
        .iter()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap()
}
#[test]
fn sealed_full_build_is_independent_of_inventory_enumeration_order() {
    let main = "import \"./math\" { identity }; \
                import \"./math\" { choose }; \
                import \"std/array\" { map, fold }; \
                def selected = choose@[Int, _]; \
                def seed = selected(0, \"seed\"); \
                def other = selected(0, True); \
                @property(PropertyTarget.Type) type Mark = struct { value: Int }; \
                def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: seed} }; \
                @mark \
                type Tree = enum { Leaf(Int), Branch((Tree, Tree)) }; \
                export def answer = fold(map([1, 2, 3], fn(x) { identity(x * 7) }), seed + other, fn(a, b) { a + b });";
    let math = "export def identity: for(T) Fn(T) -> T = fn(x) { x }; \
                export def choose: for(A, B) Fn(A, B) -> A = fn(a, b) { a };";
    let baseline = graph_order(main, math, 0);
    let sealed = baseline.seal().unwrap();
    let expected_image = format!("{:?}", sealed.types());
    let artifact = compile(sealed, entry(&baseline)).unwrap();
    for order in [1, 7, 19] {
        let rebuilt = graph_order(main, math, order);
        let sealed = rebuilt.seal().unwrap();
        assert_eq!(sealed.mir().dump(), baseline.dump());
        assert_eq!(format!("{:?}", sealed.types()), expected_image);
        let rebuilt = compile(sealed, entry(&rebuilt)).unwrap();
        assert_eq!(
            format!("{:?}", rebuilt.bytecode),
            format!("{:?}", artifact.bytecode)
        );
        assert_eq!(
            format!("{:?}", rebuilt.native_links),
            format!("{:?}", artifact.native_links)
        );
        assert_eq!(format!("{:?}", rebuilt.graph), format!("{:?}", artifact.graph));
        assert_eq!(rebuilt.result_type, artifact.result_type);
    }
}

#[test]
fn seal_rejection_preserves_the_diagnostic_graph() {
    let mut mir = graph("export def answer = missing;", "");
    let before = mir.dump();
    assert!(mir.seal().is_err());
    assert_eq!(mir.dump(), before);
    // Flags alone are not evidence that required slots were normalized.
    mir.diagnostics.clear();
    mir.type_unknowns.clear();
    mir.type_conflicts.clear();
    assert!(mir.seal().is_err());
}
#[test]
fn executes_solved_mir_with_closures_captures_and_branches() {
    let mir = graph(
        "export def answer = (fn(x) { let twice = fn(y) { x + y }; if x > 0 { twice(x) } else { 0 } })(21);",
        "",
    );
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let artifact = mir
        .seal()
        .and_then(|sealed| compile(sealed, entry(&mir)))
        .unwrap();
    let result_type = artifact.result_type;
    let result = execute(artifact).unwrap();
    assert_eq!(result.value().as_int(), Some(42));
    assert_eq!(
        mir.types[result_type.index()].constructor,
        TypeConstructor::Int
    );
}

#[test]
fn executes_imported_generic_definitions_using_resolved_ids() {
    let mir = graph(
        "import \"./math\" { identity as choose }; export def answer = choose(42);",
        "export def identity: for(T) Fn(T) -> T = fn(value) { value };",
    );
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let before = mir.dump();
    let artifact = mir
        .seal()
        .and_then(|sealed| compile(sealed, entry(&mir)))
        .unwrap();
    assert_eq!(mir.dump(), before);
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
}

#[test]
fn codegen_rejects_invalid_mir_and_leaves_runtime_failures_to_vm() {
    for source in [
        "export def answer = missing;",
        "export def answer: Int = \"wrong\";",
    ] {
        let mir = graph(source, "");
        assert!(
            mir.seal()
                .and_then(|sealed| compile(sealed, entry(&mir)))
                .is_err()
        );
    }
    let mir = graph("export def answer = 1 / 0;", "");
    let artifact = mir
        .seal()
        .and_then(|sealed| compile(sealed, entry(&mir)))
        .unwrap();
    let error = execute(artifact).err().expect("division must fail");
    assert!(error.contains("division by zero"), "{error}");
}

#[test]
fn links_native_higher_order_calls_after_codegen_without_recompiling() {
    let mir = graph(
        r#"
        import "std/array" { map as transform, fold };
        export def answer = fold(transform([1, 2, 3], fn(x) { x * 7 }), 0, fn(a, b) { a + b });
    "#,
        "",
    );
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let artifact = mir
        .seal()
        .and_then(|sealed| compile(sealed, entry(&mir)))
        .unwrap();
    assert_eq!(artifact.native_links.len(), 2);
    assert!(artifact.native_links.iter().all(|l| l.module == Some(5)));
    let bytecode = crate::execution_link::link_builtins(&artifact).unwrap();
    assert!(bytecode.shares_code_with(&artifact.bytecode));
    let types_storage = artifact.types.types.as_ptr();
    let definitions_storage = artifact.types.definitions.as_ptr();
    let result_type = artifact.result_type;
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    drop(mir);
    let result = crate::Vm::new()
        .execute_linked(
            linked,
            crate::Quota::with_fuel(10000),
            crate::DataLimits::default(),
            &mut crate::SourceDatabase::default(),
        )
        .unwrap();
    assert_eq!(result.value().as_int(), Some(42));
    assert_eq!(result.result_type(), result_type);
    assert_eq!(result.types().types.as_ptr(), types_storage);
    assert_eq!(result.types().definitions.as_ptr(), definitions_storage);
    assert_eq!(
        result.types().types[result_type.index()].constructor,
        TypeConstructor::Int
    );
}

#[test]
fn native_instances_receive_their_solved_signature_without_inspecting_arguments() {
    let mir = graph(r#"
        native signature: for(T) Fn(T) -> Type;
        def forward: for(U) Fn(U) -> Type = fn(value) { signature(value) };
        export def answer = (forward(42), forward("ok"));
    "#, "");
    let mut artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    artifact.bytecode = crate::execution_link::link_with(&artifact, |_| {
        Some(crate::NativeFunction::new("test.signature", 1, |ctx| {
            // Read only compiler-provided metadata; deliberately never
            // inspect the user argument to determine its type.
            assert!(ctx.solved_signature()?.is_some());
            ctx.copy(ctx.result(), ctx.upvalue(0)?)
        }))
    }).unwrap();
    artifact.native_links.clear();
    drop(mir);
    let result = execute(artifact).unwrap();
    for (index, expected) in [TypeConstructor::Int, TypeConstructor::String].into_iter().enumerate() {
        let id = result.value().sequence_get(index).unwrap().represented_type_id().unwrap();
        let signature = &result.types().types[id.index()];
        assert_eq!(signature.constructor, TypeConstructor::Function);
        assert_eq!(result.types().types[signature.arguments[0].index()].constructor, expected);
    }
}

#[test]
fn tuple_literals_complete_candidates_with_static_element_targets() {
    let definitions = r#"
        import "std/_rt" as rt;
        @check(fn(value) { if value.x > 0 { Ok(()) } else { Err(blame!("positive tuple item", value.x)) } }) type Point = struct {x: Int};
        def candidate: Unchecked(Point) = {x: 42};
    "#;
    for body in [
        "export def answer = do { let pair: (Point, Int) = (candidate, 0); pair.0.x + pair.1 };",
        "def accept: Fn((Point, Int)) -> Int = fn(pair) { pair.0.x }; export def answer = accept((candidate, 0));",
        "def pair: Fn(Unchecked(Point)) -> (Point, Int) = fn(value) { (value, 0) }; export def answer = pair(candidate).0.x;",
        "export def answer = do { let nested: ((Point, Int), String) = ((candidate, 0), \"ok\"); nested.0.0.x };",
        r#"export def answer = match rt.with_diagnostics(fn(x: Int) { let value: Unchecked(Point) = {x: x}; let pair: (Int, Point) = (0, value); pair })(0) { Err(errors) => if errors[0].message == "positive tuple item" { 42 } else { 0 }, _ => 0 };"#,
    ] {
        let mir = graph(&format!("{definitions}{body}"), "");
        assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{body}");
    }
}

#[test]
fn branch_completion_keeps_unselected_candidates_unchecked() {
    let definitions = r#"
        import "std/_rt" as rt;
        @check(fn(value) { if value.x > 0 { Ok(()) } else { Err(blame!("positive branch", value.x)) } }) type Point = struct {x: Int};
        def good: Point = {x: 42};
        def bad: Unchecked(Point) = {x: 0};
    "#;
    for body in [
        "export def answer = (if True { good } else { bad }).x;",
        "export def answer = (if False { bad } else { good }).x;",
        "export def answer = (match True { True => good, False => bad }).x;",
        "export def answer = (match False { True => bad, False => good }).x;",
        r#"export def answer = match rt.with_diagnostics(fn(flag: Bool) { if flag { bad } else { good } })(True) { Err(errors) => if errors[0].message == "positive branch" { 42 } else { 0 }, _ => 0 };"#,
        r#"export def answer = match rt.with_diagnostics(fn(flag: Bool) { match flag { True => bad, False => good } })(True) { Err(errors) => if errors[0].message == "positive branch" { 42 } else { 0 }, _ => 0 };"#,
        "export def answer = do { let value: Point = if True { good } else { bad }; value.x + bad.x };",
        r#"@check(fn(value) { if value.valid { Ok(()) } else { Err(blame!("invalid generic branch", value.item)) } }) type Box(T) = struct {item: T, valid: Bool};
            def choose: for(T) Fn(Bool, Unchecked(Box(T)), Box(T)) -> Box(T) = fn(flag, candidate, good) { if flag { candidate } else { good } };
            export def answer = match rt.with_diagnostics(fn(flag: Bool) { let candidate: Unchecked(Box(Int)) = {item: 0, valid: False}; let good: Box(Int) = {item: 42, valid: True}; choose(flag, candidate, good) })(True) { Err(errors) => if errors[0].message == "invalid generic branch" { 42 } else { 0 }, _ => 0 };"#,
    ] {
        let mir = graph(&format!("{definitions}{body}"), "");
        assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{body}");
    }
}

#[test]
fn unchecked_metadata_observes_shared_skeleton_without_checks() {
    for body in [
        r#"def body = match td.resolve(Unchecked(Box(Int)).type) { Ok(value) => value, _ => fail!("resolve") };
            def checked_body = match td.resolve(Box(Int).type) { Ok(value) => value, _ => fail!("resolve") };
            export def answer = if td.kind(Unchecked(Box(Int)).type) == td.TypeDescKind.Ref && body == checked_body && td.kind(body) == td.TypeDescKind.Struct && td.fields(body)[1].ty == Int.type { 42 } else { 0 };"#,
        r#"export def answer = if td.fields(Unchecked(Box(String)).type)[1].ty == String.type && td.fields(Unchecked(Box(Int)).type)[0].ty == Array(Box(Int)).type { 42 } else { 0 };"#,
        r#"export def answer = do { let candidate: Unchecked(Box(Int)) = {value: 42, children: []}; let packed = dyn.pack(Unchecked(Box(Int)).type, candidate); match dyn.project_with(Int.type, dyn.get_field_value(packed, 1)) { Some(value) => value, _ => 0 } };"#,
    ] {
        let mir = graph(&format!(r#"
            import "std/type-desc" as td; import "std/dyn" as dyn;
            @check(fn(value) {{ fail!("metadata must not complete a candidate") }})
            type Box(T) = struct {{value: T, children: Array(Box(T))}};
            {body}
        "#), "");
        assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{body}");
    }
}

#[test]
fn unchecked_values_complete_at_explicit_mir_boundaries() {
    let definitions = r#"
        import "std/dyn" as dyn; import "std/_rt" as rt;
        @check(fn(value) { if value.x > 0 { Ok(()) } else { Err(blame!("positive x", value.x)) } }) type Point = struct {x: Int};
        type Box(T) = struct {value: T};
    "#;
    for body in [
        r#"export def answer = do { let candidate: Unchecked(Point) = {x: 0}; candidate.x + 42 };"#,
        r#"def read: for(T) Fn(Box(T)) -> T = fn(value) { value.value }; export def answer = do { let candidate: Unchecked(Box(Int)) = {value: 42}; read(candidate) };"#,
        r#"export def answer = do { let candidate: Unchecked(Point) = {x: 42}; let checked: Point = candidate; checked.x };"#,
        r#"def accept: Fn(Point) -> Int = fn(point) { point.x }; export def answer = do { let candidate: Unchecked(Point) = {x: 42}; accept(candidate) };"#,
        r#"type Container = struct {point: Point}; export def answer = do { let candidate: Unchecked(Point) = {x: 42}; let value: Container = {point: candidate}; value.point.x };"#,
        r#"export def answer = match rt.with_diagnostics(fn(x: Int) { let candidate: Unchecked(Point) = {x: x}; let values: Array(Point) = [candidate]; values })(0) { Err(errors) => if errors[0].message == "positive x" { 42 } else { 0 }, _ => 0 };"#,
        r#"export def answer = do { let candidate: Unchecked(Unchecked(Point)) = {x: 42}; if Unchecked(Unchecked(Point)).type == Unchecked(Point).type { candidate.x } else { 0 } };"#,
        r#"export def answer = do { let candidate: Unchecked(Point) = {x: 42}; let packed = dyn.pack(Unchecked(Point).type, candidate); match dyn.project_with(Point.type, packed) { None => 42, _ => 0 } };"#,
        r#"def finish: for(T) Fn(Unchecked(Box(T))) -> Box(T) = fn(candidate) { candidate }; export def answer = do { let candidate: Unchecked(Box(Int)) = {value: 42}; finish(candidate).value };"#,
        r#"def finish: Fn(Unchecked(Point)) -> Point = fn(candidate) { candidate };
            export def answer = match rt.with_diagnostics(fn(x: Int) { let candidate: Unchecked(Point) = {x: x}; finish(candidate) })(0) { Err(errors) => if errors[0].message == "positive x" { 42 } else { 0 }, _ => 0 };"#,
        r#"@check(fn(value) { if value.valid { Ok(()) } else { Err(blame!("invalid box", value.value)) } }) type CheckedBox(T) = struct {value: T, valid: Bool};
            def finish: for(T) Fn(Unchecked(CheckedBox(T))) -> CheckedBox(T) = fn(candidate) { candidate };
            export def answer = match rt.with_diagnostics(fn(x: Int) { let candidate: Unchecked(CheckedBox(Int)) = {value: x, valid: False}; finish(candidate) })(0) { Err(errors) => if errors[0].message == "invalid box" { 42 } else { 0 }, _ => 0 };"#,
        r#"export def answer = match rt.with_diagnostics(fn(x: Int) { let candidate: Unchecked(Point) = {x: x}; let checked: Point = candidate; checked })(0) { Err(errors) => if errors[0].message == "positive x" { 42 } else { 0 }, _ => 0 };"#,
    ] {
        let mir = graph(&format!("{definitions}{body}"), "");
        assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{body}");
    }
}


#[test]
fn generic_construction_checks_consume_static_body_instances() {
    let definitions = r#"
        import "std/json" as json;
        import "std/_rt" as rt;
        def identity: for(T) Fn(T) -> T = fn(value) { value };
        @check(fn(value) { let copied = identity(value.item); if value.valid { Ok(()) } else { Err(blame!("invalid item", copied)) } })
        type Item(T) = struct { item: T, valid: Bool };
        @check(fn(value) { let copied = identity(value); Ok(()) }) type Wrapped(T) = struct(T);
        type Choice(T) = enum { @check(fn(value) { let copied = identity(value); Ok(()) }) Some(T), Empty };
        type Envelope(T) = struct { child: Item(T) };
    "#;
    for body in [
        r#"export def answer = do { let a: Item(Int) = { item: 40, valid: True }; let b: Item(String) = { item: "ok", valid: True }; a.item + 2 };"#,
        r#"def make: for(T) Fn(T) -> Item(T) = fn(value) { { item: value, valid: True } }; export def answer = make(42).item;"#,
        r#"export def answer = match rt.with_diagnostics(fn(value: Int) { let item: Item(Int) = { item: value, valid: False }; item })(0) { Err(errors) => if errors[0].message == "invalid item" { 42 } else { 0 }, _ => 0 };"#,
        r#"type IntWrapped = Wrapped(Int); export def answer = match IntWrapped(42) { IntWrapped(value) => value };"#,
        r#"type IntChoice = Choice(Int); export def answer = match IntChoice.Some(42) { IntChoice.Some(value) => value, _ => 0 };"#,
        r#"export def answer = match json.decode(Envelope(Int).type, "{\"child\":{\"item\":42,\"valid\":true}}") { Ok(value) => value.child.item, _ => 0 };"#,
        r#"export def answer = match json.decode(Envelope(String).type, "{\"child\":{\"item\":\"bad\",\"valid\":false}}") { Err(_) => 42, _ => 0 };"#,
    ] {
        let mir = graph(&format!("{definitions}{body}"), "");
        assert!(mir.diagnostics.is_empty(), "{body}\n{}", mir.dump());
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{body}");
    }
}

#[test]
fn solved_codec_construction_checks_reject_values_and_preserve_trial_semantics() {
    let definitions = r#"
        import "std/json" as json; import "std/string" as string; import "std/regex" as regex;
        import "std/_rt" as rt;
        @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive count", value)) } }) type Count = struct(Int);
        @string.decode_by_parse @string.encode_by_display
        @regex.parse_by(regex.compile(r"^(?P<value>\d+)$"))
        @check(fn(item) { if item.value > 0 { Ok(()) } else { Err(blame!("positive value", item.value)) } }) type Item = struct {value: Int};
        @json.untagged type Choice = enum { @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive variant", value)) } }) Checked(Int), Plain(Int) };
        @json.untagged type TextChoice = enum { Parsed(Item), Plain(String) };
    "#;
    for body in [
        r#"export def answer = match json.decode(Count.type, "0") { Err(_) => 42, _ => 0 };"#,
        r#"export def answer = match json.decode(Count.type, "42") { Ok(Count(value)) => value, _ => 0 };"#,
        r#"export def answer = match json.decode(Choice.type, "0") { Ok(Choice.Plain(_)) => 42, _ => 0 };"#,
        r#"export def answer = match json.decode(Choice.type, "1") { Err(_) => 42, _ => 0 };"#,
        r#"export def answer = match json.decode(Item.type, "\"0\"") { Err(_) => 42, _ => 0 };"#,
        r#"export def answer = match json.decode(TextChoice.type, "\"0\"") { Ok(TextChoice.Plain(_)) => 42, _ => 0 };"#,
        r#"export def answer = match rt.with_diagnostics(fn(text: String) { string.parse(Item.type, text) })("0") { Err(errors) => if errors[0].message == "positive value" { 42 } else { 0 }, _ => 0 };"#,
        r#"@check(fn(value) { fail!("checker execution failed") }) type Broken = struct(Int);
            @json.untagged type BrokenChoice = enum { Plain(Int), Broken(Broken) };
            export def answer = match rt.with_diagnostics(fn(text: String) { json.decode(BrokenChoice.type, text) })("1") { Err(errors) => if errors[0].message == "checker execution failed" { 42 } else { 0 }, _ => 0 };"#,
        r#"@check(fn(value) { if value.number > 0 { Ok(()) } else { Err(blame!("positive record", value.number)) } }) type Record = struct {number: Int};
            export def answer = match json.decode(Record.type, "{\"number\":0}") { Err(_) => 42, _ => 0 };"#,
    ] {
        let mir = graph(&format!("{definitions}{body}"), "");
        assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{body}");
    }
}

#[test]
fn construction_checks_execute_at_solved_constructor_boundaries() {
    for source in [
        r#"def minimum = 1; def validate = fn(value) { if value >= minimum { Ok(()) } else { Err(blame!("minimum", value)) } };
            @check(validate) type Item = struct(Int); export def answer = match Item(42) { Item(value) => value };"#,
        r#"@check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive", value)) } }) type Item = struct(Int);
            export def answer = match Item(42) { Item(value) => value };"#,
        r#"@check(fn(value) { if value.number > 0 { Ok(()) } else { Err(blame!("positive", value.number)) } }) type Item = struct {number: Int};
            def value: Item = {number: 42}; export def answer = value.number;"#,
        r#"type Item = enum { @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive", value)) } }) Full(Int), Empty };
            export def answer = match Item.Full(42) { Item.Full(value) => value, _ => 0 };"#,
        r#"import "std/_rt" as rt; import "std/array" as array;
            @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive", value)) } }) type Item = struct(Int);
            export def answer = match rt.with_diagnostics(fn(n: Int) { Item(n) })(0) { Err(errors) => if array.length(errors) == 1 && errors[0].message == "positive" { 42 } else { 0 }, _ => 0 };"#,
        r#"import "std/_rt" as rt; import "std/array" as array;
            @check(fn(value) { if value.number > 0 { Ok(()) } else { Err(blame!("positive", value.number)) } }) type Item = struct {number: Int};
            def attempt = rt.with_diagnostics(fn(n: Int) { let value: Item = {number: n}; value });
            export def answer = match (attempt(0), attempt(0), attempt(42)) { (Err(first), Err(second), Ok((value, _))) => if array.length(first) == 1 && array.length(second) == 1 { value.number } else { 0 }, _ => 0 };"#,
        r#"import "std/_rt" as rt; type Item = enum { @check(fn(value) { Err(blame!("rejected", value)) }) Full(Int), Empty };
            export def answer = match rt.with_diagnostics(fn(n: Int) { Item.Full(n) })(1) { Err(_) => 42, _ => 0 };"#,
    ] {
        let mir = graph(source, "");
        assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn solved_codec_text_decode_roundtrips_and_composes_with_untagged_trials() {
    let definitions = r#"
        import "std/codec" as codec; import "std/json" as json;
        import "std/regex" as regex; import "std/string" as string; import "std/fmt" as fmt;
        @string.decode_by_parse @string.encode_by_display @fmt.display_by("{host}:{port}")
        @regex.parse_by(regex.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
        type Endpoint = struct { host: String, port: Int };
        @string.decode_by_parse @string.encode_by_display @fmt.display_by("{name}@{endpoint}")
        @regex.parse_by(regex.compile(r"^(?P<name>\w+)@(?P<endpoint>.+)$"))
        type Service = struct { name: String, endpoint: Endpoint };
        @json.untagged type Choice = enum { Parsed(Endpoint), Text(String) };
    "#;
    for body in [
        r#"def value = json.decode(Service.type, "\"api@local:42\"").unwrap!(); export def answer = if codec.encode(codec.Value.type, value) == codec.Value.String("api@local:42") { value.endpoint.port } else { 0 };"#,
        r#"export def answer = match json.decode(Choice.type, "\"not-an-endpoint\"") { Ok(Choice.Text(value)) => if value == "not-an-endpoint" { 42 } else { 0 }, _ => 0 };"#,
        r#"export def answer = match json.decode(Choice.type, "\"local:42\"") { Err(_) => 42, _ => 0 };"#,
        r#"export def answer = match json.decode(Endpoint.type, "42") { Err(_) => 42, _ => 0 };"#,
    ] {
        let mir = graph(&format!("{definitions}{body}"), "");
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{body}");
    }
}

#[test]
fn solved_regex_prepare_checks_capture_contracts_without_type_reconstruction() {
    for (field, pattern, expected) in [
        ("value: Int", "^(?P<other>.*)$", "captures must match struct fields"),
        ("value: Option(Int)", "^(?P<value>.*)$", "capture \"value\" is required"),
        ("value: Int", "^(?P<value>.*)?$", "capture \"value\" is optional"),
        ("value: Array(Int)", "^(?P<value>.*)$", "not string-parsable"),
    ] {
        let mir = graph(&format!(r#"import "std/string" as string; import "std/regex" as regex;
            @regex.parse_by(regex.compile(r"{pattern}")) type Item = struct {{ {field} }};
            export def answer = string.parse(Item.type, "42");"#), "");
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let error = execute(artifact).err().expect("invalid capture contract");
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn solved_string_parse_uses_capture_ranges_and_lazy_nested_properties() {
    for source in [
        r#"import "std/string" as string; export def answer = match string.parse(Int.type, "42") { Ok(value) => value, Err(_) => 0 };"#,
        r#"import "std/string" as string; import "std/regex" as regex;
            @regex.parse_by(regex.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
            type Endpoint = struct { host: String, port: Int };
            @regex.parse_by(regex.compile(r"^(?P<name>\w+)@(?P<endpoint>.+)$"))
            type Service = struct { name: String, endpoint: Endpoint };
            export def answer = match string.parse(Service.type, "api@local:42") { Ok(value) => if value.name == "api" && value.endpoint.host == "local" { value.endpoint.port } else { 0 }, Err(_) => 0 };"#,
        r#"import "std/string" as string; import "std/regex" as regex;
            @regex.parse_by(regex.compile(r"^(?P<value>\d+)(?:/(?P<note>\w+))?$"))
            type Item = struct { value: Int, note: Option(String) };
            export def answer = match string.parse(Item.type, "42") { Ok(value) => if value.note == None { value.value } else { 0 }, Err(_) => 0 };"#,
        r#"import "std/string" as string; export def answer = match string.parse(Int.type, "bad") { Err(error) => if error.value == "bad" { 42 } else { 0 }, _ => 0 };"#,
        r#"import "std/string" as string; export def answer = match string.parse(Float.type, "NaN") { Err(_) => 42, _ => 0 };"#,
        r#"import "std/string" as string; export def answer = match string.parse(Float.type, "inf") { Err(_) => 42, _ => 0 };"#,
        r#"import "std/string" as string; export def answer = match string.parse(Float.type, "-inf") { Err(_) => 42, _ => 0 };"#,
        r#"import "std/string" as string; export def answer = match string.parse(Float.type, "1e9999") { Err(_) => 42, _ => 0 };"#,
        r#"import "std/string" as string; export def answer = match string.parse(Float.type, "1.5") { Ok(value) => if value == 1.5 { 42 } else { 0 }, Err(_) => 0 };"#,
    ] {
        let mir = graph(source, "");
        assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn solved_codec_text_encode_calls_prepared_display_for_nested_values() {
    let mir = graph(r#"
        import "std/codec" as codec; import "std/json" as json;
        import "std/string" as string; import "std/fmt" as fmt;
        @string.decode_by_parse @string.encode_by_display
        @fmt.display_by("{host}:{port}")
        type Endpoint = struct { host: String, port: Int };
        @string.decode_by_parse @string.encode_by_display
        @fmt.display_by("{name}@{endpoint}")
        type Service = struct { name: String, endpoint: Endpoint };
        def endpoint: Endpoint = {host: "localhost", port: 8080};
        def service: Service = {name: "api", endpoint};
        type Payload = struct { endpoints: Array(Endpoint), service: Service };
        def payload: Payload = { endpoints: [endpoint, endpoint], service };
        export def answer = json.stringify(codec.encode(codec.Value.type, payload));
    "#, "");
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    drop(mir);
    let result = execute(artifact).unwrap();
    assert_eq!(result.value().as_str().unwrap().as_str(), r#"{"endpoints":["localhost:8080","localhost:8080"],"service":"api@localhost:8080"}"#);
}

#[test]
fn solved_codec_text_encode_rejects_incomplete_bridge_contracts() {
    for (decorators, expected) in [
        ("@string.encode_by_display", "must be used together"),
        ("@string.decode_by_parse", "must be used together"),
        ("@string.decode_by_parse @string.encode_by_display", "requires a DisplayBy"),
    ] {
        let mir = graph(&format!(r#"import "std/codec" as codec; import "std/string" as string;
            {decorators} type Item = struct {{ value: Int }};
            def item: Item = {{value: 42}}; export def answer = codec.encode(codec.Value.type, item);"#), "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let error = execute(artifact).err().expect("invalid text bridge");
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn solved_codec_encode_consumes_layouts_and_lazy_untagged_properties() {
    let mir = graph(r#"
        import "std/codec" as codec;
        import "std/json" as json;
        import "std/value" {ScalarValue};
        type Box(T) = struct { value: T };
        def boxed: Box(Int) = { value: 42 };
        type Payload = struct { boxed: Box(Int), bindings: Array(ScalarValue), sql: String, flags: Array(Bool) };
        def payload: Payload = {
            boxed, bindings: [ScalarValue.Int(42), ScalarValue.String("ok"), ScalarValue.None],
            sql: "SELECT 1", flags: [True, False],
        };
        export def answer = json.stringify(codec.encode(codec.Value.type, payload));
    "#, "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    drop(mir);
    let result = execute(artifact).unwrap();
    assert_eq!(result.value().as_str().unwrap().as_str(),
        r#"{"bindings":[42,"ok",null],"boxed":{"value":42},"flags":[true,false],"sql":"SELECT 1"}"#);
}

#[test]
fn solved_codec_encode_uses_rename_options_and_recursive_layouts() {
    let mir = graph(r#"
        import "std/codec" as codec;
        import "std/json" as json;
        type Tree = enum { Leaf(Int), Branch(Array(Tree)), Empty };
        @json.rename_all(json.RenameCase.CamelCase)
        type Model = struct { some_value: Option(Int), tree: Tree };
        def model: Model = { some_value: Some(42), tree: Tree.Branch([Tree.Leaf(1), Tree.Empty]) };
        export def answer = json.stringify(codec.encode(codec.Value.type, model));
    "#, "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let result = execute(artifact).unwrap();
    assert_eq!(result.value().as_str().unwrap().as_str(), r#"{"someValue":42,"tree":{"Branch":[{"Leaf":1},"Empty"]}}"#);
}

#[test]
fn solved_codec_decode_untagged_requires_one_match_and_preserves_nested_work() {
    for source in [
        r#"import "std/json" as json;
            @json.untagged type Inner = enum { Text(String), Flag(Bool) };
            @json.untagged type Outer = enum { Inner(Inner), Number(Int) };
            export def answer = match json.decode(Outer.type, "42").unwrap!() { Outer.Number(n) => n, _ => 0 };"#,
        r#"import "std/json" as json;
            @json.untagged type Item = enum { Empty, Missing };
            export def answer = match json.decode(Item.type, "null") { Err(_) => 42, _ => 0 };"#,
        r#"import "std/json" as json;
            @json.untagged type Item = enum { Text(String), Number(Int), Empty };
            export def answer = match json.decode(Item.type, "42").unwrap!() { Item.Number(n) => n, _ => 0 };"#,
        r#"import "std/json" as json;
            @json.untagged type Item = enum { Text(String), Number(Int), Empty };
            export def answer = match json.decode(Item.type, "null").unwrap!() { Item.Empty => 42, _ => 0 };"#,
        r#"import "std/json" as json;
            @json.untagged type Item = enum { First(Int), Second(Int) };
            export def answer = match json.decode(Item.type, "42") { Err(_) => 42, _ => 0 };"#,
        r#"import "std/json" as json;
            @json.untagged type Item = enum { Text(String), Number(Int) };
            export def answer = match json.decode(Item.type, "true") { Err(_) => 42, _ => 0 };"#,
        r#"import "std/json" as json;
            @json.rename_all(json.RenameCase.CamelCase) type Named = struct { some_value: Int };
            @json.untagged type Item = enum { Wrong((Int, String)), Pair((Int, Int)), Named(Named) };
            def items = json.decode(Array(Item).type, "[[1,2],{\"someValue\":39}]").unwrap!();
            export def answer = match items[0] { Item.Pair(pair) => match items[1] { Item.Named(named) => pair.0 + pair.1 + named.some_value, _ => 0 }, _ => 0 };"#,
    ] {
        let mir = graph(source, "");
        assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn solved_codec_decode_uses_generic_recursive_layouts_and_reports_mismatches() {
    for source in [
        r#"import "std/json" as json;
            @json.rename_all(json.RenameCase.CamelCase)
            type Choice = enum { SomeValue(Int), NoValue };
            export def answer = match json.decode(Choice.type, "{\"someValue\":42}").unwrap!() { Choice.SomeValue(value) => value, _ => 0 };"#,
        r#"import "std/json" as json;
            @json.rename_all(json.RenameCase.CamelCase)
            type Model = struct { some_value: Int, optional_note: Option(String) };
            export def answer = json.decode(Model.type, "{\"someValue\":42}").unwrap!().some_value;"#,
        r#"import "std/json" as json;
            type Box(T) = struct { value: T, note: Option(String) };
            type Tree = enum { Leaf(Box(Int)), Branch(Array(Tree)), Empty };
            def decoded = json.decode(Tree.type, "{\"Branch\":[{\"Leaf\":{\"value\":42}},\"Empty\"]}").unwrap!();
            export def answer = match decoded { Tree.Branch(items) => match items[0] { Tree.Leaf(boxed) => if boxed.note == None { boxed.value } else { 0 }, _ => 0 }, _ => 0 };"#,
        r#"import "std/json" as json; import "std/_rt" as rt; import "std/array" as array;
            type Box = struct { value: Int };
            export def answer = match rt.with_diagnostics(fn(text: String) { json.decode(Box.type, text).unwrap!() })("{\"value\":\"bad\"}") {
                Err(errors) => if array.length(errors) == 1 { 42 } else { 0 }, _ => 0
            };"#,
        r#"import "std/json" as json; type Box = struct { value: Int };
            export def answer = match json.decode(Box.type, "{\"value\":42,\"extra\":0}") { Err(_) => 42, _ => 0 };"#,
    ] {
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn native_link_requires_an_admitted_binding_with_the_declared_arity() {
    let mir = graph(
        "native map: Fn(Int) -> Int; export def answer = map(1);",
        "",
    );
    let artifact = mir
        .seal()
        .and_then(|sealed| compile(sealed, entry(&mir)))
        .unwrap();
    assert!(crate::execution_link::link_builtins(&artifact).is_err());
    let errors = crate::execution_link::link_with(&artifact, |_| {
        Some(crate::NativeFunction::new("wrong", 2, |_| unreachable!()))
    })
    .unwrap_err();
    assert!(errors.iter().any(|d| d.message.contains("arity")));
}

#[test]
fn records_and_nominal_configs_use_existing_vm_storage_and_field_operations() {
    for main in [
        "type Config = struct {evaluate: Fn(Int) -> Int, seed: Int}; def config: Config = { evaluate: fn(x) { if True { x + 1 } else { 0 } }, seed: 41 }; export def answer = config.evaluate(config.seed);",
        "import \"./math\" { Config }; def config: Config = { evaluate: fn(x) { x + 1 }, seed: 41 }; export def answer = config.evaluate(config.seed);",
    ] {
        let mir = graph(
            main,
            "export type Config = struct { seed: Int, evaluate: Fn(Int) -> Int };",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let before = mir.dump();
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        assert_eq!(mir.dump(), before);
        let linked = crate::execution_link::link_entry(artifact).unwrap();
        let result = crate::Vm::new()
            .execute_linked(
                linked,
                crate::Quota::with_fuel(10000),
                crate::DataLimits::default(),
                &mut crate::SourceDatabase::default(),
            )
            .unwrap();
        assert_eq!(result.value().as_int(), Some(42));
    }
}

#[test]
fn executes_the_standard_entry_main_wrapper_without_the_old_compiler() {
    let mir = graph(
        r#"
        import "std/entry" { main };
        import "std/value" { Value };
        import "std/array" { length };
        def evaluator = main({ sources: [], envs: [], args: True }, fn(ctx) {
            Value.Int(length(ctx.args) * 21)
        });
        export def answer = evaluator.evaluate({ sources: {}, env: {}, args: ["one", "two"] });
    "#,
        "",
    );
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let result_type = artifact.result_type;
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    let result = crate::Vm::new()
        .execute_linked(
            linked,
            crate::Quota::with_fuel(10000),
            crate::DataLimits::default(),
            &mut crate::SourceDatabase::default(),
        )
        .unwrap();
    assert_eq!(result.to_json(result_type).unwrap(), "42");
}

#[test]
fn executes_nominal_variants_with_solved_identity_and_first_class_constructors() {
    let mir = graph(
        "import \"./math\" { Choice as C }; \
         export def answer = (C.Missing, (fn(make) { make(42) })(C.Number));",
        "export type Choice = enum { Missing, Number(Int) };",
    );
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let expected = artifact.types.types[artifact.result_type.index()].arguments[0];
    // Bytecode cannot reconstruct missing static type data in an empty VM.
    assert!(crate::Vm::new().execute(&artifact.bytecode, 10000).is_err());
    let linked = crate::execution_link::link_entry(artifact).unwrap();
    drop(mir);
    let result = crate::Vm::new()
        .execute_linked(
            linked,
            crate::Quota::with_fuel(10000),
            crate::DataLimits::default(),
            &mut crate::SourceDatabase::default(),
        )
        .unwrap();
    let missing = result.value().sequence_get(0).unwrap();
    let number = result.value().sequence_get(1).unwrap();
    assert_eq!(missing.solved_type_id(), Some(expected));
    assert_eq!(number.solved_type_id(), Some(expected));
    assert_eq!(number.tagged_parts().unwrap().1.as_int(), Some(42));
    assert_eq!(result.types().variant(expected, 0).unwrap().name, "Missing");
    assert_eq!(result.types().variant(expected, 1).unwrap().name, "Number");
}

#[test]
fn type_image_retains_recursive_and_generic_skeletons_without_mir() {
    let mir = graph(
        "type Pair(T) = struct { first: T, second: T }; \
         type Tree = enum { Leaf(Int), Branch((Tree, Tree)) }; \
         export def answer = 42;",
        "",
    );
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let before = mir.dump();
    let artifact = mir
        .seal()
        .and_then(|sealed| compile(sealed, entry(&mir)))
        .unwrap();
    assert_eq!(mir.dump(), before);
    assert_eq!(artifact.types.types.len(), mir.types.len());
    drop(mir);
    let image = &artifact.types;
    let pair = image
        .definitions
        .iter()
        .find(|d| d.name.ends_with("::Pair"))
        .unwrap();
    let parameter = pair.parameters[0];
    for member in &pair.members {
        assert_eq!(
            image.types[member.payload.unwrap().index()].constructor,
            TypeConstructor::Parameter(parameter)
        );
    }
    let tree = image
        .definitions
        .iter()
        .find(|d| d.name.ends_with("::Tree"))
        .unwrap();
    assert!(std::ptr::eq(image.definition(tree.symbol).unwrap(), tree));
    let branch = tree.members.iter().find(|m| m.name == "Branch").unwrap();
    let tuple = &image.types[branch.payload.unwrap().index()];
    assert_eq!(tuple.constructor, TypeConstructor::Tuple);
    assert_eq!(tuple.arguments.len(), 2);
    for &child in &tuple.arguments {
        assert_eq!(
            image.types[child.index()].constructor,
            TypeConstructor::Nominal(tree.symbol)
        );
    }
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
}
