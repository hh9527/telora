    #[test]
    fn recursive_values_cross_builtin_boundaries_without_losing_sealed_types() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
               import "std/codec" as codec;
               import "std/dict" as dict;
               import "std/dyn" as dyn;
               import "std/fmt" as fmt;
               import "std/json" as json;
               import "std/option" as option;
               import "std/result" as result;

               @fmt.display_by("{value}")
               type Node = struct {value: Int, children: Array(Node)};
               type NodeResult = Result(Node, String);
               def identity: Fn(Node) -> Node = fn(node) { node };
               let leaf: Node = {value: 2, children: []};
               let root: Node = {value: 1, children: [leaf]};
               let nodes: Array(Node) = [root, leaf];
               let mapped: Array(Node) = array.map(nodes, identity);
               let filtered: Array(Node) = array.filter(mapped, fn(node) { node.value > 0 });
               let flattened: Array(Node) = array.flat_map(filtered, fn(node) { [node] });
               let sum: Int = array.fold(flattened, 0, fn(total, node) {
                   total + node.value
               });
               let indexed: Dict(Node) = {root: root, leaf: leaf};
               let mapped_dict: Dict(Node) = dict.map_values(indexed, identity);
               let maybe: Option(Node) = option.map('Some(root), identity);
               let outcome: NodeResult = result.map('Ok(root), identity);
               let packed = dyn.pack(Node, root);
               let decoded: Node = codec.decode(
                   Node,
                   codec.encode(codec.Value, root) |> result.unwrap,
               ) |> result.unwrap;
               export def output = {
                   sum,
                   mapped: mapped[0],
                   dict_root: dict.get(mapped_dict, "root"),
                   maybe,
                   outcome,
                   dyn_kind: dyn.kind(packed),
                   decoded,
                   display: fmt.render(fmt.display(Node, root)),
                   schema: json.schema(Node),
               };"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 1_000_000).unwrap();
        for name in [
            "NodeResult",
            "identity",
            "nodes",
            "mapped",
            "filtered",
            "flattened",
            "indexed",
            "mapped_dict",
            "maybe",
            "outcome",
            "decoded",
        ] {
            let ty = module
                .analysis
                .declared_types
                .get(name)
                .or_else(|| module.analysis.binding_types.get(name))
                .copied()
                .expect("audited binding has a type");
            assert!(!module.analysis.display(ty).contains("Any"), "{name}");
        }
        let output = module.execute(1_000_000).unwrap().to_string();
        assert!(output.contains("sum: 3"), "{output}");
        assert!(output.contains("display: \"1\""), "{output}");
        assert!(output.contains("dyn_kind: 'Dict"), "{output}");
        assert!(output.contains("$defs"), "{output}");
        assert!(output.contains("$ref"), "{output}");

        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{next}")
               type Loop = struct {next: Loop};
               export {Loop};"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 1_000_000).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Display template field has no Display capability"),
            "{error}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_nested_type_families_preserve_recursive_codec_metadata() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"type IntValue = struct {value: Int};
               type StringValue = struct {value: String};
               type Val = enum {'Int(IntValue), 'Str(StringValue)};
               type BinaryNode = struct {left: Expr, right: Expr};
               type ColumnRef = struct {alias: String, column: String};
               type Expr = enum {
                   'Value(Val),
                   'Add(BinaryNode),
                   'Column(ColumnRef),
               };
               type Mapping = struct {predicate: Expr};
               type Relation(M) = struct {mapping: M};
               type RelationUse(Entity) = struct {
                   entity: Entity,
                   relation: Relation(Mapping),
               };
               export {Expr, Val, Mapping, Relation, RelationUse};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./types" as types;
               import "std/codec" as codec;
               import "std/json" as json;
               import "std/result" as result;
               type Entity = enum {'Order};
               type Use = types.RelationUse(Entity);
               let relation: Use = {
                   entity: 'Order,
                   relation: {mapping: {predicate: 'Add({
                       left: 'Value('Int({value: 1})),
                       right: 'Column({alias: "t", column: "id"}),
                   })}},
               };
                {
                    encoded: codec.encode(codec.Value, relation)
                       |> result.unwrap
                       |> json.stringify,
                    schema: json.schema(Use),
                }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("\\\"left\\\""), "{output}");
        assert!(output.contains("\\\"column\\\":\\\"id\\\""), "{output}");
        assert!(output.contains("$defs"), "{output}");
        assert!(output.contains("#/$defs/Type"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cross_module_recursive_enum_rebuild_preserves_declared_codec_witness() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"type Call = struct {args: Array(Expr)};
               type Expr = enum {'Int(Int), 'Call(Call)};
               type Plan = struct {grouping: Array(Expr)};
               export {Expr, Plan};"#,
        )
        .unwrap();
        fs::write(
            directory.join("creator.telora"),
            r#"import "./types" as types;
               import "std/array" as array;
               def normalize_expr: Fn(types.Expr) -> types.Expr = fn(expr) {
                   match expr {
                       'Int(value) => 'Int(value),
                       'Call(call) => 'Call({
                           args: array.map(call.args, normalize_expr),
                       }),
                   }
               };
               def make_plan: Fn(types.Expr) -> types.Plan = fn(expr) {
                   {grouping: [normalize_expr(expr)]}
               };
               export {make_plan};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./types" as types;
               import "./creator" as creator;
               import "std/codec" as codec;
               import "std/result" as result;
               import "std/value" {Value};
               let expr: types.Expr = 'Call({args: [
                   'Call({args: ['Int(1)]}),
                   'Int(2),
               ]});
               let produced: types.Plan = creator.make_plan(expr);
               let direct: types.Plan = {grouping: [expr]};
               let direct_encoded = codec.encode(Value, direct) |> result.unwrap;
               let produced_encoded = codec.encode(Value, produced) |> result.unwrap;
               {
                   direct: codec.decode(types.Plan, direct_encoded) |> result.unwrap,
                   produced: codec.decode(types.Plan, produced_encoded) |> result.unwrap,
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("produced"), "{output}");
        assert!(output.contains("grouping"), "{output}");
        assert!(output.contains("'Int(2)"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exported_codec_boundary_owns_complex_family_witness() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"import "std/codec" as codec;
               type Binary = struct {left: Expr, right: Expr};
               type Expr = enum {'Lit(Int), 'Add(Binary)};
               type Payload(A, B, C, D, E, F, G) = struct {
                   a: A, b: B, c: C, d: D, e: E, f: F, g: G,
               };
               type Rejection = Payload(
                   Int, String, Bool, Float, Expr, Array(Int), Option(String)
               );
               def encode_rejection = fn(value: Rejection) {
                   codec.encode(codec.Value, value)
               };
               export {Expr, Rejection, encode_rejection};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./types" as types;
               import "std/json" as json;
               import "std/result" as result;
               let rejection: types.Rejection = {
                   a: 1,
                   b: "two",
                   c: 'True,
                   d: 4.0,
                   e: 'Add({left: 'Lit(5), right: 'Lit(6)}),
                   f: [7],
                   g: 'Some("eight"),
               };
               types.encode_rejection(rejection)
                   |> result.unwrap
                   |> json.stringify"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output = module.execute(100_000).unwrap().to_string();
        assert!(output.contains("\\\"left\\\""), "{output}");
        assert!(output.contains("\\\"g\\\":\\\"eight\\\""), "{output}");

        fs::write(
            directory.join("invalid.telora"),
            r#"import "./types" as types;
               types.encode_rejection({
                   a: "wrong",
                   b: "two",
                   c: 'True,
                   d: 4.0,
                   e: 'Lit(5),
                   f: [7],
                   g: 'None,
               })"#,
        )
        .unwrap();
        let error = load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000)
            .unwrap_err()
            .to_string();
        assert!(error.contains("String") && error.contains("Int"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recursive_type_metadata_keeps_typed_module_import_surfaces() {
        let directory = fixture_dir();
        fs::write(
            directory.join("expr.telora"),
            r#"type Binary = struct {left: Expr, right: Expr};
               type Expr = enum {'Lit(Int), 'Add(Binary)};
               def lit: Fn(Int) -> Expr = fn(value) { 'Lit(value) };
               def add: Fn(Expr, Expr) -> Expr = fn(left, right) {
                   'Add({left, right})
               };
               def depth: Fn(Expr) -> Int = fn(expr) {
                   match expr {
                       'Lit(_) => 1,
                       'Add({left, right}) => 1 + depth(left) + depth(right),
                   }
               };
               export {Binary, Expr, lit, add, depth};"#,
        )
        .unwrap();

        fs::write(
            directory.join("whole.telora"),
            r#"import "./expr" as expr;
               import "std/array" as array;
               import "std/type-desc" as desc;
               def has_ref = fn(ty, fuel) {
                   if fuel < 1 {
                       'False
                   } else {
                       if desc.kind(ty) == 'Ref {
                           'True
                       } else {
                           array.any(desc.children(ty), fn(child) {
                               has_ref(child, fuel - 1)
                           })
                       }
                   }
               };
               def value: expr.Expr = expr.add(
                   expr.lit(1),
                   expr.add(expr.lit(2), expr.lit(3)),
               );
               export def output = (expr.depth(value), has_ref(expr.Expr, 8));"#,
        )
        .unwrap();
        let whole = load_module(directory.join("whole.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            whole.execute(100_000).unwrap().to_string(),
            "{output: (5, 'True)}"
        );

        for (name, import) in [
            (
                "selective.telora",
                r#"import "./expr" {Expr, lit, add, depth};"#,
            ),
            ("open.telora", r#"import "./expr" *;"#),
        ] {
            fs::write(
                directory.join(name),
                format!(
                    r#"{import}
                       let value: Expr = add(lit(1), lit(2));
                       export def output = depth(value);"#
                ),
            )
            .unwrap();
            let module = load_module(directory.join(name), BTreeMap::new(), 100_000).unwrap();
            assert_eq!(module.execute(100_000).unwrap().to_string(), "{output: 3}");
        }

        fs::write(
            directory.join("invalid.telora"),
            r#"import "./expr" {Expr, depth};
               export def output = depth("bad");"#,
        )
        .unwrap();
        let invalid = load_module(directory.join("invalid.telora"), BTreeMap::new(), 100_000)
            .unwrap_err()
            .to_string();
        assert!(
            invalid.contains("String") && invalid.contains("Expr"),
            "{invalid}"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn final_program_observes_only_presealed_recursive_type_roots() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/result" as result;
               type Forward = struct {next: Later};
               let premature = codec.decode(Forward, codec.encode(codec.Value, {next: 1}) |> result.unwrap);
               type Later = Int;
               premature"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "'Ok({next: 1})"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn builtin_bool_option_and_result_are_normalized_enum_metadata() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/attributes" as attributes;
               type Maybe = Option(attributes.add(Int, { marker: "payload" }));
               type Outcome = Result(String, Int);
               let compared: Bool = 1 < 2;
               let none: Maybe = 'None;
               let some: Maybe = 'Some(42);
               let ok: Outcome = 'Ok("done");
               let err: Outcome = 'Err(7);
               {
                   bool: Bool,
                   maybe: Maybe,
                   outcome: Outcome,
                   compared: validate(Bool, compared),
                   none: validate(Maybe, none),
                   some: validate(Maybe, some),
                   ok: validate(Outcome, ok),
                   err: validate(Outcome, err),
                   wrong_bool: validate(Bool, 'Other),
                   wrong_some: validate(Maybe, 'Some("forty-two")),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let result_world = module.execute(100_000).unwrap();
        let result = result_world.value();
        for field in ["compared", "none", "some", "ok", "err"] {
            assert!(result.get(field).unwrap().to_string().starts_with("'Ok("));
        }
        for field in ["wrong_bool", "wrong_some"] {
            assert!(result.get(field).unwrap().to_string().starts_with("'Err("));
        }

        fn wrapper(value: crate::ValueRef<'_>) -> crate::ValueRef<'_> {
            let wrapper = value;
            assert_eq!(wrapper.get("kind").unwrap().to_string(), "'WithAttributes");
            assert!(wrapper.get("attributes").unwrap().dict_fields().is_some());
            wrapper
        }
        for field in ["bool", "maybe", "outcome"] {
            let root = wrapper(result.get(field).unwrap());
            let metadata = root.get("inner").unwrap();
            assert_eq!(metadata.get("kind").unwrap().to_string(), "'Enum");
            let variants = metadata.get("variants").unwrap();
            for variant in variants.dict_values().unwrap() {
                wrapper(variant);
            }
        }
        let maybe = wrapper(result.get("maybe").unwrap());
        let metadata = maybe.get("inner").unwrap();
        let variants = metadata.get("variants").unwrap();
        let some = wrapper(variants.get("Some").unwrap());
        assert_eq!(
            some.get("attributes").unwrap().to_string(),
            "{marker: \"payload\"}"
        );
        let none = wrapper(variants.get("None").unwrap());
        assert_eq!(none.get("inner").unwrap().to_string(), "'None");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn builtin_enum_type_constructors_validate_inputs_and_charge_quota() {
        let directory = fixture_dir();
        let invalid_path = directory.join("invalid.telora");
        fs::write(&invalid_path, "Option(1)").unwrap();
        let invalid = load_module(&invalid_path, BTreeMap::new(), 100_000).unwrap_err();
        assert!(invalid.message.contains("cannot unify Int with Type"));

        let quota_path = directory.join("quota.telora");
        fs::write(&quota_path, "Result(String, Int)").unwrap();
        let module = load_module(&quota_path, BTreeMap::new(), 100_000).unwrap();
        let mut account = QuotaAccount::new(Quota::new(10, 1_000, 0));
        let error = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            )
            .err()
            .expect("Result construction must exhaust allocation quota");
        assert_eq!(error.kind, crate::RuntimeErrorKind::AllocationQuotaExceeded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn union_validation_and_normalization_obey_current_contracts() {
        let directory = fixture_dir();
        let run_error = |name: &str, expression: &str| {
            let path = directory.join(name);
            fs::write(&path, expression).unwrap();
            let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
            module.execute(100_000).unwrap_err()
        };
        assert!(
            run_error("empty-union.telora", "union('None, [])")
                .message
                .contains("at least one variant")
        );
        assert!(
            run_error("union-variant.telora", "union('None, [1])")
                .message
                .contains("Type metadata")
        );
        assert!(
            run_error(
                "union-wrapper.telora",
                "union('None, [{kind: 'WithAttributes, inner: Int, attributes: []}])",
            )
            .message
            .contains("attributes must be a Dict")
        );

        let path = directory.join("quota.telora");
        fs::write(&path, "union('None, [Int, String])").unwrap();
        let module = load_module(path, BTreeMap::new(), 100_000).unwrap();
        let mut account = QuotaAccount::new(Quota::new(10, 1_000, 0));
        let error = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            )
            .err()
            .expect("model normalization must exhaust allocation quota");
        assert_eq!(error.kind, crate::RuntimeErrorKind::AllocationQuotaExceeded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_attributes_rejects_malformed_wrappers_and_obeys_allocation_quota() {
        let directory = fixture_dir();
        let path = directory.join("main.telora");
        fs::write(
            &path,
            r#"import "std/attributes" as attributes;
               attributes.normalize({kind: 'WithAttributes, inner: 1, attributes: []})"#,
        )
        .unwrap();
        let module = load_module(&path, BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(error.message.contains("attributes must be a Dict"));

        fs::write(
            &path,
            r#"import "std/attributes" as attributes;
               attributes.normalize(1)"#,
        )
        .unwrap();
        let module = load_module(&path, BTreeMap::new(), 100_000).unwrap();
        let mut account = QuotaAccount::new(Quota::new(10, 1_000, 0));
        let error = Vm::new()
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            )
            .err()
            .expect("normalization must exhaust allocation quota");
        assert_eq!(error.kind, crate::RuntimeErrorKind::AllocationQuotaExceeded);
        fs::remove_dir_all(directory).unwrap();
    }


    #[test]
    fn core_dict_rejects_invalid_arguments_pairs_and_duplicates() {
        let directory = fixture_dir();
        let run_error = |name: &str, expression: &str| {
            let path = directory.join(name);
            fs::write(&path, format!("import \"std/dict\" as dicts; {expression}")).unwrap();
            match load_module(path, BTreeMap::new(), 100_000) {
                Ok(module) => module.execute(100_000).unwrap_err().message,
                Err(error) => error.to_string(),
            }
        };

        assert!(run_error("keys.telora", "dicts.keys([])").contains("Dict"));
        assert!(run_error("merge.telora", "dicts.merge({}, [])").contains("right Dict"));
        assert!(run_error("pairs-array.telora", "dicts.from_pairs({})").contains("Array"));
        assert!(!run_error("pair-shape.telora", "dicts.from_pairs([(\"a\", 1, 2)])").is_empty());
        assert!(run_error("pair-key.telora", "dicts.from_pairs([('a, 1)])").contains("String"));
        let duplicate = run_error(
            "duplicate.telora",
            "dicts.from_pairs([(\"a\", 1), (\"a\", 2)])",
        );
        assert!(duplicate.contains("duplicate field"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn diamond_dependencies_reuse_the_same_persistent_root() {
        let directory = fixture_dir();
        let c = directory.join("c.telora");
        let a = directory.join("a.telora");
        let b = directory.join("b.telora");
        let main = directory.join("main.telora");
        fs::write(&c, r#"{value: [1, 2, 3]}"#).unwrap();
        fs::write(&a, r#"import "./c" as c; c"#).unwrap();
        fs::write(&b, r#"import "./c" as c; c"#).unwrap();
        fs::write(
            &main,
            r#"import "./a" as a; import "./b" as b; [a, b]"#,
        )
        .unwrap();
        let mut loader = ModuleLoader {
            resolver: ModuleResolver::for_root(&main).unwrap(),
            cache: HashMap::new(),
            builtin_modules: HashMap::new(),
            main: MainWorld::building(),
            visiting: Vec::new(),
            dependencies: BTreeSet::new(),
            module_quota: Quota::with_fuel(100_000),
            data_limits: DataLimits::default(),
            debug_sink: Arc::new(DiscardDebugSink),
            sources: SourceDatabase::default(),
            semantic_inputs: BTreeMap::new(),
            source_policy: ModuleSourcePolicy::ExpressionHarness,
        };

        loader.load_value(&main).unwrap();
        let main_id = loader.resolver.resolve_root(&main).unwrap().id;
        let a_id = loader.resolver.resolve_import(&main_id, "./a").unwrap().id;
        let b_id = loader.resolver.resolve_import(&main_id, "./b").unwrap().id;
        let c_id = loader
            .resolver
            .resolve_import(&a_id, "./c")
            .unwrap()
            .id;
        let root = |id: &ModuleCName| match loader.cache.get(id).unwrap() {
            ModuleState::Ready(artifact) => artifact.root,
        };

        assert_eq!(root(&a_id), root(&c_id));
        assert_eq!(root(&b_id), root(&c_id));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pending_modules_defer_imports_and_cache_initialization_outcomes() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "./missing" as missing;
               export { missing as output };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let pending = engine.prepare_module(&main).unwrap();
        assert_eq!(pending.path(), main);
        let first = pending.initialize().unwrap_err().to_string();
        let second = pending.initialize().unwrap_err().to_string();
        assert_eq!(first, second);
        assert!(first.contains("standalone/missing"), "{first}");

        fs::write(&main, "export def output = 42;").unwrap();
        let pending = engine.prepare_module(&main).unwrap();
        let first = pending.initialize().unwrap();
        let second = pending.initialize().unwrap();
        assert!(std::ptr::eq(first.module(), second.module()));
        assert_eq!(
            named_output(&engine.execute(first.module()).unwrap()).to_string(),
            "42"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn host_invocation_materializes_ready_definition_captures() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"def helper = fn(value) { value + 1 };
               def helper2 = fn(value) { helper(value) + 1 };
               export def factory: Fn(Int) -> Fn(Int) -> Int = fn(offset) {
                   fn(value) { helper2(value) + offset }
               };"#,
        )
        .unwrap();

        let engine = recovery_engine();
        let loaded = engine.load_module(&main, BTreeMap::new()).unwrap();
        let factory = engine.execute(&loaded).unwrap().select("factory").unwrap();
        let generated = engine
            .invoke_world(&loaded, factory, &[crate::DataWorld::int(2)])
            .unwrap();
        assert_eq!(
            engine
                .invoke_world(&loaded, generated, &[crate::DataWorld::int(38)])
                .unwrap()
                .to_string(),
            "42"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_blocks_failed_imports_and_keeps_independent_facts() {
        let directory = fixture_dir();
        let model = directory.join("model.telora");
        let main = directory.join("main.telora");
        fs::write(
            &model,
            "type Broken = missing(Int); type Good = String; export { Good };",
        )
        .unwrap();
        fs::write(
            &main,
            "import \"./model\" as model;\
             type Local = String;\
             type Uses = model.Good;\
             type Down = Array(Uses);\
             export { Local as output };",
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let main = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        let model = snapshot
            .module_by_path(&canonicalize(&model).unwrap())
            .unwrap();
        assert_eq!(main.state, WorkspaceModuleState::Available);
        assert_eq!(model.state, WorkspaceModuleState::Available);
        let fact = |module, name: &str| {
            &snapshot
                .definitions()
                .iter()
                .find(|definition| definition.module == module && definition.name == name)
                .unwrap()
                .ty
        };
        assert_eq!(fact(main.id, "Local").state, crate::FactState::Known);
        assert!(matches!(
            fact(main.id, "Uses").state,
            crate::FactState::Unknown(crate::UnknownReason::BlockedBy(_))
        ));
        assert!(matches!(
            fact(main.id, "Down").state,
            crate::FactState::Unknown(crate::UnknownReason::BlockedBy(_))
        ));
        assert_eq!(fact(model.id, "Good").state, crate::FactState::Known);
        let broken = fact(model.id, "Broken");
        let diagnostic = broken.diagnostics[0];
        assert!(
            snapshot.diagnostics()[diagnostic.index()]
                .message
                .contains("unknown binding")
        );
        assert!(main.imports.iter().any(|import| import.target == model.id));
        assert_ne!(main.source, model.source);
        assert_eq!(model.name, "standalone/model");
        assert_eq!(
            snapshot.sources().get(model.source.unwrap()).name.as_ref(),
            "standalone/model"
        );
        fs::remove_dir_all(directory).unwrap();
    }
