use super::*;

#[test]
fn wasm_formatting_and_interpolation_produce_expected_values() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/format.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let result;
    {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        result = serde_json::from_slice::<Value>(&output.stdout).unwrap();
    }
    assert_eq!(result[7], "n=42, f=3, s=ready");
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_equality_produce_expected_values() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/equality.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let result;
    {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        result = serde_json::from_slice::<Value>(&output.stdout).unwrap();
    }
    assert_eq!(result, serde_json::json!(vec![true; 41]));
    for (name, source) in [
        (
            "nonfinite",
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/telora-wasm/tests/fixtures/nonfinite.telora"
            ))
            .expect("read test source"),
        ),
        (
            "overflow",
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/telora-wasm/tests/fixtures/float-overflow.telora"
            ))
            .expect("read test source"),
        ),
    ] {
        fs::write(cwd.join(format!("src/{name}.telora")), source).unwrap();
        {
            let output = telora(&cwd)
                .args(["eval", &format!("@src/{name}:answer")])
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("NonFiniteFloat"),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_record_updates_and_dictionary_spreads_produce_expected_values() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/records.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let result;
    {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        result = serde_json::from_slice::<Value>(&output.stdout).unwrap();
    }
    assert_eq!(
        result,
        serde_json::json!({
            "projected":"source", "updated":[3,2,4,2], "original":1,
            "renamed":"generic", "replaced":"changed", "child":2,
            "merged":{"a":1,"b":3,"c":5}, "wide":{"a":[1,2],"b":[3],"z":[9]}
        })
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_sequence_spreads_produce_expected_values() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/sequences.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let result;
    {
        let output = telora(&cwd)
            .args(["eval", "@src/main:value"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        result = serde_json::from_slice::<Value>(&output.stdout).unwrap();
    }
    assert_eq!(
        result,
        serde_json::json!({
            "numbers":[1,2], "items":[42,42,42], "appended":[1,2,3],
            "nominal":2, "metadata":42, "type_count":2
        })
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_string_operations_produce_expected_values() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/string-value.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let result;
    {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        result = serde_json::from_slice::<Value>(&output.stdout).unwrap();
    }
    assert_eq!(result["length"], 3);
    assert_eq!(result["lines"], serde_json::json!(["a", "b", ""]));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_dict_operations_produce_expected_values() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/dict-value.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let result;
    {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        result = serde_json::from_slice::<Value>(&output.stdout).unwrap();
    }
    assert_eq!(
        result,
        serde_json::json!({
            "keys":["a","m","z","é"], "merged":{"a":10,"b":20,"m":2,"z":3,"é":4},
            "filtered":{"z":3,"é":4}, "folded":1234,"missing":null
        })
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_check_preserves_warning_error_and_subject_labels() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/check-rejection.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    {
        let mut command = telora(&cwd);
        command.args(["check", "@src/main"]);
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
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_cli_initializes_data_and_runs_the_authoritative_eval_contract() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/entry.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    fs::write(cwd.join("src/input.json"), r#"{"number":42}"#).unwrap();
    fs::write(cwd.join("source.yaml"), "items: [1, true, null]\n").unwrap();
    {
        let mut command = telora(&cwd);
        command.args(["eval", "@src/main:answer"]);
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
            .args([command, "--wasm", selector])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument"));
    }
    fs::write(
        cwd.join("src/check.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/properties.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let output = telora(&cwd).args(["check", "@src/check"]).output().unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = telora(&cwd)
        .args(["check", "--only-types", "--lib"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(cwd).unwrap();
}
