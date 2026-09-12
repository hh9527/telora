#[test]
fn check_batch_roots_share_a_graph_and_obey_phase_boundaries() {
    let cwd = fixture();
    fs::create_dir_all(cwd.join("tests/nested")).unwrap();
    fs::write(cwd.join("src/main.telora"), "export def answer = 42;").unwrap();
    fs::write(cwd.join("src/_private.telora"), "export def unused = 1 / 0;").unwrap();
    fs::write(cwd.join("tests/nested/helper.telora"), "export def answer = 42;").unwrap();
    fs::write(cwd.join("tests/main.telora"), "import \"./nested/helper\" {answer}; export def result = answer;").unwrap();
    for (flags, count, initializes) in [
        (vec!["--lib"], 2, false),
        (vec!["--tests"], 2, true),
        (vec!["--lib", "--tests"], 4, false),
    ] {
        for types_only in [true, false] {
            let mut command = telora(&cwd);
            command.arg("check").args(&flags);
            if types_only { command.arg("--only-types"); }
            let output = command.output().unwrap();
            let text = String::from_utf8(output.stdout).unwrap();
            assert_eq!(output.status.success(), types_only || initializes, "{flags:?}: {text}\n{}", String::from_utf8_lossy(&output.stderr));
            let records = text.lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
            let summaries = records.iter().filter(|r| r["record"] == "summary").collect::<Vec<_>>();
            assert_eq!(summaries.len(), 1);
            assert_eq!(summaries[0]["roots"].as_array().unwrap().len(), count);
            if types_only { assert_eq!(summaries[0]["execution_seconds"], 0.0); }
        }
    }
    fs::write(cwd.join("tests/nested/helper.telora"), "export def answer: Int = \"wrong\";").unwrap();
    for flags in [vec!["check", "--tests"], vec!["check", "--tests", "--only-types"]] {
        let output = telora(&cwd).args(flags).output().unwrap();
        assert!(!output.status.success());
        let summary: Value = serde_json::from_str(String::from_utf8_lossy(&output.stdout).lines().last().unwrap()).unwrap();
        assert_eq!(summary["execution_seconds"], 0.0);
    }
    for args in [vec!["check"], vec!["check", "--lib", "@src/main"], vec!["check", "--tests", "@test/main"]] {
        assert_eq!(telora(&cwd).args(args).output().unwrap().status.code(), Some(2));
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn check_empty_batch_is_successful() {
    let cwd = fixture();
    fs::remove_dir(cwd.join("tests")).unwrap();
    for flag in ["--lib", "--tests"] {
        for types_only in [true, false] {
            let mut command = telora(&cwd);
            command.args(["check", flag]);
            if types_only { command.arg("--only-types"); }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            let summary: Value = serde_json::from_str(String::from_utf8_lossy(&output.stdout).lines().last().unwrap()).unwrap();
            assert_eq!(summary["roots"], serde_json::json!([]));
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_eval_with_executes_actor_service_with_solved_dyn_state() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), r#"
        import "std/entry" {main};
        import "std/actor" as actor;
        import "std/dyn" as dyn;
        import "std/value" {Value};
        def service = actor.service(Int.type, 41, fn(state, event) { (state + 1, []) });
        export def answer = main({sources: [], envs: [], args: False}, fn(ctx) {
            let transition = service.reduce((service.state, actor.Event.Request({id: "request", input: Value.None})));
            match dyn.project_with(Int.type, transition.0) {
                Some(value) => Value.Int(value),
                None => fail!("state witness mismatch"),
            }
        });
    "#).unwrap();
    let output = telora(&cwd).args(["eval-with", "@src/main:answer"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), serde_json::json!(42));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_check_injects_data_and_blocks_execution_after_type_errors() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), r#"
        import "./data.json" { data };
        import "std/value" { Value };
        @property(PropertyTarget.Type) type Mark = struct { value: Int };
        def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) {
            match data { Value.Int(n) => { value: n }, _ => fail!("data must precede property") }
        };
        @mark type Item = struct { value: Int };
        export {Item};
    "#).unwrap();
    for (data, valid) in [("42", true), ("invalid-json", false)] {
        fs::write(cwd.join("src/data.json"), data).unwrap();
        for types_only in [true, false] {
            let mut command = telora(&cwd);
            command.args(["check", "@src/main"]);
            if types_only { command.arg("--only-types"); }
            let output = command.output().unwrap();
            assert_eq!(output.status.success(), types_only || valid, "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
    }
    fs::write(cwd.join("src/main.telora"), "export def bad: Int = \"wrong\"; def forbidden: Int = fail!(\"must not execute\");").unwrap();
    let output = telora(&cwd).args(["check", "@src/main"]).output().unwrap();
    let records = String::from_utf8(output.stdout).unwrap().lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
    assert!(!output.status.success());
    assert!(!records.iter().any(|r| r["message"].as_str().is_some_and(|s| s.contains("must not execute"))));
    assert_eq!(records.last().unwrap()["execution_seconds"], 0.0);
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_check_executes_session_roots_after_static_solving() {
    let cwd = fixture();
    for (source, expected) in [
        ("def unused = 1 / 0; export def answer = 42;", Some("division")),
        ("def unused: Fn() -> Int = fn() { fail!(\"not called\") }; export def answer = 42;", None),
        ("@property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { fail!(\"check property sentinel\") }; @mark type Item = struct { value: Int }; export {Item};", Some("check property sentinel")),
    ] {
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        for types_only in [true, false] {
            let mut command = telora(&cwd);
            command.args(["check", "@src/main"]);
            if types_only { command.arg("--only-types"); }
            let output = command.output().unwrap();
            let text = String::from_utf8(output.stdout).unwrap();
            let records = text.lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
            let summary = records.iter().find(|r| r["record"] == "summary").unwrap();
            assert_eq!(output.status.success(), types_only || expected.is_none(), "{source}\n{text}\n{}", String::from_utf8_lossy(&output.stderr));
            assert_eq!(summary["types_only"], types_only);
            assert!(summary["static_seconds"].is_number());
            if types_only { assert_eq!(summary["execution_seconds"], 0.0); }
            if !types_only && let Some(expected) = expected {
                assert!(records.iter().any(|r| r["message"].as_str().is_some_and(|s| s.contains(expected))), "{text}");
            }
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_eval_property_initialization_precedes_both_entry_modes() {
    let cwd = fixture();
    for (mode, fail, queried_type, succeeds) in [
        ("eval", false, "Item", true),
        ("eval", true, "Int", false),
        ("eval", true, "Item", false),
        ("eval-with", false, "Item", true),
        ("eval-with", true, "Item", false),
    ] {
        let body = format!("match query({queried_type}.type, Mark.type) {{ Some(property) => Value.Int(property.value), None => Value.Int(42) }}");
        let entry = if mode == "eval" {
            format!("do {{ {body} }}")
        } else {
            format!("main({{ sources: [], envs: [], args: False }}, fn(ctx) {{ {body} }})")
        };
        let provider = if fail {
            "fail!(\"property-query-sentinel\")"
        } else {
            "{ value: config }"
        };
        fs::write(
            cwd.join("src/main.telora"),
            format!(
                r#"
            import "std/value" {{ Value }};
            import "std/entry" {{ main }};
            import "std/type-property" {{ get_type_prop as query }};
            @property(PropertyTarget.Type) type Mark = struct {{ value: Int }};
            def config = 42;
            def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) {{ {provider} }};
            @mark type Item = struct {{ value: Int }};
            export def answer = {entry};
        "#
            ),
        )
        .unwrap();
        let output = telora(&cwd)
            .args([mode, "@src/main:answer"])
            .output()
            .unwrap();
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.success(), succeeds, "{mode}: {error}");
        if succeeds {
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                serde_json::json!(42)
            );
        } else {
            assert!(output.stdout.is_empty());
            assert!(error.contains("property-query-sentinel"), "{error}");
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_eval_initializes_properties_before_reading_type_metadata() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), r#"
        import "std/value" { Value };
        @property(PropertyTarget.Type)
        type Mark = struct { value: Int };
        def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { fail!("initialization sentinel") };
        @mark
        type Item = struct { value: Int };
        type Alias = Item;
        export def answer = Value.Int(if Item.type == Alias.type { 42 } else { 0 });
    "#).unwrap();
    let output = telora(&cwd)
        .args(["eval", "@src/main:answer"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("initialization sentinel"));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_eval_initializes_globals_before_publishing_a_result() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        r#"
        import "std/value" { Value };
        def unused: Int = 1 / 0;
        def recurse: Fn(Int) -> Int = fn(n) { if n > 0 { recurse(n - 1) } else { 42 } };
        export def answer = Value.Int(if True { recurse(3) } else { unused });
    "#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["eval", "@src/main:answer"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("division by zero"));
    fs::write(
        cwd.join("src/main.telora"),
        r#"
        import "std/value" { Value };
        def a: Int = b;
        def b: Int = a;
        export def answer = Value.Int(a);
    "#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["eval", "@src/main:answer"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("cyclic demand") && error.contains("::a") && error.contains("::b"),
        "{error}"
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_eval_imports_data_after_static_solving() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        r#"
        import "std/value" { Value };
        import "./input.json" { data as j };
        import "./input.json" { data as again };
        import "./input.yaml" { data as y };
        import "./input.toml" { data as t };
        export def answer = Value.Object({ "j": j, "again": again, "y": y, "t": t });
    "#,
    )
    .unwrap();
    fs::write(cwd.join("src/input.json"), r#"{"x":[1,true,null]}"#).unwrap();
    fs::write(cwd.join("src/input.yaml"), "x: 2\n").unwrap();
    fs::write(cwd.join("src/input.toml"), "x = 3\n").unwrap();
    let output = telora(&cwd)
        .args(["eval", "@src/main:answer"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!({
            "j": {"x":[1,true,null]}, "again": {"x":[1,true,null]}, "y":{"x":2}, "t":{"x":3}
        })
    );
    fs::write(
        cwd.join("src/main.telora"),
        r#"
        import "std/entry" { main };
        import "./input.json" { data };
        export def evaluate = do {
            let initialized = data;
            main({ sources: [], envs: [], args: False }, fn(ctx) { initialized })
        };
    "#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["eval-with", "@src/main:evaluate"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!({"x":[1,true,null]})
    );
    fs::write(cwd.join("src/input.json"), "invalid json").unwrap();
    let output = telora(&cwd)
        .args(["check", "--only-types", "@src/main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = telora(&cwd)
        .args(["eval-with", "@src/main:evaluate"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("input.json"));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_eval_with_injects_declared_inputs_and_rejects_config_mismatches() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), r#"
        import "std/entry" { main };
        import "std/value" { Value };
        import "std/array" { map };
        import "std/dict" { values };
        export def evaluate = main({ sources: ["j", "t", "y"], envs: ["TELORA_MIR_EVAL_TEST"], args: True }, fn(ctx) {
            Value.Object({ "inputs": Value.Object(ctx.sources),
                "env": Value.Array(map(values(ctx.env), Value.String)),
                "args": Value.Array(map(ctx.args, Value.String)) })
        });
    "#).unwrap();
    let json = cwd.join("input.json");
    let yaml = cwd.join("input.yaml");
    let toml = cwd.join("input.toml");
    fs::write(&json, r#"{"x":[1,true,null]}"#).unwrap();
    fs::write(&yaml, "x: 2\n").unwrap();
    fs::write(&toml, "x = 3\n").unwrap();
    let inputs = [
        format!("j={}", json.display()),
        format!("y={}", yaml.display()),
        format!("t={}", toml.display()),
    ];
    let output = telora(&cwd)
        .env("TELORA_MIR_EVAL_TEST", "visible")
        .args([
            "eval-with",
            "@src/main:evaluate",
            "--source",
            &inputs[0],
            "--source",
            &inputs[1],
            "--source",
            &inputs[2],
            "--",
            "one",
            "two",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!({
            "inputs": {"j":{"x":[1,true,null]}, "y":{"x":2}, "t":{"x":3}},
            "env":["visible"], "args":["one","two"]
        })
    );
    let output = telora(&cwd)
        .args(["eval-with", "@src/main:evaluate"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("sources do not match"));
    for (config, expected) in [
        (
            "{ sources: [], envs: [], args: False }",
            "does not accept command-line",
        ),
        (
            "{ sources: [], envs: [\"TELORA_MIR_MISSING\"], args: True }",
            "cannot read declared environment",
        ),
        (
            "{ sources: [], envs: [\"x\", \"x\"], args: True }",
            "unique non-empty",
        ),
    ] {
        fs::write(
            cwd.join("src/main.telora"),
            format!(
                r#"
            import "std/entry" {{ main }}; import "std/value" {{ Value }};
            export def evaluate = main({config}, fn(ctx) {{ Value.Int(1 / 0) }});
        "#
            ),
        )
        .unwrap();
        let output = telora(&cwd)
            .env_remove("TELORA_MIR_MISSING")
            .args(["eval-with", "@src/main:evaluate", "--", "arg"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_eval_serializes_value_and_rejects_other_contracts_before_execution() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        r#"
        import "std/value" { Value };
        import "std/array" { map };
        export def answer = Value.Object({
            "values": Value.Array(map([1, 2], fn(x) { Value.Int(x * 21) })),
            "empty": Value.None,
            "empty_array": Value.Array([]),
            "empty_object": Value.Object({}),
            "false": Value.False,
            "float": Value.Float(1.5),
            "ok": Value.True,
            "text": Value.String("a\"b"),
        });
    "#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["eval", "@src/main:answer"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!({
            "values": [21, 42], "empty": null, "ok": true, "text": "a\"b",
            "empty_array": [], "empty_object": {}, "false": false, "float": 1.5,
        })
    );
    for source in [
        "export def answer = 1 / 0;",
        "import \"std/value\" { Value as Original }; type Value = enum { Int(Int) }; export def answer = Value.Int(42);",
    ] {
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("std/value.Value"));
    }
    for source in [
        "import \"std/value\" { Value }; export def answer = Value.Int(1 / 0);",
        "import \"std/value\" { Value }; export def answer = Value.LocalDate(\"2026-09-10\");",
    ] {
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty(), "no partial JSON may be published");
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_generic_native_signatures_determine_check_outcome() {
    let cwd = fixture();
    for (annotation, expected_code) in [("Bool", 0), ("String", 1)] {
        fs::write(
            cwd.join("src/main.telora"),
            format!(
                r#"
            import "std/array" {{ map as transform }};
            export def mapped: Array({annotation}) = transform([1, 2], fn(x) {{ x > 0 }});
        "#
            ),
        )
        .unwrap();
        let output = telora(&cwd)
            .args(["check", "--only-types", "@src/main"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected_code),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let records = jsonl(&output.stdout);
        let summary = records.iter().find(|r| r["record"] == "summary").unwrap();
        assert_eq!(
            summary["status"],
            if expected_code == 0 { "ok" } else { "error" }
        );
        if expected_code == 1 {
            assert!(summary["type_conflicts"].as_u64().unwrap() > 0);
            assert!(records.iter().any(|r| r["record"] == "diagnostic"
                && r["labels"].as_array().is_some_and(|l| !l.is_empty())));
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_proves_trait_property_dependencies_without_executing_functions() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/model.telora"),
        r#"
        @property(PropertyTarget.Type) type Tag = struct { value: Int };
        def tag: Fn(Type, Option(Tag)) -> Tag = fn(owner, previous) { fail!("provider executed") };
        @tag type Item = struct { value: Int };
        trait Named { name: Fn(Self) -> String };
        impl(T: Property(Tag)) Named for T { name: fn(value) { fail!("impl executed") } };
        def name: for(T: Named) Fn(T) -> String = fn(value) { Named.name(value) };
        export { Item, name };
    "#,
    )
    .unwrap();
    for (value, expected_code) in [("item", 0), ("1", 1)] {
        fs::write(
            cwd.join("src/main.telora"),
            format!(
                r#"
            import "./model" as model;
            def item: model.Item = {{ value: 1 }};
            export def answer = model.name({value});
        "#
            ),
        )
        .unwrap();
        let output = telora(&cwd)
            .args(["check", "--only-types", "@src/main"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected_code),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let records = jsonl(&output.stdout);
        let summary = records.iter().find(|r| r["record"] == "summary").unwrap();
        assert_eq!(summary["unknown_types"], 0);
        assert_eq!(summary["type_conflicts"], 0);
        assert_eq!(
            summary["unproven_bounds"].as_u64().unwrap() == 0,
            expected_code == 0
        );
        assert!(
            !records
                .iter()
                .any(|r| r["message"] == "provider executed" || r["message"] == "impl executed")
        );
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_query_returns_known_unknown_and_conflicted_without_evaluation() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        r#"
import "./data.json" { data };
export def answer = 1 / 0;
export def unknown = unknown;
export def unresolved = missing;
export def bad: Int = "wrong";
"#,
    )
    .unwrap();
    // An invalid data document must not be parsed in either static CLI consumer.
    fs::write(cwd.join("src/data.json"), "THIS IS NOT JSON").unwrap();
    let output = telora(&cwd)
        .args(["query", "exports", "@src/main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = jsonl(&output.stdout);
    let export = |name| {
        records
            .iter()
            .find(|r| r["record"] == "export" && r["name"] == name)
            .unwrap()
    };
    assert_eq!(export("answer")["type"], "Int");
    assert!(export("answer")["type_id"].is_number());
    assert_eq!(export("unknown")["state"], "Unknown");
    assert_eq!(export("unresolved")["state"], "Conflicted");
    assert_eq!(export("bad")["state"], "Conflicted");
    assert!(records.iter().any(|r| {
        r["record"] == "diagnostic"
            && r["message"]
                .as_str()
                .is_some_and(|m| m.contains("unknown binding \"missing\""))
    }));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("division by zero"));

    {
        let output = telora(&cwd)
            .args(["check", "--only-types", "@src/main"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let records = jsonl(&output.stdout);
        let summary = records.iter().find(|r| r["record"] == "summary").unwrap();
        assert_eq!(summary["types_only"], true);
        assert!(summary["type_conflicts"].as_u64().unwrap() > 0);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("division by zero"));
        assert!(!records.iter().any(|r| {
            r["labels"]
                .as_array()
                .is_some_and(|labels| labels.iter().any(|l| l["source"] == "fixture/data.json"))
        }));
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_data_exports_have_the_resolved_value_type_without_parsing_data() {
    let cwd = fixture();
    fs::write(cwd.join("src/payload.json"), "THIS IS NOT JSON").unwrap();
    fs::write(
        cwd.join("src/main.telora"),
        r#"
        import "./payload.json" { data };
        import "std/value" { Value };
        export def payload: Value = data;
        export def unevaluated = 1 / 0;
    "#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["check", "--only-types", "@src/main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let summary = jsonl(&output.stdout)
        .into_iter()
        .find(|r| r["record"] == "summary")
        .unwrap();
    assert_eq!(summary["unknown_types"], 0);
    assert_eq!(summary["unproven_bounds"], 0);
    let output = telora(&cwd)
        .args(["query", "exports", "@src/main"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(jsonl(&output.stdout).iter().any(|r| r["record"] == "export"
        && r["name"] == "payload"
        && r["state"] == "Known"
        && r["type_id"].is_number()));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_query_links_imports_and_source_positions() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/math.telora"),
        "export def inc = fn(x) { x + 1 };",
    )
    .unwrap();
    fs::write(
        cwd.join("src/main.telora"),
        "import \"@src/math\" { inc };\nexport def answer = inc(41);\n",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["query", "at", "@src/main:2:20"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let records = jsonl(&output.stdout);
    assert!(records.iter().any(|r| r["record"] == "reference"
        && r["name"] == "inc"
        && r["resolution"] == "Bound"
        && r["target_id"].is_number()));
    assert!(
        records.iter().any(|r| r["record"] == "expression"
            && r["type"] == "Int"
            && r["type_slot"].is_number())
    );

    fs::write(
        cwd.join("tests/probe.telora"),
        "import \"@src/math\" { inc }; export def answer = inc(1);",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["query", "exports", "@test/probe"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        jsonl(&output.stdout)
            .iter()
            .any(|r| r["record"] == "export" && r["name"] == "answer" && r["type"] == "Int")
    );
    fs::remove_dir_all(cwd).unwrap();
}
#[test]
fn static_mir_run_drives_a_host_ees_reply_with_explicit_value_input() {
    let cwd = fixture();
    let data = cwd.join("data");
    fs::create_dir_all(&data).unwrap();
    let connection = rusqlite::Connection::open(data.join("catalog.sqlite")).unwrap();
    connection.execute_batch("CREATE TABLE items (score INTEGER); INSERT INTO items VALUES (42);").unwrap();
    drop(connection);
    fs::write(cwd.join("src/main.telora"), r#"
        import "std/entry" as entry;
        import "std/ees" as ees;
        import "std/actor" as actor;
        import "std/value" {Value};
        type State = enum {Ready, Waiting(String)};
        def config: ees.Config = {vars: {}, models: [ees.sqlite_model("catalog", "user-data:catalog.sqlite")]};
        export def main = entry.run(State.type, {sources: [], envs: [], args: False}, config, fn(ctx) {
            (State.Ready, fn(state, event) {
                match (state, event) {
                    (State.Ready, actor.Event.Request(request)) => (
                        State.Waiting(request.id),
                        [actor.ees_call("query", request.id, ees.request("catalog", "Query", Value.Object({
                            sql: Value.String("SELECT score FROM items"), bindings: Value.Array([]),
                        })))],
                    ),
                    (State.Waiting(id), actor.Event.EesReply(reply)) => match reply.result {
                        Ok(value) => (State.Ready, [actor.reply(id, value)]),
                        Err(message) => fail!(message),
                    },
                    _ => fail!("unexpected event"),
                }
            })
        });
    "#).unwrap();
    let output = telora(&cwd).args(["run", "@src/main:main"]).env("XDG_DATA_HOME", &data).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), serde_json::json!({"columns": ["score"], "rows": [[42]]}));
    fs::remove_dir_all(cwd).unwrap();
}
#[test]
fn static_mir_serve_initializes_and_handles_eof_in_the_new_session() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), r#"
        import "std/entry" as entry;
        import "std/ees" as ees;
        export def main = entry.serve(Int.type, {sources: [], envs: [], args: False}, ees.none,
            fn(ctx) { (0, fn(state, event) { (state, []) }) });
    "#).unwrap();
    let output = telora(&cwd).args(["serve", "@src/main:main", "--bind", "stdio://"]).stdin(Stdio::null()).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stdout.is_empty());
    fs::remove_dir_all(cwd).unwrap();
}
#[test]
fn static_mir_serve_collects_a_failed_request_and_continues() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), r#"
        import "std/entry" as entry;
        import "std/ees" as ees;
        import "std/actor" as actor;
        import "std/value" {Value};
        export def main = entry.serve(Int.type, {sources: [], envs: [], args: False}, ees.none, fn(ctx) {
            (42, fn(state, event) {
                match event {
                    actor.Event.Request(request) => if request.id == "request-0" {
                        fail!("rejected")
                    } else { (state, [actor.reply(request.id, Value.Int(state))]) },
                    _ => fail!("unexpected EES reply"),
                }
            })
        });
    "#).unwrap();
    let mut child = telora(&cwd).args(["serve", "@src/main:main", "--bind", "stdio://"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"null\nnull\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let replies = jsonl(&output.stdout);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"], true);
    assert_eq!(replies[0]["diagnostics"][0]["message"], "rejected");
    assert_eq!(replies[1]["ok"], 42);
    fs::remove_dir_all(cwd).unwrap();
}
#[test]
fn dump_types_layout_is_static_deterministic_and_hidden() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), r#"
        import "std/prelude" {Int as Number};
        export type Rec = struct { a: Number, b: Array(Int) };
        export type Choices = enum { Empty, Items(Array(Int)) };
        export type Recursive = enum { End, More(Recursive) };
        export type Box(T) = struct { value: T };
        export def x: Rec = { a: 1 / 0, b: [1, 2] };
        export def choice: Choices = Choices.Items([1]);
        export def boxed: Box(Int) = {value: 1};
        export def metadata = Int.type;
    "#).unwrap();
    let help = telora(&cwd).args(["check", "--help"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&help.stdout).contains("dump-types-layout"));
    assert!(!telora(&cwd).args(["check", "--new-types-layout", "@src/main"]).output().unwrap().status.success());
    assert!(!telora(&cwd).args(["check", "@src/main", "--dump-types-layout"]).output().unwrap().status.success());
    let run = |flags: &[&str]| {
        let output = telora(&cwd).arg("check").args(flags).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let records = String::from_utf8(output.stdout).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).collect::<Vec<_>>();
        assert!(records.iter().all(|r| r["record"] == "diagnostic" || r["record"] == "summary"));
        assert_eq!(records.last().unwrap()["execution_seconds"], 0.0);
        assert_eq!(records.last().unwrap()["types_only"], true);
        serde_json::from_slice::<Value>(&fs::read(cwd.join("layout.json")).unwrap()).unwrap()
    };
    let flags = ["--dump-types-layout", "layout.json", "@src/main"];
    fs::write(cwd.join("layout.json"), "old content").unwrap();
    let report = run(&flags);
    assert_eq!(report["schema"], "telora.types-layout/v1");
    let bytes = fs::read(cwd.join("layout.json")).unwrap();
    assert_eq!(report, run(&flags));
    assert_eq!(bytes, fs::read(cwd.join("layout.json")).unwrap());
    assert_eq!(report, run(&["--dump-types-layout", "layout.json", "--only-types", "@src/main"]));
    let first = report["types"].as_array().unwrap();
    for row in first {
        let entry = &row["entry"];
        if entry["constructor"] == "Type" || (entry["constructor"] == "TypeOf" && entry["layout"]["status"] != "template") {
            assert_eq!(entry["layout"]["shape"]["data_bytes"], 4);
            assert_eq!(entry["layout"]["shape"]["value_bytes"], 24);
        }
        if entry["constructor"] == "Bytes" {
            assert_eq!(entry["layout"]["shape"]["value_bytes"], 32);
            assert_eq!(entry["layout"]["shape"]["table"], "BytesTable");
            assert_eq!(entry["object"]["element_stride"], 1);
        }
    }
    assert!(first.iter().any(|r| r["entry"]["constructor"] == "TypeOf" && r["entry"]["layout"]["status"] == "known"));
    assert!(first.iter().any(|r| r["entry"]["constructor"] == "Meta"));
    let rec = first.iter().find(|r| r["entry"]["object"]["members"].as_array().is_some_and(|m| m.len() == 2 && m[0]["name"] == "a" && m[1]["name"] == "b")).unwrap();
    assert_eq!(rec["entry"]["object"]["members"][0]["offset"], 0);
    assert_eq!(rec["entry"]["object"]["members"][1]["offset"], 24);
    assert_eq!(rec["entry"]["object"]["bytes"], 56);
    assert!(first.iter().any(|r| r["entry"]["object"]["element_stride"] == 24));
    let choices = first.iter().find(|r| r["entry"]["variants"].as_array().is_some_and(|v| v.iter().any(|v| v["name"] == "Items"))).unwrap();
    assert_eq!(choices["entry"]["layout"]["shape"]["value_bytes"], 56);
    assert_eq!(choices["entry"]["variants"][1]["offset"], 24);
    assert!(first.iter().any(|r| r["entry"]["layout"]["status"] == "template"));
    let recursive = first.iter().find(|r| r["entry"]["variants"].as_array().is_some_and(|v| v.iter().any(|v| v["name"] == "More"))).unwrap();
    assert_eq!(recursive["entry"]["layout"]["status"], "known");
    assert_eq!(recursive["entry"]["variants"][1]["storage"], "heap_id");
    assert_eq!(recursive["entry"]["layout"]["shape"]["value_bytes"], 32);
    assert_eq!(report["summary"]["closed"], true);
    assert!(first.iter().all(|r| r["entry"]["layout"]["status"] != "pending"));
    assert!(!telora(&cwd).args(["check", "@src/main"]).output().unwrap().status.success());
    for selection in [vec!["--lib"], vec!["--tests"], vec!["--lib", "--tests"]] {
        let mut flags = vec!["--dump-types-layout", "layout.json"];
        flags.extend(selection);
        run(&flags);
    }
    let empty = run(&["--dump-types-layout", "layout.json", "--tests"]);
    assert_eq!(empty["summary"]["types"], 0);
    let before = fs::read(cwd.join("layout.json")).unwrap();
    fs::create_dir(cwd.join("destination-dir")).unwrap();
    fs::write(cwd.join("destination-dir/keep"), "keep").unwrap();
    let output = telora(&cwd).args(["check", "@src/main", "--dump-types-layout", "destination-dir"]).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(cwd.join("destination-dir/keep")).unwrap(), "keep");
    assert_eq!(fs::read_dir(&cwd).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with(".tmp")).count(), 0);
    fs::write(cwd.join("src/main.telora"), "export def x: Int = \"bad\";").unwrap();
    let output = telora(&cwd).arg("check").args(flags).output().unwrap();
    assert!(!output.status.success());
    assert_eq!(before, fs::read(cwd.join("layout.json")).unwrap());
    let output = telora(&cwd).args(["check", "@src/main", "--dump-types-layout", "absent.json"]).output().unwrap();
    assert!(!output.status.success());
    assert!(!cwd.join("absent.json").exists());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("\"record\":\"type_layout\""));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn concrete_layouts_close_recursive_wrapped_callable_and_dynamic_types() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), r#"
        import "std/regex" as regex;
        import "std/hash" as hash;
        export type Left = enum { Stop, Next(Right) };
        export type Right = enum { Step(Left) };
        export type Dead = enum { Again(Dead) };
        export type Nested = enum { Value(Left), Flag(Bool) };
        export type Wrapped = struct(Int);
        export type Rec = struct { item: Int };
        export def pair: (Int, String) = (1, "hello");
        export def wrapped = Wrapped(1);
        export def factory = fn(x: Int) { fn(y: Int) { x + y } };
        export def poly: for(T) Fn(T) -> T = fn(x) { x };
        export def inferred_identity = fn(x) { x };
        export def same_poly = poly == poly;
        export def unchecked: Fn(Unchecked(Rec)) -> Int = fn(x) { x.item };
        export def empty: Array(Never) = [];
        export def dictionary: Dict(Int) = {x: 1};
        export def accepts_dyn = fn(value: Dyn) { value };
        export def dynamics: Array(Dyn) = [];
    "#).unwrap();
    let output = telora(&cwd).args(["check", "@src/main", "--dump-types-layout", "layout.json"]).output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&fs::read(cwd.join("layout.json")).unwrap()).unwrap();
    assert_eq!(report["summary"]["closed"], true);
    let entries = report["types"].as_array().unwrap();
    for name in ["Left", "Right"] {
        let row = entries.iter().find(|r| r["type_name"] == name && r["entry"]["constructor"] == "Nominal").unwrap();
        assert_eq!(row["entry"]["layout"]["shape"]["value_bytes"], 32);
        assert!(row["entry"]["variants"].as_array().unwrap().iter().any(|v| v["storage"] == "heap_id"));
    }
    assert!(entries.iter().any(|r| r["type_name"] == "Dead" && r["entry"]["layout"]["status"] == "uninhabited"));
    let nested = entries.iter().find(|r| r["type_name"] == "Nested" && r["entry"]["constructor"] == "Nominal").unwrap();
    assert_eq!(nested["entry"]["layout"]["shape"]["value_bytes"], 56);
    assert!(nested["entry"]["variants"].as_array().unwrap().iter().all(|v| v["storage"] == "full_value"));
    let wrapped = entries.iter().find(|r| r["type_name"] == "Wrapped" && r["entry"]["constructor"] == "Nominal").unwrap();
    assert_eq!(wrapped["entry"]["object"]["bytes"], 24);
    assert_eq!(wrapped["entry"]["layout"]["shape"]["table"], "NewtypeTable");
    assert!(entries.iter().any(|r| r["entry"]["constructor"] == "Tuple" && r["entry"]["object"]["bytes"] == 56));
    assert!(entries.iter().any(|r| r["entry"]["constructor"] == "Dyn" && r["entry"]["layout"]["shape"]["value_bytes"] == 40));
    assert!(entries.iter().any(|r| r["type_name"] == "Array(Dyn)" && r["entry"]["object"]["element_stride"] == 40));
    assert!(entries.iter().any(|r| r["entry"]["constructor"] == "Record" && r["entry"]["layout"]["status"] == "compile_time" && r["entry"]["layout"]["reason"].as_str().is_some_and(|s| s.contains("module body"))));
    assert!(entries.iter().any(|r| r["entry"]["constructor"] == "Quantified" && r["entry"]["layout"]["status"] == "known"));
    assert!(entries.iter().any(|r| r["entry"]["constructor"] == "Function" && r["entry"]["layout"]["shape"]["table"] == "ClosureEnvTable"));
    assert!(entries.iter().any(|r| r["type_name"] == "Array(Never)" && r["entry"]["object"]["element_stride"] == 0));
    assert!(entries.iter().any(|r| r["entry"]["constructor"] == "Native" && r["entry"]["object"]["bytes"] == 8));
    let dict = entries.iter().find(|r| r["type_name"] == "Dict(Int)").unwrap();
    assert_eq!(dict["entry"]["object"]["element_stride"], 24);
    assert_eq!(dict["entry"]["layout"]["shape"]["value_bytes"], 32);
    assert_eq!(dict["entry"]["layout"]["shape"]["table"], "ArrayTable");
    for row in entries {
        if matches!(row["entry"]["constructor"].as_str(), Some("Meta" | "Namespace" | "TypeList" | "PropertyBound" | "TypeFunction" | "Bound")) {
            assert_eq!(row["entry"]["layout"]["status"], "compile_time");
        }
        if row["entry"]["layout"]["status"] == "template" {
            assert!(row["entry"]["layout"]["reason"].as_str().unwrap().contains("free type parameter"));
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}
