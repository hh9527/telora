    #[test]
    fn fmt_atom_contract_is_statically_typed() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               import "std/type-desc" as type_desc;
               def state: Atom = 'Ready;
               export def output = {
                   kind: type_desc.kind(Atom),
                   rendered: fmt.render(fmt.from_atom(state)),
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("kind: 'Atom"), "{output}");
        assert!(output.contains("rendered: \"Ready\""), "{output}");

        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               export def output = fmt.from_atom(1);"#,
        )
        .unwrap();
        let error = engine.load_module(&main, BTreeMap::new()).unwrap_err();
        assert!(error.message().contains("Atom"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_nominal_display_builds_structured_fmt() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/fmt" as fmt;
               @fmt.display_by("{host}:{port}")
               type Endpoint = struct { host: String, port: Int };
               impl fmt.Display for Endpoint {
                   display: fn(self) {
                       fmt.concat(
                           ["explicit(", ":", ")"],
                           [fmt.from_string(self.host), fmt.from_int(self.port)],
                       )
                   },
               };
               def endpoint: Endpoint = { host: "localhost", port: 8080 };
               export def output = `endpoint=\{endpoint}`;"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        assert_eq!(
            named_output(&engine.execute(&module).unwrap()).to_string(),
            "\"endpoint=explicit(localhost:8080)\""
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
