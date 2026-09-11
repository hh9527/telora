fn test_command(cwd: &Path, name: &str) -> std::process::Output {
    telora(cwd).args(["test", name]).output().unwrap()
}

#[test]
fn test_command_composes_top_level_and_nested_modules_without_running_unreachable_tests() {
    let cwd = fixture();
    fs::create_dir_all(cwd.join("tests/helpers")).unwrap();
    for (path, source) in [
        ("src/model.telora", "export def value = 40;"),
        ("tests/t2.telora", "export def value = 2;"),
        (
            "tests/helpers/common.telora",
            "import \"../t2\" as t2; import \"std/test\" as test; export def value = t2.value; export def check = test.should_ok(fn() { value });",
        ),
        ("tests/broken.telora", "export def broken = ;"),
        (
            "tests/failing.telora",
            "export def broken = fail!(\"unreachable failure\");",
        ),
        (
            "tests/t1.telora",
            r#"import "std/test" as test;
import "@src/model" as model;
import "./helpers/common" as common;
import "fixture/tests/t2" as t2;
export def check = test.should_ok(fn() -> Bool { if model.value + common.value == 42 && t2.value == 2 { True } else { fail!("wrong answer") } });
export def ordinary_false: Bool = False;
export def not_invoked: Fn() -> Int = fn() { fail!("must not invoke exports") };
"#,
        ),
    ] {
        fs::write(cwd.join(path), source).unwrap();
    }
    refresh_fixture_workspace(&cwd);
    let manifest = fs::read(cwd.join("telora-crate.json")).unwrap();
    let lock = fs::read(cwd.join("telora-lock.json")).unwrap();
    let output = test_command(&cwd, "t1");
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let records = jsonl(&output.stdout);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema"], "telora.test/v2");
    assert_eq!(records[0]["module"], "fixture/tests/t1");
    assert_eq!(records[0]["status"], "passed");
    assert_eq!(records[1]["status"], "ok");
    assert_eq!(fs::read(cwd.join("telora-crate.json")).unwrap(), manifest);
    assert_eq!(fs::read(cwd.join("telora-lock.json")).unwrap(), lock);
    let check = telora(&cwd).args(["check", "@test/t1"]).output().unwrap();
    assert!(check.status.success());
    assert_eq!(
        jsonl(&check.stdout).last().unwrap()["schema"],
        "telora.check/v1"
    );
    assert!(test_command(&cwd, "helpers/common").status.success());
    for operation in ["at", "exports"] {
        let query = telora(&cwd)
            .args(["query", operation, "@test/helpers/common"])
            .output()
            .unwrap();
        assert!(
            query.status.success(),
            "{}",
            String::from_utf8_lossy(&query.stdout)
        );
        assert!(
            jsonl(&query.stdout)
                .iter()
                .any(|record| record["name"] == "value")
        );
    }
    let catalog = telora(&cwd).args(["query", "modules"]).output().unwrap();
    assert!(catalog.status.success());
    assert!(
        jsonl(&catalog.stdout)
            .iter()
            .all(|record| !record["module"].as_str().unwrap().contains("/tests/"))
    );
    fs::write(
        cwd.join("tests/t1.telora"),
        "import \"./failing\" as failing; export def value = 1;",
    )
    .unwrap();
    assert_eq!(test_command(&cwd, "t1").status.code(), Some(1));
    fs::write(
        cwd.join("tests/t1.telora"),
        "import \"./broken\" as broken; export def value = 1;",
    )
    .unwrap();
    assert_eq!(test_command(&cwd, "t1").status.code(), Some(1));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn test_command_import_cycles_are_static_and_demand_cycles_fail() {
    let cwd = fixture();
    fs::create_dir_all(cwd.join("tests/helpers")).unwrap();
    for (a, b) in [
        ("./t1", ""),
        ("./t2", "fixture/tests/t1"),
        ("./helpers/a", ""),
    ] {
        fs::write(
            cwd.join("tests/t1.telora"),
            format!("import \"{a}\" as dep; import \"std/test\" as test; export def value = test.should_ok(fn() {{ 1 }});"),
        )
        .unwrap();
        fs::write(
            cwd.join("tests/t2.telora"),
            format!("import \"{b}\" as dep; export def value = 2;"),
        )
        .unwrap();
        fs::write(
            cwd.join("tests/helpers/a.telora"),
            "import \"./b\" as dep; export def value = 3;",
        )
        .unwrap();
        fs::write(
            cwd.join("tests/helpers/b.telora"),
            "import \"./a\" as dep; export def value = 4;",
        )
        .unwrap();
        let output = test_command(&cwd, "t1");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        let records = jsonl(&output.stdout);
        assert_eq!(records.last().unwrap()["status"], "ok");
    }
    fs::write(cwd.join("tests/t1.telora"), r#"
        import "std/test" as test;
        def cycle: Int = cycle;
        export def a_cycle = test.should_ok(fn() { cycle });
        export def healthy = test.should_ok(fn() { 42 });
    "#).unwrap();
    let output = test_command(&cwd, "t1");
    assert_eq!(output.status.code(), Some(1));
    let records = jsonl(&output.stdout);
    assert!(records.iter().any(|record| record["message"].as_str()
        .is_some_and(|message| message.contains("cyclic demand"))), "{records:?}");
    assert!(!records.iter().any(|record| record["record"] == "case"));
    assert_eq!(records.last().unwrap()["aborted"], true);
    assert_eq!(records.last().unwrap()["total"], 0);
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn test_command_static_data_and_diamond_imports_preserve_provenance_and_identity() {
    let cwd = fixture();
    fs::create_dir_all(cwd.join("tests/data")).unwrap();
    fs::write(cwd.join("tests/data/input.json"), "{\"n\": 7}").unwrap();
    fs::write(cwd.join("tests/data/input.yaml"), "n: 7\n").unwrap();
    fs::write(cwd.join("tests/data/input.toml"), "n = 7\n").unwrap();
    fs::write(
        cwd.join("tests/common.telora"),
        "export def value = dbg!(7, \"initialized once\");",
    )
    .unwrap();
    fs::write(
        cwd.join("tests/left.telora"),
        "import \"./common\" as common; export def value = common.value;",
    )
    .unwrap();
    fs::write(
        cwd.join("tests/right.telora"),
        "import \"@test/common\" as common; export def value = common.value;",
    )
    .unwrap();
    fs::write(cwd.join("tests/t1.telora"), r#"import "std/test" as test;
import "./left" as left;
import "./right" as right;
import "./data/input.json" { data as j };
import "./data/input.yaml" { data as y };
import "./data/input.toml" { data as t };
export def check = test.should_ok(fn() -> Bool { if j == y && y == t && left.value == right.value { True } else { fail!("mismatch", j) } });
"#).unwrap();
    let output = test_command(&cwd, "t1");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        jsonl(&output.stderr)
            .iter()
            .filter(|record| record["message"] == "initialized once")
            .count(),
        1
    );
    fs::write(cwd.join("tests/t1.telora"), "import \"std/test\" as test; import \"std/value\" {Value}; import \"./data/input.json\" { data }; export def check = test.should_ok(fn() { match data { Value.Object(fields) => fail!(\"bad input\", fields.n), _ => fail!(\"bad shape\") } });").unwrap();
    let output = test_command(&cwd, "t1");
    assert_eq!(output.status.code(), Some(1));
    let records = jsonl(&output.stdout);
    assert!(
        records
            .iter()
            .filter_map(|record| record["labels"].as_array())
            .flatten()
            .any(|label| label["source"] == "fixture/tests/data/input.json"),
        "{records:?}"
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn test_command_rejects_invalid_roots_and_source_to_test_imports() {
    let cwd = fixture();
    fs::write(cwd.join("tests/t1.telora"), "export def value = 1;").unwrap();
    fs::write(cwd.join("tests/_private.telora"), "export def value = 1;").unwrap();
    for name in [
        "../t1",
        "/t1",
        "t1.telora",
        "t1.json",
        "t1:run",
        "@test/t1",
        "a//b",
        "a/../t1",
        "a\\b",
        "*",
        "t?",
        "t[12]",
        "",
    ] {
        let output = test_command(&cwd, name);
        assert_eq!(output.status.code(), Some(2), "{name}");
        assert!(output.stdout.is_empty());
    }
    assert_eq!(
        telora(&cwd).arg("test").output().unwrap().status.code(),
        Some(2)
    );
    for name in ["missing", "_private"] {
        let output = test_command(&cwd, name);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(jsonl(&output.stderr)[0]["schema"], "telora.error/v1");
    }
    for target in ["@test/t1", "fixture/tests/t1"] {
        fs::write(
            cwd.join("src/lib.telora"),
            format!("import \"{target}\" as test; export def value = 1;"),
        )
        .unwrap();
        fs::write(
            cwd.join("tests/t1.telora"),
            "import \"@src/lib\" as lib; export def value = 1;",
        )
        .unwrap();
        let output = test_command(&cwd, "t1");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(jsonl(&output.stdout).last().unwrap()["status"], "error");
    }
    fs::write(cwd.join("tests/t1.telora"), "import \"std/test\" as test; def reject: Fn() -> Result(Int, String) = fn() { Err(\"notice\") }; def checked = reject().ok_or_warn!(); def unused: Fn() -> Int = fn() { fail!(\"unused closure must not be called\") }; export def value = test.should_ok(fn() { checked });").unwrap();
    let output = test_command(&cwd, "t1");
    assert!(output.status.success());
    assert!(
        jsonl(&output.stdout)
            .iter()
            .any(|record| record["severity"] == "warning")
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn test_command_uses_member_context_and_declared_dependencies() {
    let cwd = fixture();
    fs::create_dir_all(cwd.join("member/src")).unwrap();
    fs::create_dir_all(cwd.join("member/tests/nested")).unwrap();
    fs::create_dir_all(cwd.join("dep/src")).unwrap();
    fs::create_dir_all(cwd.join("dep/tests")).unwrap();
    fs::write(
        cwd.join("telora-config.json"),
        r#"{"version":1,"members":["member","dep"]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("member/telora-crate.json"),
        r#"{"name":"app","modules":["@src/lib"],"dependencies":["dep"]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("dep/telora-crate.json"),
        r#"{"name":"dep","modules":["@src/lib"],"dependencies":[]}"#,
    )
    .unwrap();
    fs::write(cwd.join("dep/src/lib.telora"), "export def value = 42;").unwrap();
    fs::write(
        cwd.join("dep/tests/broken.telora"),
        "not a valid module !!!",
    )
    .unwrap();
    fs::write(
        cwd.join("member/src/lib.telora"),
        "import \"dep/lib\" as dep; export def value = dep.value;",
    )
    .unwrap();
    fs::write(cwd.join("member/tests/nested/t1.telora"), "import \"std/test\" as test; import \"@src/lib\" as lib; export def check = test.should_ok(fn() -> Bool { if lib.value == 42 { True } else { fail!(\"wrong dependency\") } });").unwrap();
    let spec = telora_core::WorkspaceSpec::discover(&cwd).unwrap();
    let lock = spec
        .generate_lock(&std::collections::BTreeMap::new())
        .unwrap();
    spec.write_lock(&lock).unwrap();
    let output = telora(&cwd)
        .args(["-C", "member/src", "test", "nested/t1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        jsonl(&output.stdout).last().unwrap()["module"],
        "app/tests/nested/t1"
    );
    fs::write(
        cwd.join("member/tests/nested/t1.telora"),
        "import \"dep/tests/broken\" as dep; export def value = 1;",
    )
    .unwrap();
    let output = test_command(&cwd.join("member"), "nested/t1");
    assert_eq!(output.status.code(), Some(1));
    assert!(jsonl(&output.stdout).iter().any(|record| {
        record["message"]
            .as_str()
            .is_some_and(|message| message.contains("dep/tests/broken"))
    }));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn test_command_expands_fixture_factories_relative_to_their_declaring_module() {
    let cwd = fixture();
    fs::create_dir_all(cwd.join("tests/helpers")).unwrap();
    fs::write(cwd.join("tests/helpers/a.json"), "42").unwrap();
    fs::write(cwd.join("tests/helpers/b.yaml"), "42\n").unwrap();
    fs::write(cwd.join("tests/helpers/c.toml"), "n = 42\n").unwrap();
    fs::write(cwd.join("tests/helpers/group.telora"), r#"
        import "std/test" as test;
        export def group = test.with_fixtures(["a.json", "b.yaml", "c.toml"], fn(outer) {
            let warning: Option(Int) = warn!("group warning");
            test.with_fixtures(["a.json"], fn(inner) { test.should_ok(fn() { (outer, inner) }) })
        });
    "#).unwrap();
    fs::write(cwd.join("tests/main.telora"), "import \"./helpers/group\" {group}; export {group};").unwrap();
    let output = test_command(&cwd, "main");
    assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let records = jsonl(&output.stdout);
    let cases = records.iter().filter(|r| r["record"] == "case").collect::<Vec<_>>();
    assert_eq!(cases.len(), 3);
    assert_eq!(cases[2]["fixtures"], serde_json::json!([2, 0]));
    assert_eq!(cases[2]["sources"], serde_json::json!(["c.toml", "a.json"]));
    assert_eq!(records.iter().filter(|r| r["message"] == "group warning").count(), 3);
    fs::write(cwd.join("tests/helpers/b.yaml"), "[invalid").unwrap();
    let output = test_command(&cwd, "main");
    assert_eq!(output.status.code(), Some(1));
    let records = jsonl(&output.stdout);
    assert_eq!(records.last().unwrap()["passed"], 2);
    assert_eq!(records.last().unwrap()["failed"], 1);
    assert_eq!(records.last().unwrap()["aborted"], false);
    assert!(records.iter().filter_map(|r| r["labels"].as_array()).flatten().any(|label| {
        label["source"].as_str().is_some_and(|name| name.starts_with("@test-ctx/") && name.ends_with("/group/1"))
    }), "{records:?}");
    // Relative paths cannot leave the declaring crate, even through a helper.
    let outside = cwd.with_extension("json");
    fs::write(&outside, "42").unwrap();
    fs::write(cwd.join("tests/helpers/group.telora"), format!(r#"
        import "std/test" as test;
        export def group = test.with_fixtures(["../../../{}"], fn(value) {{ test.should_ok(fn() {{value}}) }});
    "#, outside.file_name().unwrap().to_str().unwrap())).unwrap();
    let output = test_command(&cwd, "main");
    assert_eq!(output.status.code(), Some(1));
    assert!(jsonl(&output.stdout).iter().any(|record| record["message"] == "fixture path escapes its declaring crate"));
    fs::remove_file(outside).unwrap();
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn test_command_reports_all_invalid_data_modules_before_executing_user_code() {
    let cwd = fixture();
    fs::write(cwd.join("tests/bad.json"), "{").unwrap();
    fs::write(cwd.join("tests/bad.yaml"), "[invalid").unwrap();
    fs::write(cwd.join("tests/main.telora"), r#"
        import "std/test" as test;
        import "./bad.json" {data as j};
        import "./bad.yaml" {data as y};
        export def case = dbg!(test.should_ok(fn() { (j, y) }), "must not initialize");
    "#).unwrap();
    let output = test_command(&cwd, "main");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    let records = jsonl(&output.stdout);
    assert_eq!(records.last().unwrap()["total"], 0);
    assert_eq!(records.last().unwrap()["aborted"], true);
    for expected in ["fixture/tests/bad.json", "fixture/tests/bad.yaml"] {
        assert!(records.iter().filter_map(|record| record["labels"].as_array()).flatten()
            .any(|label| label["source"] == expected), "{records:?}");
    }
    fs::remove_dir_all(cwd).unwrap();
}
