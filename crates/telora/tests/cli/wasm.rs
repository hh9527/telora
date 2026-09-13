use super::*;

#[test]
fn wasm_check_preserves_warning_error_and_subject_labels() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/check-rejection.telora"),
    )
    .unwrap();
    let mut results = vec![];
    for backend in [None, Some("--wasm")] {
        let mut command = telora(&cwd);
        command.args(["check", "@src/main"]).args(backend);
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let diagnostics = jsonl(&output.stdout)
            .into_iter()
            .filter(|v| v["record"] == "diagnostic")
            .collect::<Vec<_>>();
        assert_eq!(
            diagnostics.len(),
            2,
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(diagnostics[0]["severity"], "warning");
        assert_eq!(diagnostics[0]["message"], "checker initialized");
        assert_eq!(diagnostics[1]["message"], "positive required");
        assert_eq!(diagnostics[1]["labels"].as_array().unwrap().len(), 2);
        results.push(diagnostics);
    }
    assert_eq!(results[0], results[1]);
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_cli_initializes_data_and_runs_the_authoritative_eval_contract() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/entry.telora"),
    )
    .unwrap();
    fs::write(cwd.join("src/input.json"), r#"{"number":42}"#).unwrap();
    fs::write(cwd.join("source.yaml"), "items: [1, true, null]\n").unwrap();
    for backend in [None, Some("--wasm")] {
        let mut command = telora(&cwd);
        command.args(["eval", "@src/main:answer"]);
        command.args(backend);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::json!([{ "number":42 }, 42])
        );
        let mut command = telora(&cwd);
        command.env("TELORA_WASM_TEST_ENV", "env value").args([
            "eval-with",
            "@src/main:main",
            "--source",
            "input=source.yaml",
        ]);
        command.args(backend);
        let output = command.args(["--", "argument"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::json!({"loaded":{"number":42},"input":{"items":[1,true,null]},"arg":"argument","env":"env value"})
        );
    }
    for command in ["check", "eval", "eval-with"] {
        let output = telora(&cwd).args([command, "--help"]).output().unwrap();
        assert!(output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("--wasm"));
        let selector = if command == "check" {
            "@src/main"
        } else {
            "@src/main:answer"
        };
        let output = telora(&cwd)
            .args([command, "--native", "--wasm", selector])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    }
    fs::write(
        cwd.join("src/check.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/properties.telora"),
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["check", "--wasm", "@src/check"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = telora(&cwd)
        .args(["check", "--wasm", "--only-types", "--lib"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(cwd).unwrap();
}
