use super::*;

#[test]
fn wasm_artifact_runs_without_workspace_source_or_data_files() {
    let cwd = fixture();
    let deployment = cwd.with_extension("deployment");
    fs::create_dir(&deployment).unwrap();
    fs::write(
        cwd.join("src/bundle.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/bundle-data.telora"),
    )
    .unwrap();
    fs::write(
        cwd.join("src/input.yaml"),
        include_str!("../../../telora-wasm/tests/fixtures/bundle-input.yaml"),
    )
    .unwrap();
    fs::write(
        cwd.join("src/input.toml"),
        include_str!("../../../telora-wasm/tests/fixtures/bundle-input.toml"),
    )
    .unwrap();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/entry.telora"),
    )
    .unwrap();
    fs::write(
        cwd.join("src/input.json"),
        r#"{"number":42,"max":9223372036854775807}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/reject.telora"),
        include_str!("../../../telora-wasm/tests/fixtures/check-rejection.telora"),
    )
    .unwrap();
    for (selector, file) in [
        ("@src/bundle:answer", "bundle.wasm"),
        ("@src/main:answer", "value.wasm"),
        ("@src/main:main", "entry.wasm"),
        ("@src/reject:answer", "reject.wasm"),
    ] {
        let output = telora(&cwd)
            .args(["wasm", "build", selector, "-o"])
            .arg(deployment.join(file))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = telora(&cwd)
        .args(["wasm", "build", "@src/main:answer", "-o"])
        .arg(deployment.join("again.wasm"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(deployment.join("again.wasm")).unwrap(),
        fs::read(deployment.join("value.wasm")).unwrap()
    );
    fs::write(
        cwd.join("src/invalid.telora"),
        "export def answer = missing_binding;",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["wasm", "build", "@src/invalid:answer", "-o"])
        .arg(deployment.join("invalid.wasm"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!deployment.join("invalid.wasm").exists());
    fs::remove_dir_all(&cwd).unwrap();
    let output = telora(&deployment)
        .args(["wasm", "eval", "bundle.wasm"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!([true, true])
    );
    let output = telora(&deployment)
        .args(["wasm", "eval", "value.wasm"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!([{"number":42,"max":i64::MAX},42])
    );
    let output = telora(&deployment)
        .args(["wasm", "check", "entry.wasm"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    fs::write(deployment.join("request.yaml"), "items: [1, true, null]\n").unwrap();
    let output = telora(&deployment)
        .env("TELORA_WASM_TEST_ENV", "published env")
        .args([
            "wasm",
            "eval-with",
            "entry.wasm",
            "--source",
            "input=request.yaml",
            "--",
            "argument",
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
            "loaded":{"number":42,"max":i64::MAX},"input":{"items":[1,true,null]},"arg":"argument","env":"published env"
        })
    );
    let output = telora(&deployment)
        .args(["wasm", "check", "reject.wasm"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostics.contains("checker initialized"));
    assert!(diagnostics.contains("positive required"));
    assert!(diagnostics.contains("reject"));
    assert!(diagnostics.contains("subject originated here"));
    let output = telora(&deployment).arg("--help").output().unwrap();
    assert!(!String::from_utf8_lossy(&output.stdout).contains("wasm"));
    fs::remove_dir_all(deployment).unwrap();
}
