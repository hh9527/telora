#[test]
fn static_mir_eval_demands_globals_in_the_vm() {
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
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!(42)
    );
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
export def unknown = missing;
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
    assert_eq!(export("bad")["state"], "Conflicted");
    assert!(records.iter().any(|r| {
        r["record"] == "diagnostic"
            && r["message"]
                .as_str()
                .is_some_and(|m| m.contains("unresolved symbol"))
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
