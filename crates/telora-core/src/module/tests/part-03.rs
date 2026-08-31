    #[test]
    fn homogeneous_dict_metadata_preserves_types_through_core_codecs_and_schema() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/codec" as codec;
               import "std/dict" as dicts;
               import "std/json" as json;
               import "std/result" as result;
               type Env = Dict(String);
               let env: Env = {PATH: "/bin", HOME: "/tmp"};
               let decoded = codec.decode(
                   Env,
                   codec.encode(codec.Value, {SHELL: "/bin/sh"}) |> result.unwrap,
               ) |> result.unwrap;
               {
                   env: env,
                   decoded: decoded,
                   values: dicts.values(env),
                   built: dicts.from_pairs([("A", "one"), ("B", "two")]),
                   encoded: codec.encode(codec.Value, decoded) |> result.unwrap,
                   schema: json.schema(Env),
               }"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "{built: Dict<String>, decoded: Dict<String>, encoded: Value, env: Dict<String>, schema: Any, values: Array<Any>}"
        );
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        assert_eq!(
            output.get("values").unwrap().to_string(),
            "[\"/tmp\", \"/bin\"]"
        );
        assert_eq!(
            output.get("built").unwrap().to_string(),
            "{A: \"one\", B: \"two\"}"
        );
        assert_eq!(
            output.get("encoded").unwrap().to_string(),
            "'Object({SHELL: 'String(\"/bin/sh\")})"
        );
        let schema = output.get("schema").unwrap();
        assert_eq!(schema.get("type").unwrap().to_string(), "\"object\"");
        assert_eq!(
            schema.get("additionalProperties").unwrap().to_string(),
            "{type: \"string\"}"
        );

        fs::write(
            &main,
            r#"type Env = Dict(String);
               let env: Env = {GOOD: "yes", BAD: 1};
               env"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("BAD"), "{error}");
        assert!(error.to_string().contains("Int"), "{error}");
        assert!(error.to_string().contains("String"), "{error}");

        fs::write(
            &main,
            r#"type Fixed = struct {a: String};
               let dynamic: Dict(String) = {a: "value"};
               let fixed: Fixed = dynamic;
               fixed"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("not assignable"), "{error}");
        assert!(error.to_string().contains("Dict<String>"), "{error}");

        fs::write(
            &main,
            r#"type Fixed = struct {a: String};
               let read: Fn(Fixed) -> String = fn(value) { value.a };
               let dynamic: Dict(String) = {a: "value"};
               read(dynamic)"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(
            error.to_string().contains("cannot unify Dict<String>"),
            "{error}"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recursive_dict_metadata_reuses_existing_schema_links() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/json" as json;
               type Node = struct {children: Dict(Node)};
               json.schema(Node)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let schema = module.execute(100_000).unwrap().to_string();
        assert!(schema.contains("additionalProperties"), "{schema}");
        assert!(schema.contains("$defs"), "{schema}");
        assert!(schema.contains("$ref"), "{schema}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_core_exports_instantiate_per_member_access_but_not_per_local_use() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as arrays;
               {
                   ints: arrays.map([1, 2], fn(value) { value + 1 }),
                   strings: arrays.map(["a"], fn(value) { value }),
               }"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "{ints: Array<Int>, strings: Array<String>}"
        );

        fs::write(
            &main,
            r#"import "std/array" as arrays;
               let map = arrays.map;
               (map([1], fn(value) { value }), map(["a"], fn(value) { value }))"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 100_000).unwrap_err();
        assert!(error.to_string().contains("cannot unify String with Int"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_definition_exports_instantiate_per_member_access() {
        let directory = fixture_dir();
        fs::write(
            directory.join("identity.telora"),
            r#"decl identity: for(A) Fn(A) -> A;
               def identity = fn(value) { value };
               {identity: identity}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./identity" as generic;
               (generic.identity(1), generic.identity("x"), generic.identity@[_](2))"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "(Int, String, Int)"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(1, \"x\", 2)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_import_forms_do_not_leak_bound_identities_into_definition_checks() {
        let directory = fixture_dir();
        fs::write(
            directory.join("pipeline.telora"),
            r#"import "std/array" as array;
               export def relay:
                   for(Item, Context, Output, Result)
                   Fn(
                       Array(Item),
                       Context,
                       Fn(Item, Context) -> Output,
                       Fn(Array(Output)) -> Result
                   ) -> Result =
                   fn(items, context, transform, finish) {
                       finish(array.map(items, fn(item) {
                           transform(item, context)
                       }))
                   };"#,
        )
        .unwrap();

        for (name, import, relay) in [
            (
                "namespace.telora",
                r#"import "./pipeline" as pipeline;"#,
                "pipeline.relay",
            ),
            (
                "selective.telora",
                r#"import "./pipeline" {relay};"#,
                "relay",
            ),
            (
                "aliased.telora",
                r#"import "./pipeline" {relay as forward};"#,
                "forward",
            ),
            ("open.telora", r#"import "./pipeline" *;"#, "relay"),
        ] {
            fs::write(
                directory.join(name),
                format!(
                    r#"{import}
                       import "std/array" as array;
                       export def execute:
                           for(Prefix, Item, Context, Output, Result)
                           Fn(
                               Prefix,
                               Array(Item),
                               Context,
                               Fn(Prefix, Item, Context) -> Output,
                               Fn(Array(Output)) -> Result
                           ) -> Result =
                           fn(prefix, items, context, transform, finish) {{
                               {relay}(
                                   items,
                                   context,
                                   fn(item, current) {{
                                       transform(prefix, item, current)
                                   }},
                                   finish
                               )
                           }};
                       export def output = execute(
                           1,
                           [2, 3],
                           4,
                           fn(prefix, item, context) {{ prefix + item + context }},
                           fn(values) {{ array.length(values) }}
                       );"#,
                ),
            )
            .unwrap();

            let module = load_module(directory.join(name), BTreeMap::new(), 100_000).unwrap();
            let execute = module
                .analysis
                .hir
                .definitions()
                .iter()
                .find(|definition| definition.name == "execute")
                .expect("execute definition");
            assert_eq!(
                module.analysis.definition_schemes[&execute.id].display_name(),
                "for(Prefix, Item, Context, Output, Result) Fn(Prefix, Array<Item>, Context, Fn(Prefix, Item, Context) -> Output, Fn(Array<Output>) -> Result) -> Result",
                "{name}"
            );
            let (result, _) = module
                .execute_world_observed(Quota::with_fuel(100_000), Arc::new(DiscardDebugSink));
            let result = result.unwrap();
            assert!(
                result
                    .member_function_arity(&module.runtime.main.heap, "execute")
                    .unwrap()
                    .is_some(),
                "{name}"
            );
            assert!(
                result
                    .module_member_ref(&module.runtime.main.heap, "output")
                    .unwrap()
                    .and_then(ValueRef::as_int)
                    == Some(2),
                "{name}"
            );
        }

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn inferred_generic_let_exports_instantiate_per_member_access() {
        let directory = fixture_dir();
        fs::write(
            directory.join("identity.telora"),
            r#"let identity = fn(value) { value };
               {identity: identity}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./identity" as generic;
               (generic.identity(1), generic.identity("x"), generic.identity@[_](2))"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "(Int, String, Int)"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(1, \"x\", 2)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn acyclic_generic_def_exports_instantiate_per_member_access() {
        let directory = fixture_dir();
        fs::write(
            directory.join("identity.telora"),
            r#"def identity = fn(value) { value };
               {identity: identity}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./identity" as generic;
               (generic.identity(1), generic.identity("x"))"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "(Int, String)"
        );
        assert_eq!(module.execute(100_000).unwrap().to_string(), "(1, \"x\")");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typed_metadata_constructors_cross_module_interfaces() {
        let directory = fixture_dir();
        fs::write(
            directory.join("constructors.telora"),
            r#"def Maybe: for(A) Fn(TypeOf(A)) -> TypeOf(Option(A)) = fn(Item) { Option(Item) };
               {Maybe: Maybe}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./constructors" as constructors;
               type MaybeInt = constructors.Maybe(Int);
               let value: MaybeInt = 'None;
               value"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "enum {None, Some(Int)}"
        );
        assert_eq!(module.execute(100_000).unwrap().to_string(), "'None");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parameterized_type_families_preserve_schemes_across_import_forms() {
        let directory = fixture_dir();
        fs::write(
            directory.join("families.telora"),
            r#"type Status = enum {'Ready};
               type Box(A) = struct {status: Status, value: A};
               {Box: Box}"#,
        )
        .unwrap();

        for (name, import, family) in [
            (
                "whole.telora",
                r#"import "./families" as families;"#,
                "families.Box",
            ),
            (
                "selective.telora",
                r#"import "./families" {Box};"#,
                "Box",
            ),
            ("open.telora", r#"import "./families" *;"#, "Box"),
            (
                "aliased.telora",
                r#"import "./families" {Box as Container};"#,
                "Container",
            ),
        ] {
            fs::write(
                directory.join(name),
                format!(
                    r#"{import}
                       type IntBox = {family}(Int);
                       let value: IntBox = {{status: 'Ready, value: 42}};
                       value"#
                ),
            )
            .unwrap();
            let module = load_module(directory.join(name), BTreeMap::new(), 100_000).unwrap();
            assert_eq!(
                module.analysis.display(module.analysis.result_type),
                "Box",
                "{name}"
            );
            assert_eq!(
                module.execute(100_000).unwrap().to_string(),
                "{status: 'Ready, value: 42}",
                "{name}"
            );
        }

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parameterized_type_families_construct_local_concrete_types() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"type Box(A) = struct {value: A};
               type StringBox = Box(String);
               let value: StringBox = {value: "ready"};
               value"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.analysis.display(module.analysis.result_type), "Box");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{value: \"ready\"}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn declaration_initializers_preserve_structural_family_and_recursive_behavior() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"type Box(A) = struct {value: A};
               type Maybe(A) = enum {'None, 'Some(A)};
               type Node = struct {value: Int, children: Array(Node)};
               let boxed: Box(String) = {value: "ready"};
               let maybe: Maybe(Int) = 'Some(3);
               let node: Node = {value: 1, children: [{value: 2, children: []}]};
               (boxed.value, maybe, node.children[0].value)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let node = module
            .analysis
            .display(module.analysis.declared_types["Node"]);
        assert_eq!(node, "Node");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(\"ready\", 'Some(3), 2)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concrete_declared_types_preserve_identity_across_values_dyn_and_codec() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/codec" as codec;
               import "std/dyn" as dyn;
               import "std/result" as result;
               type Left = struct {value: Int};
               type Right = struct {value: Int};
               type State = enum {'Idle, 'Ready(Int)};
               let left: Left = {value: 1};
               let state: State = 'Ready(2);
               let decoded = codec.decode(
                   Left,
                   codec.encode(codec.Value, {value: 3}) |> result.unwrap,
               ) |> result.unwrap;
               let packed = dyn.pack(Left, decoded);
               {
                   left: left,
                   state: state,
                   decoded: decoded,
                   encoded: codec.encode(codec.Value, decoded) |> result.unwrap,
                   dyn_desc: dyn.desc(packed),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["left"]),
            "Left"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["state"]),
            "State"
        );
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        assert!(output.get("left").unwrap().is_declared());
        assert!(output.get("state").unwrap().is_declared());
        assert!(output.get("decoded").unwrap().is_declared());
        assert_eq!(
            output.get("dyn_desc").unwrap().kind(),
            crate::ValueKind::Type
        );
        assert_eq!(
            output.get("encoded").unwrap().to_string(),
            "'Object({value: 'Int(3)})"
        );

        fs::write(
            directory.join("wrong.telora"),
            r#"type Left = struct {value: Int};
               type Right = struct {value: Int};
               let left: Left = {value: 1};
               let right: Right = left;
               right"#,
        )
        .unwrap();
        let wrong =
            load_module(directory.join("wrong.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(wrong.to_string().contains("Left"));
        assert!(wrong.to_string().contains("Right"));

        fs::write(
            directory.join("erased.telora"),
            r#"import "std/codec" as codec;
               type Left = struct {value: Int};
               let raw: Any = {value: 1};
               codec.encode(codec.Value, raw)"#,
        )
        .unwrap();
        let erased =
            load_module(directory.join("erased.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            erased.execute(100_000).unwrap().to_string(),
            "'Ok('Object({value: 'Int(1)}))"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn equality_contextualizes_nominal_literals_in_both_operand_orders() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"type Wrapper = enum {'Box(String), 'Bag(Int)};
               type Status = enum {'Ready, 'Waiting};
               type Point = struct {x: Int};
               export {Wrapper, Status, Point};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./types" {Wrapper, Status, Point};
               let wrapper: Wrapper = 'Box("x");
               let expected: Wrapper = 'Box("x");
               let status: Status = 'Ready;
               let point: Point = {x: 1};
               {
                   annotated: wrapper == expected,
                   payload_right: wrapper == 'Box("x"),
                   payload_left: 'Box("x") == wrapper,
                   atom_right: status == 'Ready,
                   atom_left: 'Ready == status,
                   struct_right: point == {x: 1},
                   struct_left: {x: 1} == point,
               }"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{annotated: 'True, atom_left: 'True, atom_right: 'True, payload_left: 'True, payload_right: 'True, struct_left: 'True, struct_right: 'True}"
        );

        fs::write(
            directory.join("different.telora"),
            r#"type Left = struct {x: Int};
               type Right = struct {x: Int};
               let left: Left = {x: 1};
               let right: Right = {x: 1};
               left == right"#,
        )
        .unwrap();
        let different =
            load_module(directory.join("different.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(different.to_string().contains("cannot unify"));
        assert!(different.to_string().contains("Left"));
        assert!(different.to_string().contains("Right"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_function_contracts_brand_nested_private_nominal_literals() {
        let directory = fixture_dir();
        fs::write(
            directory.join("provider.telora"),
            r#"import "std/array" as array;
               type Status = enum {'Ready, 'Waiting};
               type Input = struct {statuses: Array(Status)};
               export def accepts: Fn(Input) -> Bool = fn(input) {
                   array.any(input.statuses, fn(status) { status == 'Ready })
               };"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./provider" {accepts}; accepts({statuses: ['Ready]})"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.execute(100_000).unwrap().to_string(), "'True");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_generic_struct_families_construct_nested_array_tuple_fields() {
        let directory = fixture_dir();
        fs::write(
            directory.join("families.telora"),
            r#"type Box(A) = struct {value: Array(Tuple([A, Int]))};
               def make: for(A) Fn(Array(Tuple([A, Int]))) -> Box(A) =
                   fn(value) { {value} };
               {Box, make}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./families" as families;
               families.make([("ready", 1)]).value"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.analysis.display(module.analysis.result_type),
            "Array<(String, Int)>"
        );
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "[(\"ready\", 1)]"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_declared_results_install_concrete_call_site_witnesses() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"type Box(A) = struct {value: A};
               def make: for(A) Fn(A) -> Box(A) = fn(value) { {value} };
               (make("ready"), make(1))"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let output_world = module.execute(100_000).unwrap();
        let output = output_world.value();
        let string_box = output.sequence_get(0).unwrap();
        let int_box = output.sequence_get(1).unwrap();
        let (string_owner, _) = string_box
            .declared_value_parts()
            .expect("Box(String) result has a nominal witness");
        let (int_owner, _) = int_box
            .declared_value_parts()
            .expect("Box(Int) result has a nominal witness");
        let (string_id, _, _) = string_owner.declared_type_parts().unwrap();
        let (int_id, _, _) = int_owner.declared_type_parts().unwrap();
        assert_eq!(string_id.arguments(), &[TypeDescriptor::String]);
        assert_eq!(int_id.arguments(), &[TypeDescriptor::Int]);
        assert_ne!(string_id, int_id);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_families_preserve_recursive_arguments_in_top_level_aliases() {
        let directory = fixture_dir();
        fs::write(
            directory.join("families.telora"),
            r#"type Box(A) = struct {value: A}; export { Box };"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./families" {Box};
               type Branch = struct {children: Array(Tree)};
               type Tree = enum {'Leaf(Int), 'Branch(Branch)};
               type TreeBox = Box(Tree);
               def identity: Fn(TreeBox) -> TreeBox = fn(value) { value };
               identity({value: 'Branch({children: ['Leaf(1)]})})"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let alias = module.analysis.declared_types["TreeBox"];
        assert_eq!(module.analysis.display(alias), "Box");
        let crate::TypeNode::Declared { body, .. } = module.analysis.types.node(alias) else {
            panic!("TreeBox must retain the Box application owner")
        };
        let body = module.analysis.types.display(*body);
        assert!(body.contains("Tree"), "{body}");
        assert!(!body.contains("Any"), "{body}");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{value: 'Branch({children: ['Leaf(1)]})}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_families_preserve_provider_recursive_fields_regardless_of_declaration_order() {
        let recursive_orders = [
            r#"type CallExpr = struct {name: String, args: Array(Expr)};
               type Expr = enum {'Literal(Literal), 'Column(ColumnRef), 'Call(CallExpr)};"#,
            r#"type Expr = enum {'Literal(Literal), 'Column(ColumnRef), 'Call(CallExpr)};
               type CallExpr = struct {name: String, args: Array(Expr)};"#,
        ];
        let imports = [
            (
                r#"import "./types" {Expr, Definition};"#,
                "Expr",
                "Definition",
            ),
            (
                r#"import "./types" as types;"#,
                "types.Expr",
                "types.Definition",
            ),
            (r#"import "./types" *;"#, "Expr", "Definition"),
        ];

        for recursive_types in recursive_orders {
            for (import, expr, definition) in imports {
                let directory = fixture_dir();
                let provider = r#"type Literal = struct {value: Int};
                   type ColumnRef = struct {alias: String, column: String};
                   $RECURSIVE_TYPES
                   type Definition(Id, Output, Input) = struct {
                       id: Id,
                       expr: Expr,
                       lower: Fn(Id, Input) -> Output,
                   };
                   export {Expr, CallExpr, Definition};"#
                    .replace("$RECURSIVE_TYPES", recursive_types);
                fs::write(directory.join("types.telora"), provider).unwrap();
                let consumer = r#"$IMPORT
                   type Id = enum {'Name};
                   type Output = struct {value: String};
                   type Input = enum {'All};
                   def column: Fn(String, String) -> $EXPR = fn(alias, name) {
                       'Column({alias: alias, column: name})
                   };
                   def lower: Fn(Id, Input) -> Output = fn(id, input) {
                       {value: "ready"}
                   };
                   type Concrete = $DEFINITION(Id, Output, Input);
                   let definitions: Array(Concrete) = [{
                       id: 'Name,
                       expr: column("t", "name"),
                       lower: lower,
                   }];
                   export def output = definitions;"#
                    .replace("$IMPORT", import)
                    .replace("$EXPR", expr)
                    .replace("$DEFINITION", definition);
                fs::write(directory.join("main.telora"), consumer).unwrap();

                let module =
                    load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
                for name in ["Concrete", "definitions", "output"] {
                    let ty = module
                        .analysis
                        .declared_types
                        .get(name)
                        .or_else(|| module.analysis.binding_types.get(name))
                        .copied()
                        .expect("tested binding has an analyzed type");
                    let ty = module.analysis.display(ty);
                    assert!(!ty.contains("Any"), "{name}: {ty}");
                }
                let output = module.execute(100_000).unwrap().to_string();
                assert!(output.contains("expr: 'Column"), "{output}");
                fs::remove_dir_all(directory).unwrap();
            }
        }
    }

    #[test]
    fn reexported_families_preserve_provider_recursive_fields() {
        let directory = fixture_dir();
        fs::write(
            directory.join("types.telora"),
            r#"type Call = struct {args: Array(Expr)};
               type Expr = enum {'Literal(Int), 'Call(Call)};
               type Family(A) = struct {expr: Expr, value: A};
               export {Expr, Family};"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./types" {Expr, Family}; export {Expr, Family};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade" {Family};
               type Concrete = Family(String);
               let value: Concrete = {expr: 'Literal(1), value: "ready"};
               export {value};"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        for name in ["Concrete", "value"] {
            let ty = module
                .analysis
                .declared_types
                .get(name)
                .or_else(|| module.analysis.binding_types.get(name))
                .copied()
                .expect("tested binding has an analyzed type");
            let ty = module.analysis.display(ty);
            assert!(!ty.contains("Any"), "{name}: {ty}");
        }
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{value: {expr: 'Literal(1), value: \"ready\"}}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reexported_recursive_generic_calls_accept_equivalent_result_annotations() {
        let directory = fixture_dir();
        fs::write(
            directory.join("expr.telora"),
            r#"type CallExpr = struct {name: String, args: Array(Expr)};
               type Expr = enum {'Literal(Int), 'Call(CallExpr)};
               export {Expr};"#,
        )
        .unwrap();
        fs::write(
            directory.join("plan.telora"),
            r#"import "./expr" {Expr};
               type Plan(A) = struct {value: A, expr: Expr};
               type Output = struct {text: String};
               def render: Fn(Expr) -> String = fn(expr) {
                   match expr {
                       'Literal(value) => `\{value}`,
                       'Call(call) => render(call.args[0]),
                   }
               };
               export def transform: for(A) Fn(Plan(A)) -> Output = fn(plan) {
                   {text: render(plan.expr)}
               };
               export {Plan, Output};"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./plan" {Plan, Output, transform};
               export {Plan, Output, transform};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade" as api;
               type Item = struct {id: Int};
               type ItemPlan = api.Plan(Item);
               type OutputAlias = api.Output;
               let plan: ItemPlan = {
                   value: {id: 1},
                   expr: 'Call({name: "f", args: ['Literal(1)]}),
               };
               let direct: api.Output = api.transform(plan);
               let alias: OutputAlias = api.transform(plan);
               export {direct, alias};"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{alias: {text: \"1\"}, direct: {text: \"1\"}}"
        );
        for name in ["direct", "alias"] {
            assert_eq!(
                module.analysis.display(module.analysis.binding_types[name]),
                "Output"
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generic_array_callbacks_preserve_recursive_nominal_items() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/array" as array;
               type Branch = struct {children: Array(Expr)};
               type Expr = enum {'Leaf(Int), 'Branch(Branch)};
               def eval: Fn(Expr) -> Int = fn(expr) {
                   match expr {
                       'Leaf(value) => value,
                       'Branch(branch) => array.fold(branch.children, 0, fn(total, child) {
                           total + eval(child)
                       }),
                   }
               };
               export def output: Int = eval('Branch({children: ['Leaf(1)]}));"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.execute(100_000).unwrap().to_string(), "{output: 1}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_family_aliases_preserve_provider_local_concrete_arguments() {
        let directory = fixture_dir();
        fs::write(
            directory.join("provider.telora"),
            r#"type Box(A) = struct {value: A}; export {Box};"#,
        )
        .unwrap();
        fs::write(
            directory.join("alias.telora"),
            r#"import "./provider" {Box};
               type Local = enum {'A};
               type LocalBox = Box(Local);
               export {LocalBox, Local};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./provider" {Box};
               import "./alias" {LocalBox, Local};
               def identity: Fn(LocalBox) -> LocalBox = fn(value) { value };
               let via_alias: LocalBox = identity({value: 'A});
               let direct: Box(Local) = {value: 'A};
               export def output = (via_alias, direct);"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["via_alias"]),
            "Box"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["direct"]),
            "Box"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["identity"]),
            "Fn(Box) -> Box"
        );
        assert_eq!(
            module
                .analysis
                .display(module.analysis.binding_types["output"]),
            "(Box, Box)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_instantiated_higher_order_creators_with_recursive_results() {
        let directory = fixture_dir();
        fs::write(
            directory.join("factory.telora"),
            r#"type Model(Subject, Output) = struct {subject: Subject, output: Output};
               def apply: for(Input, Output) Fn(Input, Fn(Input) -> Output) -> Output =
                   fn(input, callback) { callback(input) };
               export def make_creator:
                   for(Subject, Output)
                   Fn(Model(Subject, Output)) -> Fn(Subject) -> Output =
                   fn(model) { fn(subject) { model.output } };
               export def make_composed_creator:
                   for(Subject, Output)
                   Fn(Model(Subject, Output)) -> Fn(Subject) -> Output =
                   fn(model) {
                       fn(subject) { apply(subject, fn(current) { model.output }) }
                   };
               export { Model };"#,
        )
        .unwrap();
        fs::write(
            directory.join("domain.telora"),
            r#"import "./factory" {Model, make_creator, make_composed_creator};
               type Subject = enum {'Order};
               type CallExpr = struct {name: String, args: Array(Expr)};
               type Expr = enum {'Subject(Subject), 'Call(CallExpr)};
               let model: Model(Subject, Expr) = {
                   subject: 'Order,
                   output: 'Call({name: "root", args: ['Subject('Order)]}),
               };
               export def creator = make_creator(model);
               export def composed_creator = make_composed_creator(model);"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./domain" {creator, composed_creator};
               (creator('Order), composed_creator('Order))"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let result_type = module.analysis.display(module.analysis.result_type);
        assert_eq!(result_type, "(Expr, Expr)");
        assert!(!result_type.contains("Any"), "{result_type}");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "('Call({args: ['Subject('Order)], name: \"root\"}), 'Call({args: ['Subject('Order)], name: \"root\"}))"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_imported_local_bindings_without_creating_local_aliases() {
        let directory = fixture_dir();
        fs::write(
            directory.join("origin.telora"),
            r#"type Box(A) = struct {value: A};
               type Branch = struct {children: Array(Tree)};
               type Tree = enum {'Leaf(Int), 'Branch(Branch)};
               export def identity: for(A) Fn(A) -> A = fn(value) { value };
               export {Box, Tree};"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./origin" {Box as Container, Tree, identity};
               export {Container as Box, Tree, identity};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade" {Box, Tree, identity};
               type TreeBox = Box(Tree);
               export def output: TreeBox = identity({value: 'Leaf(1)});"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let alias = module
            .analysis
            .display(module.analysis.declared_types["TreeBox"]);
        assert_eq!(alias, "Box");
        assert!(!alias.contains("Any"), "{alias}");
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{output: {value: 'Leaf(1)}}"
        );

        fs::write(
            directory.join("invalid-local.telora"),
            r#"let a = 1; export {a as b}; export def output = b;"#,
        )
        .unwrap();
        let error = load_module(
            directory.join("invalid-local.telora"),
            BTreeMap::new(),
            100_000,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unknown binding \"b\""), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_imported_namespace_as_a_semantic_module() {
        let directory = fixture_dir();
        fs::write(
            directory.join("origin.telora"),
            r#"type Box(A) = struct {value: A};
               export def identity: for(A) Fn(A) -> A = fn(value) { value };
               export {Box};"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./origin" as model; export {model};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade" {model};
               type IntBox = model.Box(Int);
               export def output: IntBox = model.identity({value: 1});
               export def polymorphic = (model.identity(1), model.identity("x"));"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{output: {value: 1}, polymorphic: (1, \"x\")}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_open_imported_locals_through_multihop_facades() {
        let directory = fixture_dir();
        fs::write(
            directory.join("origin.telora"),
            r#"export def value = 7;
               export def identity: for(A) Fn(A) -> A = fn(item) { item };"#,
        )
        .unwrap();
        fs::write(
            directory.join("first.telora"),
            r#"import "./origin" *;
               export {value as answer, identity};"#,
        )
        .unwrap();
        fs::write(
            directory.join("second.telora"),
            r#"import "./first" {answer, identity as relay};
               export {answer, relay as identity};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./origin" {identity as direct};
               import "./second" {answer, identity};
               export def output = {
                   answer,
                   same: direct == identity,
                   values: (identity(1), identity("x")),
               };"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{output: {answer: 7, same: 'True, values: (1, \"x\")}}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn exports_imported_opaque_types_with_provider_identity() {
        let directory = fixture_dir();
        fs::write(
            directory.join("facade.telora"),
            r#"import "std/hash" {HashState}; export {HashState as State};"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade" {State};
               import "std/type-desc" as desc;
               export def output = {
                   kind: desc.kind(State),
                   name: desc.opaque_name(State),
               };"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "{output: {kind: 'Opaque, name: 'Some(\"std/hash#HashState\")}}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_generic_apis_widen_singleton_fields_in_anonymous_records() {
        let directory = fixture_dir();
        fs::write(
            directory.join("api.telora"),
            r#"def use: for(Req, Node) Fn(Array(Req), Fn(Req) -> Node) -> Node =
                   fn(requirements, selector) { selector(requirements[0]) };
               {use}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./api" as api;
               type Node = enum {'A, 'B};
               type Requirement = struct {target: Node};
               def target_of: Fn(Requirement) -> Node = fn(req) { req.target };
               api.use([{target: 'B}], target_of)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(module.analysis.display(module.analysis.result_type), "Node");
        assert_eq!(module.execute(100_000).unwrap().to_string(), "'B");
        fs::remove_dir_all(directory).unwrap();
    }
