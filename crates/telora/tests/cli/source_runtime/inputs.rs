use super::*;

#[test]
fn source_data_depth_limit_applies_before_materialization() {
    let cwd = fixture();
    let input = format!("{}0{}", "[".repeat(256), "]".repeat(256));
    fs::write(cwd.join("src/deep.json"), &input).unwrap();
    fs::write(
        cwd.join("src/main.telora"),
        "import \"./deep.json\" as data; export def answer = data.data;",
    )
    .unwrap();
    let output = telora(&cwd).args(["check", "@src/main"]).output().unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("depth"), "{stdout}");
    fs::write(cwd.join("src/main.telora"), "import \"std/entry\" as entry; import \"std/value\" {Value}; export def answer = entry.main({sources: [\"input\"], envs: [], args: False}, fn(ctx) { Value.Int(42) });").unwrap();
    let output = telora(&cwd)
        .args([
            "eval-with",
            "@src/main:answer",
            "--source",
            "input=src/deep.json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("depth"));
    assert!(output.stdout.is_empty());
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_eval_with_rejects_invalid_config_and_preserves_execution_diagnostics() {
    let cwd = fixture();
    for (source, extra, message) in [
        (
            "import \"std/entry\" as entry; import \"std/value\" {Value}; export def answer = entry.main({sources: [], envs: [], args: False}, fn(ctx) { Value.Int(42) });",
            vec!["--", "unexpected"],
            "does not accept command-line arguments",
        ),
        (
            "import \"std/entry\" as entry; import \"std/value\" {Value}; export def answer = entry.main({sources: [\"dup\", \"dup\"], envs: [], args: False}, fn(ctx) { Value.Int(42) });",
            vec![],
            "unique non-empty names",
        ),
        (
            "import \"std/entry\" as entry; import \"std/value\" {Value}; export def answer = entry.main({sources: [], envs: [\"TELORA_NATIVE_MISSING_ENV\"], args: False}, fn(ctx) { Value.Int(42) });",
            vec![],
            "cannot read declared environment variable",
        ),
        (
            "import \"std/entry\" as entry; export def answer = entry.main({sources: [], envs: [], args: False}, fn(ctx) {\n fail!(\"observed entry failed\");\n});",
            vec![],
            "observed entry failed",
        ),
        ("export def answer = 42;", vec![], "expected Eval"),
    ] {
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        let output = telora(&cwd)
            .env_remove("TELORA_NATIVE_MISSING_ENV")
            .args(["eval-with", "@src/main:answer"])
            .args(extra)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message), "{stderr}");
        if message == "observed entry failed" {
            assert!(
                stderr.contains("main:2:") || stderr.contains("main.telora:2:"),
                "{stderr}"
            );
        }
        assert!(output.stdout.is_empty());
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_eval_with_initializes_then_injects_declared_context() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../../../tests/runtime/eval-with.telora"),
    )
    .unwrap();
    fs::write(cwd.join("src/base.json"), "{\"loaded\":true}").unwrap();
    fs::write(cwd.join("input.json"), "{\"answer\":42}").unwrap();
    let formats = telora(&cwd)
        .args(["eval-with", "@src/main:formats"])
        .output()
        .unwrap();
    assert!(
        formats.status.success(),
        "{}",
        String::from_utf8_lossy(&formats.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&formats.stdout).unwrap(),
        serde_json::json!([{ "answer": 42 }, { "answer": 43 }, { "answer": 44 }])
    );
    let output = telora(&cwd)
        .env("TELORA_NATIVE_TEST_ENV", "selected")
        .args([
            "eval-with",
            "@src/main:answer",
            "--source",
            "input=input.json",
            "--",
            "hello",
            "中",
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
        serde_json::json!([
            {"args":["hello","中"], "env":{"TELORA_NATIVE_TEST_ENV":"selected"}, "sources":{"input":{"answer":42}}},
            {"loaded":true}
        ])
    );
    let selected = telora(&cwd)
        .env("TELORA_WASM_TIMINGS", "1")
        .args([
            "eval-with",
            "@src/main:selected",
            "--source",
            "input=input.json",
        ])
        .output()
        .unwrap();
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&selected.stdout).unwrap(),
        serde_json::json!({"answer":42})
    );
    let phases = String::from_utf8_lossy(&selected.stderr)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        phases
            .iter()
            .map(|phase| phase["wasm_phase"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "frontend",
            "codegen_link",
            "engine_load",
            "data_input",
            "initialize",
            "entry_input",
            "entry_output"
        ]
    );
    assert!(
        phases
            .iter()
            .all(|phase| phase["elapsed_ns"].as_u64().is_some())
    );
    let decoded = telora(&cwd)
        .args([
            "eval-with",
            "@src/main:decoded",
            "--source",
            "input=input.json",
        ])
        .output()
        .unwrap();
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&decoded.stdout).unwrap(),
        serde_json::json!({"answer":42})
    );
    fs::write(cwd.join("renamed.json"), "{\"answerValue\":42}").unwrap();
    let renamed = telora(&cwd)
        .args([
            "eval-with",
            "@src/main:property_decoded",
            "--source",
            "input=renamed.json",
        ])
        .output()
        .unwrap();
    assert!(
        renamed.status.success(),
        "{}",
        String::from_utf8_lossy(&renamed.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&renamed.stdout).unwrap(),
        serde_json::json!({"answerValue":42})
    );
    fs::write(cwd.join("text.json"), "\"localhost:42\"").unwrap();
    let text = telora(&cwd)
        .args([
            "eval-with",
            "@src/main:text_decoded",
            "--source",
            "input=text.json",
        ])
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&text.stdout).unwrap(),
        serde_json::json!("localhost:42")
    );
    fs::write(cwd.join("invalid.json"), "{\"answer\":\"wrong\"}").unwrap();
    let rejected = telora(&cwd)
        .args([
            "eval-with",
            "@src/main:decoded",
            "--source",
            "input=invalid.json",
        ])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&rejected.stderr);
    assert!(diagnostic.contains("expected Int"), "{diagnostic}");
    assert!(diagnostic.contains("@eval-ctx/input:1:11"), "{diagnostic}");
    assert!(diagnostic.contains("fixture/main:"), "{diagnostic}");
    fs::write(cwd.join("rejected.json"), "{\"answer\":0}").unwrap();
    let rejected = telora(&cwd)
        .args([
            "eval-with",
            "@src/main:decoded",
            "--source",
            "input=rejected.json",
        ])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        diagnostic.contains("positive input required"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("@eval-ctx/input:1:11"), "{diagnostic}");
    let output = telora(&cwd)
        .args(["eval-with", "@src/main:answer"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("eval sources do not match"));
    assert!(output.stdout.is_empty());
    let help = telora(&cwd).args(["eval-with", "--help"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&help.stdout).contains("--native"));
    fs::remove_dir_all(cwd).unwrap();
}
