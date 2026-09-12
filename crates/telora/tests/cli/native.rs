#[test]
fn native_check_obeys_phase_boundaries_and_preserves_failure_location() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-native/tests/fixtures/generic-initialization.telora"),
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["check", "--native", "--lib"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(
        cwd.join("src/main.telora"),
        "def unused: Int = fail!(\"native initializer failed\"); export def answer = 42;",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["check", "--native", "--lib", "--only-types"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = telora(&cwd)
        .args(["check", "--native", "--lib"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let records = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let failures = records
        .iter()
        .filter(|record| record["message"] == "native initializer failed")
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 1, "{records:?}");
    assert_eq!(failures[0]["labels"][0]["location"]["line"], 1);
    let help = telora(&cwd).args(["check", "--help"]).output().unwrap();
    assert!(!String::from_utf8(help.stdout).unwrap().contains("--native"));
    let help = telora(&cwd).args(["eval", "--help"]).output().unwrap();
    assert!(!String::from_utf8(help.stdout).unwrap().contains("--native"));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_check_injects_data_before_initialization() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        "import \"./input.json\" as input; export def answer = input.data;",
    )
    .unwrap();
    fs::write(cwd.join("src/input.json"), "{\"answer\":42}").unwrap();
    let output = telora(&cwd)
        .args(["check", "--native", "@src/main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = telora(&cwd)
        .args(["eval", "--native", "@src/main:answer"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!({"answer":42})
    );
    fs::write(
        cwd.join("src/main.telora"),
        "import \"std/value\" {Value}; export def answer = Value.Int(42);",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["eval", "--native", "@src/main:answer"])
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
    fs::remove_dir_all(cwd).unwrap();
}
