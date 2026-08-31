    #[test]
    fn tool_stage_properties_publish_prepared_closures_across_modules() {
        let directory = fixture_dir();
        let model = directory.join("model.telora");
        let facade = directory.join("facade.telora");
        let main = directory.join("main.telora");
        fs::write(
            &model,
            r#"import "std/array" as array;
                   import "std/dyn" as dyn;
                   import "std/type-desc" as type_desc;

                   @property('Type)
                   type Prepared = struct { run: Fn(Dyn) -> String };

                   def prepare: Fn(String) -> Fn(Type, Option(Prepared)) -> Prepared = fn(prefix) {
                       fn(target, previous) {
                           let run: Fn(Dyn) -> String = fn(value) {
                               let name = match dyn.get_field_value(value, 0) |> dyn.check_string {
                                   'Some(name) => name,
                                   'None => fail!("prepared field is not String", value),
                               };
                               let field_count = type_desc.fields(target) |> array.length;
                               `\{prefix}:\{field_count}:\{name}`
                           };
                           { run }
                       }
                   };

                   @prepare("ready")
                   type Target = struct { name: String };

                   export { Prepared, Target };"#,
        )
        .unwrap();
        fs::write(
            &facade,
            r#"import "./model" { Prepared, Target };
                   export { Prepared, Target };"#,
        )
        .unwrap();
        fs::write(
                &main,
                r#"import "./model" as direct;
                   import "./facade" as facade;
                   import "std/dyn" as dyn;
                   import "std/type-property" as type_property;

                   def prepared: for(P, T: Property(P)) Fn(TypeOf(T), TypeOf(P)) -> P = fn(target, property) {
                       type_property.evidence(target, property)
                   };
                   def first = prepared(direct.Target, direct.Prepared);
                   def second = prepared@[facade.Prepared, facade.Target](facade.Target, facade.Prepared);
                   def value = dyn.pack(direct.Target, { name: "Ada" });
                   export def output = {
                       same_function: first.run == second.run,
                       rendered: first.run(value),
                   };"#,
            )
            .unwrap();

        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("same_function: 'True"), "{output}");
        assert!(output.contains("rendered: \"ready:1:Ada\""), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codec_prepared_display_failures_preserve_rule_and_data_provenance() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let source = |output: &str| {
            format!(
                r#"import "std/codec" as codec;
    import "std/dyn" as dyn;
    import "std/fmt" as fmt;
    import "std/regex" as re;
    import "std/result" as result;
    import "std/string" as string;
    import "std/type-desc" {{ TypeDesc }};
    import "std/type-property" as type_property;

    def reject_display: Fn(TypeDesc, Option(fmt.DisplayBy)) -> fmt.DisplayBy = fn(target, previous) {{
        {{
            template: {{ strings: [""], fields: [] }},
            display: fn(value) {{
                fail!("prepared display rejected endpoint", dyn.get_field_value(value, 0))
            }},
        }}
    }};

    @string.decode_by_parse
    @string.encode_by_display
    @reject_display
    @re.parse_by(re.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
    type Endpoint = struct {{ host: String, port: Int }};

    let endpoint: Endpoint = {{ host: "example.com", port: 443 }};
    {output}"#
            )
        };
        fs::write(
            &main,
            source(
                r#"match type_property.get_type_prop(Endpoint, fmt.DisplayBy) {
        'Some(property) => property.display(dyn.pack(Endpoint, endpoint)),
        'None => fail!("missing DisplayBy", Endpoint),
    }"#,
            ),
        )
        .unwrap();
        let direct_module = load_module(&main, BTreeMap::new(), 200_000).unwrap();
        let direct_error = direct_module.execute(200_000).unwrap_err();

        fs::write(
            &main,
            source("codec.encode(codec.Value, endpoint) |> result.unwrap"),
        )
        .unwrap();
        let module = load_module(&main, BTreeMap::new(), 200_000).unwrap();
        let error = module.execute(200_000).unwrap_err();
        assert!(
            error.message.contains("prepared display rejected endpoint"),
            "{error}"
        );
        let data_location = error
            .data_location()
            .expect("prepared display data location");
        let direct_data_location = direct_error
            .data_location()
            .expect("direct prepared display data location");
        assert_eq!(
            module.sources.get(data_location.source).name.as_ref(),
            direct_module
                .sources
                .get(direct_data_location.source)
                .name
                .as_ref()
        );
        assert_eq!(
            module
                .sources
                .get(data_location.source)
                .slice(data_location)
                .as_deref(),
            direct_module
                .sources
                .get(direct_data_location.source)
                .slice(direct_data_location)
                .as_deref()
        );
        let rule_location = error.rule_location().expect("codec call rule location");
        assert_eq!(
            module
                .sources
                .get(rule_location.source)
                .slice(rule_location)
                .as_deref(),
            Some("codec.encode(codec.Value, endpoint)")
        );
        let implementation = error
            .implementation_rule_location()
            .expect("prepared display implementation location");
        let direct_implementation = direct_error
            .implementation_rule_location()
            .expect("direct prepared display implementation location");
        assert_eq!(
            module
                .sources
                .get(implementation.source)
                .slice(implementation)
                .as_deref(),
            direct_module
                .sources
                .get(direct_implementation.source)
                .slice(direct_implementation)
                .as_deref()
        );
        assert_eq!(
            module
                .sources
                .get(implementation.source)
                .slice(implementation)
                .as_deref(),
            Some("fail!(\"prepared display rejected endpoint\", dyn.get_field_value(value, 0))")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fmt_rendering_accounts_for_reused_fragment_output() {
        fn source(output: &str, depth: usize) -> String {
            let mut source = String::from(
                r#"import "std/fmt" as fmt;
                   def f0 = fmt.from_string("x");
                "#,
            );
            for depth in 1..=depth {
                source.push_str(&format!(
                    "def f{depth} = fmt.concat([\"\", \"\", \"\"], [f{}, f{}]);\n",
                    depth - 1,
                    depth - 1
                ));
            }
            if output == "interpolation" {
                source.push_str("type Rendered = struct { marker: Int };\n");
                source.push_str(&format!(
                    "impl fmt.Display for Rendered {{ display: fn(self) {{ f{depth} }} }};\n"
                ));
                source.push_str(
                    "def rendered: Rendered = { marker: 0 };\n\
                     export def output = `value=\\{rendered}`;",
                );
            } else {
                source.push_str(&format!("export def output = {output};"));
            }
            source
        }

        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let quota = Quota::new(1_000_000, 10_000, 50_000);

        fs::write(&main, source("f48", 48)).unwrap();
        let module = load_module(&main, BTreeMap::new(), 1_000_000).unwrap();
        module
            .execute_with_quota(quota)
            .expect("the shared Fmt graph itself fits the quota");

        for output in ["fmt.render(f48)", "interpolation"] {
            fs::write(&main, source(output, 48)).unwrap();
            let module = load_module(&main, BTreeMap::new(), 1_000_000).unwrap();
            let error = module.execute_with_quota(quota).unwrap_err();
            assert_eq!(
                error.kind,
                crate::RuntimeErrorKind::AllocationQuotaExceeded
            );
            let message = error.to_string();
            assert!(
                message.contains("allocation quota") && message.contains("exceeded"),
                "{message}"
            );
        }

        let unlimited = Quota::with_fuel(1_000_000);
        for output in ["fmt.render(f63)", "interpolation"] {
            fs::write(&main, source(output, 63)).unwrap();
            let module = load_module(&main, BTreeMap::new(), 1_000_000).unwrap();
            let error = module.execute_with_quota(unlimited).unwrap_err();
            assert_eq!(
                error.kind,
                crate::RuntimeErrorKind::AllocationQuotaExceeded
            );
            let message = error.to_string();
            assert!(message.contains("cannot be reserved"), "{message}");
        }
        fs::remove_dir_all(directory).unwrap();
    }
