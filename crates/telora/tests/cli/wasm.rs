use super::*;

#[test]
fn wasm_reflection_and_display_properties_match_default_backend() {
    let cwd = fixture();
    for (name, source, expected) in [
        (
            "codec-rename",
            include_str!("../../../telora-wasm/tests/fixtures/codec-rename.telora"),
            serde_json::json!({"aValue":"text","zValue":7}),
        ),
        (
            "codec-newtype",
            include_str!("../../../telora-wasm/tests/fixtures/codec-newtype.telora"),
            serde_json::json!([1,2]),
        ),
        (
            "codec-enum",
            include_str!("../../../telora-wasm/tests/fixtures/codec-enum.telora"),
            serde_json::json!(["Empty",{"Child":{"Number":42}},"Empty"]),
        ),
        (
            "codec-collections",
            include_str!("../../../telora-wasm/tests/fixtures/codec-scalars.telora"),
            serde_json::json!(vec![true; 18]),
        ),
        (
            "json-parse",
            include_str!("../../../telora-wasm/tests/fixtures/json-parse.telora"),
            serde_json::json!({"ok": true}),
        ),
        (
            "json-stringify",
            include_str!("../../../telora-wasm/tests/fixtures/json-stringify.telora"),
            serde_json::json!("{\"a\":[null,true,false,-7,1,\"中\\n\\\"\"],\"z\":{}}"),
        ),
        (
            "hash",
            include_str!("../../../telora-wasm/tests/fixtures/hash.telora"),
            serde_json::json!(vec![true; 12]),
        ),
        (
            "string-parse-record",
            include_str!("../../../telora-wasm/tests/fixtures/string-parse-record.telora"),
            serde_json::json!(vec![true; 8]),
        ),
        (
            "regex-prepare",
            include_str!("../../../telora-wasm/tests/fixtures/regex-prepare.telora"),
            serde_json::json!([true, true, true, true, true]),
        ),
        (
            "string-parse",
            include_str!("../../../telora-wasm/tests/fixtures/string-parse.telora"),
            serde_json::json!(vec![true; 13]),
        ),
        (
            "regex",
            include_str!("../../../telora-wasm/tests/fixtures/regex.telora"),
            serde_json::json!(vec![true; 10]),
        ),
        (
            "format-equality",
            include_str!("../../../telora-wasm/tests/fixtures/format-equality.telora"),
            serde_json::json!(vec![true; 13]),
        ),
        (
            "dynamic-fields",
            include_str!("../../../telora-wasm/tests/fixtures/dynamic-fields.telora"),
            serde_json::json!(vec![true; 10]),
        ),
        (
            "dynamic-sequences",
            include_str!("../../../telora-wasm/tests/fixtures/dynamic-sequences.telora"),
            serde_json::json!(vec![true; 8]),
        ),
        (
            "dynamic-kind",
            include_str!("../../../telora-wasm/tests/fixtures/dynamic-kind.telora"),
            serde_json::json!(vec![true; 16]),
        ),
        (
            "dynamic-variants",
            include_str!("../../../telora-wasm/tests/fixtures/dynamic-variants.telora"),
            serde_json::json!(vec![true; 13]),
        ),
        (
            "reflection",
            include_str!("../../../telora-wasm/tests/fixtures/reflection.telora"),
            serde_json::json!(vec![true; 30]),
        ),
        (
            "display",
            include_str!("../../../telora-wasm/tests/fixtures/display-by.telora"),
            serde_json::json!([
                "localhost:8080",
                "localhost:8080",
                "api@localhost:8080 {ready} -0 api",
                "endpoint=explicit(localhost:8080)",
                "absent"
            ]),
        ),
    ] {
        fs::write(cwd.join(format!("src/{name}.telora")), source).unwrap();
        for backend in [None, Some("--wasm")] {
            let output = telora(&cwd)
                .args(["eval", &format!("@src/{name}:answer")])
                .args(backend)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{name} {backend:?}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                expected
            );
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_dynamic_projection_matches_default_backend() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/dynamic.telora"),
    )
    .unwrap();
    for backend in [None, Some("--native"), Some("--wasm")] {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .args(backend)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::json!(vec![true; 18])
        );
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_formatting_and_interpolation_match_default_backend() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/format.telora"),
    )
    .unwrap();
    let mut results = vec![];
    for backend in [None, Some("--wasm")] {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .args(backend)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        results.push(serde_json::from_slice::<Value>(&output.stdout).unwrap());
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(results[1][7], "n=42, f=3, s=ready");
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_equality_matches_default_backend() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/equality.telora"),
    )
    .unwrap();
    let mut results = vec![];
    for backend in [None, Some("--wasm")] {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .args(backend)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        results.push(serde_json::from_slice::<Value>(&output.stdout).unwrap());
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(results[1], serde_json::json!(vec![true; 41]));
    for (name, source) in [
        (
            "nonfinite",
            include_str!("../../../telora-wasm/tests/fixtures/nonfinite.telora"),
        ),
        (
            "overflow",
            include_str!("../../../telora-wasm/tests/fixtures/float-overflow.telora"),
        ),
    ] {
        fs::write(cwd.join(format!("src/{name}.telora")), source).unwrap();
        for backend in [None, Some("--wasm")] {
            let output = telora(&cwd)
                .args(["eval", &format!("@src/{name}:answer")])
                .args(backend)
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
fn wasm_record_updates_and_dictionary_spreads_match_default_backend() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/records.telora"),
    )
    .unwrap();
    let mut results = vec![];
    for backend in [None, Some("--wasm")] {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .args(backend)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        results.push(serde_json::from_slice::<Value>(&output.stdout).unwrap());
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(
        results[1],
        serde_json::json!({
            "projected":"source", "updated":[3,2,4,2], "original":1,
            "renamed":"generic", "replaced":"changed", "child":2,
            "merged":{"a":1,"b":3,"c":5}, "wide":{"a":[1,2],"b":[3],"z":[9]}
        })
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_sequence_spreads_match_default_backend() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/sequences.telora"),
    )
    .unwrap();
    let mut results = vec![];
    for backend in [None, Some("--wasm")] {
        let output = telora(&cwd)
            .args(["eval", "@src/main:value"])
            .args(backend)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        results.push(serde_json::from_slice::<Value>(&output.stdout).unwrap());
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(
        results[1],
        serde_json::json!({
            "numbers":[1,2], "items":[42,42,42], "appended":[1,2,3],
            "nominal":2, "metadata":42, "type_count":2
        })
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_path_operations_match_default_backend() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/path.telora"),
    )
    .unwrap();
    let mut results = vec![];
    for backend in [None, Some("--wasm")] {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .args(backend)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        results.push(serde_json::from_slice::<Value>(&output.stdout).unwrap());
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(
        results[1]["joins"],
        serde_json::json!([".", ".", "a/b", "b", "/root/b", "/", "../b", "目录/文件"])
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_string_operations_match_default_backend() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/string-value.telora"),
    )
    .unwrap();
    let mut results = vec![];
    for backend in [None, Some("--wasm")] {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .args(backend)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        results.push(serde_json::from_slice::<Value>(&output.stdout).unwrap());
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(results[1]["length"], 3);
    assert_eq!(results[1]["lines"], serde_json::json!(["a", "b", ""]));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn wasm_dict_operations_match_default_backend() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/dict-value.telora"),
    )
    .unwrap();
    let mut results = vec![];
    for backend in [None, Some("--wasm")] {
        let output = telora(&cwd)
            .args(["eval", "@src/main:answer"])
            .args(backend)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        results.push(serde_json::from_slice::<Value>(&output.stdout).unwrap());
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(
        results[1],
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
