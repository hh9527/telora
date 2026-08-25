    #[test]
    fn regex_native_values_validate_structs_and_drive_typed_decode() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/regex" as re;
               import "std/result" as result;
               import "std/string" as string;
               let pattern = re.compile(r"^(?P<name>\w+)=(?P<value>\d+)(?:;(?P<unit>\w+))?$");
               @re.parse_by(pattern)
               type Rec = struct {
                   name: String,
                   value: Int,
                   unit: Option(String),
               };
               {
                   matched: re.is_match(pattern, "answer=42"),
                   equal: pattern == re.compile(r"^(?P<name>\w+)=(?P<value>\d+)(?:;(?P<unit>\w+))?$"),
                   text: result.unwrap(string.parse(String, "plain")),
                   number: result.unwrap(string.parse(Int, "42")),
                   float: result.unwrap(string.parse(Float, "1.5")),
                   first: result.unwrap(string.parse(Rec, "answer=42")),
                   second: result.unwrap(string.parse(Rec, "size=7;px")),
                   failed: string.parse(Rec, "not a record"),
                   bad_int: string.parse(Int, "4x"),
                   bad_float: string.parse(Float, "1.5x"),
               }"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 500_000).unwrap();
        let output = module.execute(500_000).unwrap().to_string();
        assert!(output.contains("matched: 'True"), "{output}");
        assert!(output.contains("equal: 'True"), "{output}");
        assert!(output.contains("text: \"plain\""), "{output}");
        assert!(output.contains("number: 42"), "{output}");
        assert!(output.contains("float: 1.5"), "{output}");
        assert!(
            output.contains("first: {name: \"answer\", unit: 'None, value: 42}"),
            "{output}"
        );
        assert!(
            output.contains("second: {name: \"size\", unit: 'Some(\"px\"), value: 7}"),
            "{output}"
        );
        assert!(output.contains("failed: 'Err("), "{output}");
        assert!(output.contains("bad_int: 'Err("), "{output}");
        assert!(output.contains("bad_float: 'Err("), "{output}");

        fs::write(
            &main,
            r#"import "std/regex" as re;
               @re.parse_by(re.compile(r"(?P<name>\w+)"))
               type Bad = struct { value: Int };
               { Bad }"#,
        )
        .unwrap();
        let error = load_module(&main, BTreeMap::new(), 500_000).unwrap_err();
        assert!(
            error
                .message()
                .contains("captures must match struct fields")
        );

        fs::write(&main, r#"import "std/regex" as re; re.compile(r"(")"#).unwrap();
        let module = load_module(&main, BTreeMap::new(), 500_000).unwrap();
        let error = module.execute(500_000).unwrap_err();
        assert!(error.message.contains("invalid regular expression"));

        fs::write(
            &main,
            r#"import "std/regex" as re;
               import "std/result" as result;
               import "std/string" as string;
               @re.parse_by(re.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
               type Endpoint = struct { host: String, port: Int };
               @re.parse_by(re.compile(r"^(?P<name>\w+)@(?P<endpoint>.+)$"))
               type Service = struct { name: String, endpoint: Endpoint };
               export def output = result.unwrap(string.parse(Service, "api@localhost:8080"));"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(
            output.contains("{endpoint: {host: \"localhost\", port: 8080}, name: \"api\"}"),
            "{output}"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn display_templates_validate_once_and_compose_nested_types() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{host}:{port}")
               type Endpoint = struct { host: String, port: Int };
               @fmt.display_by("{name}@{endpoint} {{ready}} {ratio} {name}")
               type Service = struct { name: String, endpoint: Endpoint, ratio: Float };
               export def output = fmt.render(fmt.display(Service, {
                   name: "api",
                   endpoint: { host: "localhost", port: 8080 },
                   ratio: -0.0,
               }));"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(
            named_output(&engine.execute(&module).unwrap()).to_string(),
            "\"api@localhost:8080 {ready} -0 api\""
        );

        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{missing}")
               type Bad = struct { value: Int };
               export { Bad };"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(error.message().contains("unknown field \"missing\""), "{error}");

        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{value")
               type Bad = struct { value: Int };
               export { Bad };"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(error.message().contains("unclosed Display template field"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typed_properties_are_queried_by_target_and_property_identity() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               import "std/type-property" as type_property;
               @fmt.display_by("{host}:{port}")
               type Endpoint = struct { host: String, port: Int };
               type Plain = struct { value: Int };
               def endpoint_property = type_property.get_type_prop(Endpoint, fmt.DisplayBy);
               def has_endpoint = match endpoint_property {
                   'Some(_) => 'True,
                   'None => 'False,
               };
               def has_plain = match type_property.get_type_prop(Plain, fmt.DisplayBy) {
                   'Some(_) => 'True,
                   'None => 'False,
               };
               def property_valid = match endpoint_property {
                   'Some(property) => validate(fmt.DisplayBy, property),
                   'None => 'Err("missing"),
               };
               export def output = {has_endpoint, has_plain, property_valid, target: Endpoint};"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let executed = engine.execute(&module).unwrap();
        let output = named_output(&executed);
        assert_eq!(output.get("has_endpoint").unwrap().to_string(), "'True");
        assert_eq!(output.get("has_plain").unwrap().to_string(), "'False");
        assert!(
            output
                .get("property_valid")
                .unwrap()
                .to_string()
                .starts_with("'Ok(")
        );
        let target = output
            .get("target")
            .unwrap()
            .declared_body()
            .expect("Endpoint Type metadata");
        assert_eq!(target.get("kind").unwrap().to_string(), "'WithAttributes");
        assert!(
            target
                .get("attributes")
                .unwrap()
                .dict_fields()
                .unwrap()
                .is_empty(),
            "typed property must not modify target metadata"
        );
        let target = target.get("inner").expect("Endpoint metadata body");
        assert_eq!(target.get("kind").unwrap().to_string(), "'Struct");
        assert_eq!(
            target.get("fields").unwrap().dict_fields().unwrap(),
            vec!["host", "port"]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn published_type_properties_satisfy_generic_constraints() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{host}:{port}")
               type Endpoint = struct { host: String, port: Int };
               def accept: for(T: Property(fmt.DisplayBy)) Fn(TypeOf(T)) -> String = fn(target) {
                   "accepted"
               };
               export def output = accept(Endpoint);"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let executed = engine.execute(&module).unwrap();
        assert_eq!(named_output(&executed).to_string(), "\"accepted\"");

        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               type Plain = struct { value: Int };
               def accept: for(T: Property(fmt.DisplayBy)) Fn(TypeOf(T)) -> String = fn(target) {
                   "accepted"
               };
               export def output = accept(Plain);"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(
            error.message().contains("has no published Property(DisplayBy) evidence"),
            "{error}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn type_property_evidence_links_across_module_boundaries() {
        let directory = fixture_dir();
        let model = directory.join("model.telora");
        let main = directory.join("main.telora");
        fs::write(
            &model,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{host}:{port}")
               type Endpoint = struct { host: String, port: Int };
               export { Endpoint };"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               import "./model" as model;
               def accept: for(T: Property(fmt.DisplayBy)) Fn(TypeOf(T)) -> String = fn(target) {
                   "accepted"
               };
               export def output = accept(model.Endpoint);"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let executed = engine.execute(&module).unwrap();
        assert_eq!(named_output(&executed).to_string(), "\"accepted\"");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn property_constrained_blanket_impl_dispatches_for_decorated_types() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               trait Display { display: Fn(Self) -> String };
               impl(T: Property(fmt.DisplayBy)) Display for T {
                   display: fn(value) { "decorated" },
               };
               @fmt.display_by("{host}:{port}")
               type Endpoint = struct { host: String, port: Int };
               def endpoint: Endpoint = { host: "localhost", port: 80 };
               export def output = Display.display(endpoint);"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let executed = engine.execute(&module).unwrap();
        assert_eq!(named_output(&executed).to_string(), "\"decorated\"");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fmt_display_uses_the_display_by_blanket_implementation() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{host}:{port}")
               type Endpoint = struct { host: String, port: Int };
               def endpoint: Endpoint = { host: "localhost", port: 8080 };
               export def output = fmt.render(fmt.Display.display(endpoint));"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let executed = engine.execute(&module).unwrap();
        assert_eq!(named_output(&executed).to_string(), "\"localhost:8080\"");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fmt_fragments_render_primitives_and_structured_concatenation() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               export def output = {
                   string: fmt.render(fmt.Display.display("host")),
                   int: fmt.render(fmt.Display.display(8080)),
                   float: fmt.render(fmt.Display.display(1.25)),
                   atom: fmt.render(fmt.Display.display('Ready)),
                   joined: fmt.render(fmt.concat(
                       ["[", "]:", ""],
                       [fmt.from_string("api"), fmt.from_int(3)],
                   )),
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        for expected in [
            "string: \"host\"",
            "int: \"8080\"",
            "float: \"1.25\"",
            "atom: \"Ready\"",
            "joined: \"[api]:3\"",
        ] {
            assert!(output.contains(expected), "{output}");
        }

        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               export def output = fmt.concat(["missing tail"], [fmt.from_int(1)]);"#,
        )
        .unwrap();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let error = engine.execute(&module).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("strings.len == items.len + 1"),
            "{error}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn standard_interpolation_uses_display_without_atom_representation_leaks() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"export def output = `value=\{1}/\{'Ready}`;"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(
            named_output(&engine.execute(&module).unwrap()).to_string(),
            "\"value=1/Ready\""
        );

        fs::write(
            &main,
            r#"type Flag = enum { 'On, 'Off };
               def flag: Flag = 'On;
               export def output = `flag=\{flag}`;"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(error.message().contains("does not implement"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interpolation_uses_static_display_evidence_for_nominal_values() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{host}:{port}")
               type Endpoint = struct { host: String, port: Int };
               def endpoint: Endpoint = { host: "localhost", port: 8080 };
               def show: for(T: fmt.Display) Fn(T) -> String = fn(value) {
                   `endpoint = \{value}`
               };
               export def output = show(endpoint);"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let executed = engine.execute(&module).unwrap();
        assert_eq!(
            named_output(&executed).to_string(),
            "\"endpoint = localhost:8080\""
        );

        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               type Plain = struct { value: Int };
               def plain: Plain = { value: 1 };
               export def output = `plain = \{plain}`;"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(
            error.message().contains("does not implement fmt.Display"),
            "{error}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interpolation_inherits_standard_display_identity_from_domain_modules() {
        let directory = fixture_dir();
        let model = directory.join("model.telora");
        let main = directory.join("main.telora");
        fs::write(
            &model,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{host}:{port}")
               type Endpoint = struct { host: String, port: Int };
               export { Endpoint };"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./model" as model;
               def endpoint: model.Endpoint = { host: "localhost", port: 8080 };
               export def output = `endpoint = \{endpoint}`;"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let executed = engine.execute(&module).unwrap();
        assert_eq!(
            named_output(&executed).to_string(),
            "\"endpoint = localhost:8080\""
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typed_properties_reject_targets_carriers_and_duplicate_identity() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let cases = [
            (
                "@property('Type) type Prop = Int; export { Prop };",
                "concrete nominal",
            ),
            (
                "@property('Type) type Prop(T) = struct { value: T }; export { Prop };",
                "concrete nominal struct or enum declarations",
            ),
            (
                r#"import "std/type-desc" { TypeDesc };
                   type Prop = struct {};
                   def deco: Fn(TypeDesc, Option(Prop)) -> Prop = fn(target, previous) { let value: Prop = {}; value };
                   @deco type Target = struct {};
                   export { Prop, Target };"#,
                "is not marked with @property",
            ),
            (
                r#"import "std/type-desc" { TypeDesc };
                   def deco: Fn(TypeDesc, Option(Int)) -> Int = fn(target, previous) { 1 };
                   @deco type Target = struct {};
                   export { Target };"#,
                "concrete nominal property type",
            ),
            (
                r#"import "std/type-desc" { TypeDesc };
                   @property('Field) type Prop = struct {};
                   def deco: Fn(TypeDesc, Option(Prop)) -> Prop = fn(target, previous) { let value: Prop = {}; value };
                   @deco type Target = struct {};
                   export { Prop, Target };"#,
                "does not support this decorator target",
            ),
        ];
        for (source, expected) in cases {
            fs::write(&main, source).unwrap();
            let error = recovery_engine().load_module(&main, BTreeMap::new()).unwrap_err();
            assert!(error.message().contains(expected), "{source}\n{error}");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn local_property_markers_are_source_order_independent() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/type-desc" { TypeDesc };
               import "std/type-property" as type_property;
               def deco: Fn(TypeDesc, Option(Prop)) -> Prop = fn(target, previous) { let value: Prop = {}; value };
               @deco type Target = struct {};
               @property('Type) type Prop = struct {};
               export def output = match type_property.get_type_prop(Target, Prop) {
                   'Some(_) => 'True,
                   'None => 'False,
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(
            named_output(&engine.execute(&module).unwrap()).to_string(),
            "'True"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn indexed_reflection_uses_canonical_member_coordinates() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/dyn" as dyn;
               import "std/type-desc" as type_desc;
               type User = struct { z: Int, a: String };
               type Choice = enum { 'Some(Int), 'None };
               def user: User = { z: 7, a: "alpha" };
               def choice: Choice = 'Some(9);
               def user_dyn = dyn.pack(User, user);
               def choice_dyn = dyn.pack(Choice, choice);
               export def output = {
                   fields: type_desc.fields(User),
                   variants: type_desc.variants(Choice),
                   first_field: dyn.project_with(String, dyn.get_field_value(user_dyn, 0)),
                   variant_index: dyn.get_variant_index(choice_dyn),
                   payload: match dyn.get_variant_payload(choice_dyn, 1) {
                       'Some(value) => dyn.project_with(Int, value),
                       'None => 'None,
                   },
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("name: \"a\""), "{output}");
        assert!(output.contains("name: \"z\""), "{output}");
        assert!(output.contains("first_field: 'Some(\"alpha\")"), "{output}");
        assert!(output.contains("variant_index: 1"), "{output}");
        assert!(output.contains("payload: 'Some(9)"), "{output}");

        fs::write(
            &main,
            r#"import "std/dyn" as dyn;
               type Choice = enum { 'Some(Int), 'None };
               def choice: Choice = 'Some(9);
               dyn.get_variant_payload(dyn.pack(Choice, choice), 0)"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(error.message.contains("variant index is 1, not 0"));

        fs::write(
            &main,
            r#"import "std/dyn" as dyn;
               type User = struct { value: Int };
               def user: User = { value: 1 };
               dyn.get_field_value(dyn.pack(User, user), 1)"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(error.message.contains("field index 1 is out of range"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn indexed_reflection_is_finite_for_recursive_nominal_types() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/dyn" as dyn;
               import "std/type-desc" as type_desc;
               type Node = struct { value: Int, next: Option(Node) };
               def node: Node = { value: 7, next: 'None };
               def next = dyn.get_field_value(dyn.pack(Node, node), 0);
               export def output = {
                   fields: type_desc.fields(Node),
                   next: dyn.project_with(Option(Node), next),
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("name: \"next\""), "{output}");
        assert!(output.contains("name: \"value\""), "{output}");
        assert!(output.contains("next: 'Some('None)"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn member_properties_fold_before_type_properties() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/type-desc" { TypeDesc };
               import "std/type-property" as prop;
               import "std/type-property" { FieldPropertyCtx };
               @property('Field)
               type Count = struct { value: Int };
               @property('Type)
               type Total = struct { value: Int };
               def count: Fn(Int) -> Fn(FieldPropertyCtx, Option(Count)) -> Count = fn(value) {
                   fn(ctx, previous) {
                       let prior = match previous { 'Some(item) => item.value, 'None => 0 };
                       let result: Count = { value: prior + value };
                       result
                   }
               };
               def total: Fn(TypeDesc, Option(Total)) -> Total = fn(target, previous) {
                   let count_value = match prop.get_field_prop(target, 0, Count) {
                       'Some(item) => item.value,
                       'None => 0,
                   };
                   let result: Total = { value: count_value };
                   result
               };
               @total
               type User = struct {
                   @count(2)
                   @count(3)
                   name: String,
               };
               export def output = {
                   field: prop.get_field_prop(User, 0, Count),
                   total: prop.get_type_prop(User, Total),
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("field: 'Some({value: 5})"), "{output}");
        assert!(output.contains("total: 'Some({value: 5})"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn static_trait_declarations_and_explicit_dictionaries_seal() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"trait Display { display: Fn(Self) -> String };
               type Endpoint = struct { host: String };
               impl Display for Endpoint {
                   display: fn(value) { value.host },
               };
               export def output = 42;"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(named_output(&engine.execute(&module).unwrap()).to_string(), "42");

        fs::write(
            &main,
            r#"trait Display { display: Fn(Self) -> String };
               type Endpoint = struct { host: String };
               impl Display for Endpoint {};
               export def output = 42;"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(error.message().contains("display"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn static_trait_evidence_links_across_module_boundaries() {
        let directory = fixture_dir();
        let capability = directory.join("capability.telora");
        let facade = directory.join("facade.telora");
        let main = directory.join("main.telora");
        fs::write(
            &capability,
            r#"trait Display { display: Fn(Self) -> String };
               impl Display for Int { display: fn(value) { "int" } };
               export def render: for(T: Display) Fn(T) -> String = fn(value) {
                   Display.display(value)
               };
               export { Display };"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./capability" as cap;
               export def output = {
                   direct: cap.Display.display(8),
                   generic: cap.render(9),
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("direct: \"int\""), "{output}");
        assert!(output.contains("generic: \"int\""), "{output}");

        fs::write(
            &facade,
            r#"import "./capability" { render };
               export { render };"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./facade" as facade;
               export def output = facade.render(10);"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(
            named_output(&engine.execute(&module).unwrap()).to_string(),
            "\"int\""
        );

        fs::write(
            &facade,
            r#"import "./capability" { render };
               let broken = missing;
               export { render };"#,
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&facade).unwrap();
        let facade_module = snapshot
            .module_by_path(&canonicalize(&facade).unwrap())
            .unwrap();
        let render = snapshot
            .definitions()
            .iter()
            .find(|definition| {
                definition.module == facade_module.id
                    && definition.name == "render"
                    && definition.kind == crate::DefinitionKind::Import
            })
            .expect("re-exported constrained function");
        assert_eq!(
            render.scheme.as_deref(),
            Some("for(T: Display) Fn(T) -> String")
        );

        let model = directory.join("model.telora");
        fs::write(
            &capability,
            r#"trait Display { display: Fn(Self) -> String };
               export { Display };"#,
        )
        .unwrap();
        fs::write(
            &model,
            r#"type Endpoint = struct { host: String };
               export { Endpoint };"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./capability" as cap;
               import "./model" as model;
               impl cap.Display for model.Endpoint { display: fn(value) { value.host } };
               export def output = 1;"#,
        )
        .unwrap();
        let error = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(error.message().contains("orphan impl"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn property_capabilities_merge_across_field_and_variant_owners() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/type-property" as prop;
               import "std/type-property" { FieldPropertyCtx, VariantPropertyCtx };
               @property('Type)
               @property('Field)
               @property('Variant)
               type Mark = struct { value: Int };
               def type_mark: Fn(Int) -> Fn(Type, Option(Mark)) -> Mark = fn(value) {
                   fn(ctx, previous) {
                       let prior = match previous { 'Some(item) => item.value, 'None => 0 };
                       let result: Mark = { value: prior + value };
                       result
                   }
               };
               def field_mark: Fn(FieldPropertyCtx, Option(Mark)) -> Mark = fn(ctx, previous) {
                   let result: Mark = { value: ctx.index + 10 };
                   result
               };
               def variant_mark: Fn(VariantPropertyCtx, Option(Mark)) -> Mark = fn(ctx, previous) {
                   let result: Mark = { value: ctx.index + 20 };
                   result
               };
               @type_mark(40)
               @type_mark(2)
               type User = struct { @field_mark name: String };
               type Choice = enum { @variant_mark 'Some(Int), 'None };
               export def output = {
                   type_prop: prop.get_type_prop(User, Mark),
                   field: prop.get_field_prop(User, 0, Mark),
                   variant: prop.get_variant_prop(Choice, 1, Mark),
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(
            output.contains("type_prop: 'Some({value: 42})"),
            "{output}"
        );
        assert!(output.contains("field: 'Some({value: 10})"), "{output}");
        assert!(output.contains("variant: 'Some({value: 21})"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn container_text_codec_bridge_round_trips_nested_values_and_schema() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/codec" as codec;
               import "std/fmt" as fmt;
               import "std/json" as json;
               import "std/regex" as re;
               import "std/result" as result;
               import "std/string" as string;

               @string.decode_by_parse
               @string.encode_by_display
               @fmt.display_by("{host}:{port}")
               @re.parse_by(re.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
               type Endpoint = struct { host: String, port: Int };

               type Config = struct { endpoint: Endpoint, name: String };
               def decoded = result.unwrap(codec.decode(Config, codec.encode(codec.Value, {
                   endpoint: "localhost:8080",
                   name: "dev",
               }) |> result.unwrap));
               export def output = {
                   decoded,
                   encoded: result.unwrap(codec.encode(codec.Value, decoded)),
                   direct: result.unwrap(codec.decode(Endpoint, codec.encode(codec.Value, "example.com:443") |> result.unwrap)),
                   schema: json.schema(Config),
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(
            output
                .contains("decoded: {endpoint: {host: \"localhost\", port: 8080}, name: \"dev\"}"),
            "{output}"
        );
        assert!(
            output.contains(
                "encoded: 'Object({endpoint: 'String(\"localhost:8080\"), name: 'String(\"dev\")})"
            ),
            "{output}"
        );
        assert!(
            output.contains("direct: {host: \"example.com\", port: 443}"),
            "{output}"
        );
        assert!(output.contains("endpoint: {type: \"string\"}"), "{output}");

        fs::write(
            &main,
            r#"import "std/codec" as codec;
               import "std/regex" as re;
               import "std/result" as result;
               import "std/string" as string;
               @string.decode_by_parse
               @re.parse_by(re.compile(r"^(?P<value>\d+)$"))
               type Bad = struct { value: Int };
               export def output = codec.decode(Bad, codec.encode(codec.Value, "42") |> result.unwrap);"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("must be used together"), "{output}");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dependency_imports_preserve_identity_across_relative_edges() {
        let directory = fixture_dir();
        let app = directory.join("app");
        let models = directory.join("models");
        fs::create_dir(&app).unwrap();
        fs::create_dir(&models).unwrap();
        fs::write(
            directory.join("telora-deps.json"),
            r#"{"name":"app","dependencies":{"models":{"path":"models"}}}"#,
        )
        .unwrap();
        fs::write(models.join("base.telora"), "export def answer = 42;").unwrap();
        fs::write(
            models.join("user.telora"),
            "import \"./base\" as base; export { base as base };",
        )
        .unwrap();
        let main = app.join("main.telora");
        fs::write(
            &main,
            "import \"models/user\" as user; export def output = user.base.answer;",
        )
        .unwrap();

        let engine = recovery_engine();
        let loaded = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(
            named_output(&engine.execute(&loaded).unwrap()).to_string(),
            "42"
        );
        let names = loaded
            .workspace
            .modules()
            .iter()
            .map(|module| module.name.as_str())
            .collect::<HashSet<_>>();
        assert!(names.contains("models/user"));
        assert!(names.contains("models/base"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn telora_show_interpreter_renders_supported_values() {
        let directory = fixture_dir();
        fs::write(
            directory.join("reference-show.telora"),
            include_str!("../../../../../examples/reference-show.telora"),
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./reference-show" as show;
               type User = struct {name: String, scores: Array(Int)};
               type Node = struct {value: Int, children: Array(Node)};
               type Choice = enum {'None, 'Some(String)};
               type Pair = Tuple([Int, String]);
               type Unary = Fn(Int) -> Int;
               let user: User = {name: "Ada", scores: [2, 3]};
               let none: Choice = 'None;
               let some: Choice = 'Some("x");
               let node: Node = {value: 1, children: [{value: 2, children: []}]};
               {
                   inferred: show.my_show(Int)(42),
                   explicit: show.my_show@[Int](Int)(42),
                   string: show.my_show(String)("a\"b\\c"),
                   array: show.my_show(Array(Int))([1, 2]),
                   tuple: show.my_show(Pair)((1, "x")),
                   record: show.my_show(User)(user),
                   atom: show.my_show(Choice)(none),
                   tagged: show.my_show(Choice)(some),
                   recursive: show.my_show(Node)(node),
                   function_error: show.my_show(Unary)(fn(value) { value }),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 500_000).unwrap();
        let output_world = module.execute(500_000).unwrap();
        let output = output_world.value();
        for (field, expected) in [
            ("inferred", "'Ok(\"42\")"),
            ("explicit", "'Ok(\"42\")"),
            ("string", "'Ok(\"\\\"a\\\\\\\"b\\\\\\\\c\\\"\")"),
            ("array", "'Ok(\"[1, 2]\")"),
            ("tuple", "'Ok(\"(1, \\\"x\\\")\")"),
            ("record", "'Ok(\"{name: \\\"Ada\\\", scores: [2, 3]}\")"),
            ("atom", "'Ok(\"'None\")"),
            ("tagged", "'Ok(\"'Some(\\\"x\\\")\")"),
            (
                "recursive",
                "'Ok(\"{children: [{children: [], value: 2}], value: 1}\")",
            ),
        ] {
            assert_eq!(output.get(field).unwrap().to_string(), expected, "{field}");
        }
        let (tag, payload) = output
            .get("function_error")
            .unwrap()
            .tagged_parts()
            .unwrap();
        assert_eq!(tag.as_atom().as_deref(), Some("Err"));
        assert_eq!(
            payload.to_string(),
            "\"unsupported my_show descriptor\""
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn telora_hash_interpreter_threads_state_and_distinguishes_structure() {
        let directory = fixture_dir();
        fs::write(
            directory.join("reference-hash.telora"),
            include_str!("../../../../../examples/reference-hash.telora"),
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./reference-hash" as reference;
               import "std/hash" as hash;
               type User = struct {name: String, scores: Array(Int)};
               type Renamed = struct {label: String, scores: Array(Int)};
               type Node = struct {value: Int, children: Array(Node)};
               type Choice = enum {'None, 'Some(String)};
               type Pair = Tuple([Int, Int]);
               type Unary = Fn(Int) -> Int;
               let user: User = {name: "Ada", scores: [2, 3]};
               let changed: User = {name: "Ada", scores: [2, 4]};
               let renamed: Renamed = {label: "Ada", scores: [2, 3]};
               let node: Node = {value: 1, children: [{value: 2, children: []}]};
               let changed_node: Node = {value: 1, children: [{value: 3, children: []}]};
               let state = hash.new();
               let first = reference.my_hash(User)(user, state);
               {
                   equal: first == reference.my_hash(User)(user, state),
                   changed: first == reference.my_hash(User)(changed, state),
                   field_name: first == reference.my_hash(Renamed)(renamed, state),
                   array_tuple: reference.my_hash(Array(Int))([1, 2], state) ==
                       reference.my_hash(Pair)((1, 2), state),
                   tag_payload: reference.my_hash(Choice)('None, state) ==
                       reference.my_hash(Choice)('Some(""), state),
                   alias_unchanged: hash.finish(state) == hash.finish(hash.new()),
                   recursive_equal: reference.my_hash(Node)(node, state) ==
                       reference.my_hash(Node)(node, state),
                   recursive_different: reference.my_hash(Node)(node, state) ==
                       reference.my_hash(Node)(changed_node, state),
                   function_error: reference.my_hash(Unary)(fn(value) { value }, state),
                   float_error: reference.my_hash(Float)(1.5, state),
                   opaque_error: reference.my_hash(hash.HashState)(state, state),
                   recursive_error: reference.my_hash(Array(Float))([1.0, 2.0], state),
               }"#,
        )
        .unwrap();
        let module =
            load_module(directory.join("main.telora"), BTreeMap::new(), 1_000_000).unwrap();
        let output_world = module.execute(1_000_000).unwrap();
        let output = output_world.value();
        for field in ["equal", "alias_unchanged", "recursive_equal"] {
            assert_eq!(output.get(field).unwrap().to_string(), "'True", "{field}");
        }
        for field in [
            "changed",
            "field_name",
            "array_tuple",
            "tag_payload",
            "recursive_different",
        ] {
            assert_eq!(output.get(field).unwrap().to_string(), "'False", "{field}");
        }
        for field in [
            "function_error",
            "float_error",
            "opaque_error",
            "recursive_error",
        ] {
            assert!(
                output.get(field).unwrap().to_string().starts_with("'Err("),
                "{field}"
            );
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_diagnostics_collect_without_implicit_host_publication() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let data = directory.join("project.json");
        fs::write(
            directory.join("explicit-diagnostics.telora"),
            include_str!("../../../../../examples/explicit-diagnostics.telora"),
        )
        .unwrap();
        fs::write(
            &data,
            r#"{"name":"","packages":[{"name":"","version":0},{"name":"ok","version":1}]}"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./explicit-diagnostics" as validation;
import "std/array" as arrays;
import "std/result" as result;
import "./project.json" { data as project };
def initial: Array(validation.DiagnosticRecord) = [];
import "std/codec" as codec;
def checked_input = codec.decode(validation.Project, project) |> result.unwrap;
def output = match validation.validate_project(checked_input, initial) {
    (checked, diagnostics) => {
        count: arrays.length(diagnostics),
        initial_count: arrays.length(initial),
        unchanged: checked == checked_input,
        messages: arrays.map(diagnostics, fn(item) { item.message }),
    },
};
export { output };"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 500_000).unwrap();
        let output_world = module.execute(500_000).unwrap();
        let output = named_output(&output_world);
        assert_eq!(output.dict_get("count").unwrap().to_string(), "3");
        assert_eq!(output.dict_get("initial_count").unwrap().to_string(), "0");
        assert_eq!(output.dict_get("unchanged").unwrap().to_string(), "'True");
        let messages = output.dict_get("messages").unwrap();
        assert_eq!(
            (0..messages.sequence_len().unwrap())
                .map(|index| messages.sequence_get(index).unwrap().to_string())
                .collect::<Vec<_>>(),
            [
                "\"project name must not be empty\"",
                "\"package name must not be empty\"",
                "\"package version must be positive\"",
            ]
        );

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        assert!(snapshot.diagnostics().is_empty());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fail_intrinsic_preserves_data_and_authored_rule_locations() {
        let directory = fixture_dir();
        fs::write(directory.join("user.json"), r#"{"age":42}"#).unwrap();
        let source = r#"import "./user.json" { data as user };
import "std/codec" as codec;
import "std/result" as result;
import "std/dyn" as dyn;
type User = struct {age: Int};
def inspect_i: Fn(Dyn) -> Int = fn(value) {
    match dyn.field(value, "age") {
        'Ok(age) => fail!("age rejected", age),
        'Err(error) => fail!(error.message, error),
    }
};
def inspect: for(A) Fn(TypeOf(A)) -> Fn(A) -> Int = interpreter!(inspect_i);
let checked = codec.decode(User, user) |> result.unwrap;
inspect(User)(checked)"#;
        fs::write(directory.join("main.telora"), source).unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(error.message.contains("age rejected"));
        let data = error.data_location().expect("blame data location");
        assert_eq!(
            module.sources.get(data.source).name.as_ref(),
            "fixture/user.json"
        );
        assert_eq!(
            module.sources.get(data.source).slice(data).as_deref(),
            Some("42")
        );
        let rule = error.rule_location().expect("blame rule location");
        assert_eq!(
            module.sources.get(rule.source).slice(rule).as_deref(),
            Some("inspect(User)(checked)")
        );
        let implementation = error
            .implementation_rule_location()
            .expect("blame implementation location");
        assert_eq!(
            module
                .sources
                .get(implementation.source)
                .slice(implementation)
                .as_deref(),
            Some("fail!(\"age rejected\", age)")
        );
        let rendered = error.to_string();
        assert!(rendered.contains("fixture/main:14:1"), "{rendered}");
        assert!(rendered.contains("user.json:1:8"), "{rendered}");
        assert!(rendered.contains("fixture/main:8:21"), "{rendered}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fail_rule_boundary_crosses_facade_modules() {
        let directory = fixture_dir();
        fs::write(
            directory.join("provider.telora"),
            r#"def reject: Fn(Int) -> Int = fn(value) { fail!("rejected", value) };
export { reject };"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./provider" as provider;
export def inspect: Fn(Int) -> Int = fn(value) { provider.reject(value) };"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade" as facade;
facade.inspect(7)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        let source_text = |location: crate::Loc| {
            module
                .sources
                .get(location.source)
                .slice(location)
                .expect("source slice")
                .into_owned()
        };
        assert_eq!(
            source_text(error.rule_location().expect("rule location")),
            "facade.inspect(7)"
        );
        assert_eq!(
            source_text(
                error
                    .implementation_rule_location()
                    .expect("implementation rule location")
            ),
            "fail!(\"rejected\", value)"
        );
        assert_eq!(
            source_text(error.data_location().expect("data location")),
            "7"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fail_rule_boundary_survives_native_callback_continuations() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
let reject: Fn(Int) -> Int = fn(value) { fail!("rejected", value) };
array.map([1], reject)"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        let source_text = |location: crate::Loc| {
            module
                .sources
                .get(location.source)
                .slice(location)
                .expect("source slice")
                .into_owned()
        };
        assert_eq!(
            source_text(error.rule_location().expect("rule location")),
            "array.map([1], reject)"
        );
        assert_eq!(
            source_text(error.data_location().expect("data location")),
            "1"
        );
        assert_eq!(
            source_text(
                error
                    .implementation_rule_location()
                    .expect("implementation rule location")
            ),
            "fail!(\"rejected\", value)"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interpreter_lifts_parameters_independently() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"def unary_i: Fn(Dyn) -> String = fn(value) { "unary" };
               def unary: for(A) Fn(TypeOf(A)) -> Fn(A) -> String = interpreter!(unary_i);

               def mixed_i: Fn(Dyn, Bool) -> String = fn(value, verbose) { "mixed" };
               def mixed: for(A) Fn(TypeOf(A)) -> Fn(A, Bool) -> String = interpreter!(mixed_i);

               def many_i: Fn(String, Dyn, Bool, Dyn, Dyn) -> String =
                   fn(prefix, a, flag, b, again_a) { prefix };
               def many: for(A, B) Fn(TypeOf(B), TypeOf(A)) ->
                   Fn(String, A, Bool, B, A) -> String = interpreter!(many_i);

               def metadata_i: Fn(Bool) -> String = fn(flag) { "metadata" };
               def metadata: for(A) Fn(TypeOf(A)) -> Fn(Bool) -> String =
                   interpreter!(metadata_i);

               {
                   unary: unary(Int)(1),
                   mixed: mixed(Int)(1, 'True),
                   many: many(Int, String)("many", "a", 'False, 2, "b"),
                   metadata: metadata(String)('True),
               }"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 200_000).unwrap();
        let output_world = module.execute(200_000).unwrap();
        let output = output_world.value();
        for (field, expected) in [
            ("unary", "\"unary\""),
            ("mixed", "\"mixed\""),
            ("many", "\"many\""),
            ("metadata", "\"metadata\""),
        ] {
            assert_eq!(output.get(field).unwrap().to_string(), expected);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn should_and_must_ok_select_warning_or_failure() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"def reject: Fn(String) -> Result(String, String) = fn(value) { 'Err("deprecated") };
export def output = reject.should_ok!("old");"#,
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            named_output(&module.execute(100_000).unwrap()).to_string(),
            "'None"
        );
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.severity == crate::source::Severity::Warning
                && diagnostic.message == "deprecated"
        }));

        fs::write(
            &main,
            r#"def reject: Fn(Int) -> Result(Int, String) = fn(value) { 'Err("invalid") };
export def output = reject.must_ok!(42);"#,
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let error = snapshot
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.message == "invalid")
            .expect("reported error");
        assert_eq!(error.severity, crate::source::Severity::Error);
        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let failure = module.execute(100_000).unwrap_err();
        assert_eq!(failure.kind, crate::RuntimeErrorKind::RaisedBlame);
        let strict = failure.diagnostic().expect("strict diagnostic");
        let normalize = |diagnostic: &crate::Diagnostic| {
            diagnostic
                .labels
                .iter()
                .map(|label| (label.location.range(), label.message.clone(), label.primary))
                .collect::<Vec<_>>()
        };
        assert_eq!(normalize(&strict), normalize(error));
        fs::remove_dir_all(directory).unwrap();
    }
