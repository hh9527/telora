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
export def check = test.should_ok(fn() { if model.value + common.value == 42 && t2.value == 2 { 'True } else { fail!("wrong answer") } });
export def ordinary_false = 'False;
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
fn test_command_cycles_fail_and_independent_errors_are_collected() {
    let cwd = fixture();
    fs::create_dir_all(cwd.join("tests/helpers")).unwrap();
    for (a, b) in [
        ("./t1", ""),
        ("./t2", "fixture/tests/t1"),
        ("./helpers/a", ""),
    ] {
        fs::write(
            cwd.join("tests/t1.telora"),
            format!("import \"{a}\" as dep; export def value = 1;"),
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
        assert_eq!(output.status.code(), Some(1));
        let records = jsonl(&output.stdout);
        assert_eq!(records.last().unwrap()["status"], "error");
        assert!(
            records.iter().any(|record| record["message"]
                .as_str()
                .is_some_and(|message| message.contains("module cycle"))),
            "{records:?}"
        );
    }
    fs::write(cwd.join("tests/t1.telora"), "export def first = fail!(\"first failure\"); export def second = fail!(\"second failure\"); export def healthy = 42;").unwrap();
    let output = test_command(&cwd, "t1");
    assert_eq!(output.status.code(), Some(1));
    let records = jsonl(&output.stdout);
    for expected in ["first failure", "second failure"] {
        assert!(
            records.iter().any(|record| record["message"]
                .as_str()
                .is_some_and(|message| message.contains(expected))),
            "{records:?}"
        );
    }
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
export def check = test.should_ok(fn() { if j == y && y == t && left.value == right.value { 'True } else { fail!("mismatch", j) } });
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
    fs::write(cwd.join("tests/t1.telora"), "import \"./data/input.json\" { data }; export def check = match data { 'Object(fields) => fail!(\"bad input\", fields.n), _ => fail!(\"bad shape\") };").unwrap();
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
    fs::write(cwd.join("tests/t1.telora"), "import \"std/test\" as test; def reject: Fn() -> Result(Int, String) = fn() { 'Err(\"notice\") }; def checked = reject.should_ok!(); export def value = test.should_ok(fn() { 1 });").unwrap();
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
    fs::write(cwd.join("member/tests/nested/t1.telora"), "import \"std/test\" as test; import \"@src/lib\" as lib; export def check = test.should_ok(fn() { if lib.value == 42 { 'True } else { fail!(\"wrong dependency\") } });").unwrap();
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
            .is_some_and(|message| message.contains("invalid module import"))
    }));
    fs::remove_dir_all(cwd).unwrap();
}
