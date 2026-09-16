use super::*;

#[test]
fn batch_type_diagnostics_identify_the_producing_module() {
    let cwd = fixture();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/language/src/check/diag-shared-contract");
    let destination = cwd.join("src/check/diag-shared-contract");
    fs::create_dir_all(&destination).unwrap();
    for name in ["shared", "good", "bad", "testee"] {
        fs::copy(source.join(format!("{name}.telora")), destination.join(format!("{name}.telora"))).unwrap();
    }
    let output = telora(&cwd).args(["check", "--lib", "--only-types"]).output().unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let diagnostics = text.lines().map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["record"] == "diagnostic").collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 2, "{text}");
    for diagnostic in diagnostics {
        assert_eq!(diagnostic["module"], "fixture/check/diag-shared-contract/bad");
        assert_eq!(diagnostic["session"], "--lib");
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn check_batch_roots_share_a_graph_and_obey_phase_boundaries() {
    let cwd = fixture();
    fs::create_dir_all(cwd.join("tests/nested")).unwrap();
    fs::write(cwd.join("src/main.telora"), "export def answer: Int = 42;").unwrap();
    fs::write(cwd.join("src/_private.telora"), "export def unused: Int = 1 / 0;").unwrap();
    fs::write(cwd.join("tests/nested/helper.telora"), "export def answer: Int = 42;").unwrap();
    fs::write(cwd.join("tests/main.telora"), "import \"./nested/helper\" {answer}; export def result: Int = answer;").unwrap();
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
        ("def unused: Int = 1 / 0; export def answer: Int = 42;", Some("division")),
        ("def unused: Fn() -> Int = fn() { fail!(\"not called\") }; export def answer: Int = 42;", None),
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
    ] {
        let body = format!("match query({queried_type}.type, Mark.type) {{ Some(property) => Value.Int(property.value), None => Value.Int(42) }}");
        let entry = if mode == "eval" {
            format!("do {{ {body} }}")
        } else {
            format!("main({{ sources: [], envs: [], args: False }}, fn(ctx) {{ {body} }})")
        };
        let entry_type = if mode == "eval" { "Value" } else { "Eval" };
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
            import "std/type-property" {{ get_type_prop as query }};
            @property(PropertyTarget.Type) type Mark = struct {{ value: Int }};
            def config: Int = 42;
            def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) {{ {provider} }};
            @mark type Item = struct {{ value: Int }};
            export def answer: {entry_type} = {entry};
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
        export def answer: Value = Value.Int(if Item.type == Alias.type { 42 } else { 0 });
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
        export def answer: Value = Value.Int(if True { recurse(3) } else { unused });
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
        export def answer: Value = Value.Int(a);
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
        export def answer: Value = Value.Object({ "j": j, "again": again, "y": y, "t": t });
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
        import "std/transform-service" as service;
        import "std/value" {Value};
        import "./input.json" { data };
        type MainService = struct {data: Value};
        impl service.TransformService for MainService {
            init: fn(ctx) { {data}.ty!(Self) },
            transform: fn(self, input) { self.data },
        };
        export {MainService};
    "#,
    )
    .unwrap();
    let output = execute_value(&cwd, "run", "@src/main");
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
        .args(["serve", "@src/main", "--bind", "stdio://"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("input.json"));
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
        export def answer: Value = Value.Object({
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
        "export def answer: Int = 1 / 0;",
        "import \"std/value\" { Value as Original }; type Value = enum { Int(Int) }; export def answer: Value = Value.Int(42);",
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
        "import \"std/value\" { Value }; export def answer: Value = Value.Int(1 / 0);",
        "import \"std/value\" { Value }; export def answer: Value = Value.LocalDate(\"2026-09-10\");",
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
            export def answer: String = model.name({value});
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
export def answer: Int = 1 / 0;
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
    assert_eq!(export("bad")["state"], "Known");
    assert_eq!(export("bad")["type"], "Int");
    assert_eq!(export("bad")["failed_constraints"].as_array().unwrap().len(), 1);
    assert_eq!(export("answer")["failed_constraints"], serde_json::json!([]));
    let failed = export("bad")["failed_constraints"][0].clone();
    assert!(records.iter().any(|record| record["record"] == "diagnostic"
        && record["constraint_ids"].as_array().is_some_and(|ids| ids.contains(&failed))));
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
        export def unevaluated: Int = 1 / 0;
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
        "export def inc: Fn(Int) -> Int = fn(x) { x + 1 };",
    )
    .unwrap();
    fs::write(
        cwd.join("src/main.telora"),
        "import \"@src/math\" { inc };\nexport def answer: Int = inc(41);\n",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["query", "at", "@src/main:2:25"])
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
        "import \"@src/math\" { inc }; export def answer: Int = inc(1);",
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
        export def metadata: TypeOf(Int) = Int.type;
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
        export def wrapped: Wrapped = Wrapped(1);
        export def factory: Fn(Int) -> Fn(Int) -> Int = fn(x: Int) { fn(y: Int) { x + y } };
        export def poly: for(T) Fn(T) -> T = fn(x) { x };
        export def inferred_identity: for(T) Fn(T) -> T = fn(x) { x };
        export def same_poly: Bool = poly@[Int] == poly@[Int];
        export def unchecked: Fn(Unchecked(Rec)) -> Int = fn(x) { x.item };
        export def empty: Array(Never) = [];
        export def dictionary: Dict(Int) = {x: 1};
        export def accepts_dyn: Fn(Dyn) -> Dyn = fn(value: Dyn) { value };
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
    assert!(!entries.iter().any(|r| r["entry"]["constructor"] == "Quantified"));
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
