use super::*;

#[test]
fn source_check_obeys_phase_boundaries_and_preserves_failure_location() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/generic-initialization.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let output = telora(&cwd).args(["check", "--lib"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(
        cwd.join("src/main.telora"),
        "def unused = fail!(\"observed initializer failed\"); export def answer = 42;",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["check", "--lib", "--only-types"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = telora(&cwd).args(["check", "--lib"]).output().unwrap();
    assert!(!output.status.success());
    let records = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let failures = records
        .iter()
        .filter(|record| record["message"] == "observed initializer failed")
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 1, "{records:?}");
    assert_eq!(failures[0]["labels"][0]["location"]["line"], 1);
    for (source, message, line) in [
        (
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/runtime/function-before-initialization.telora"
            ))
            .expect("read test source"),
            "before its declaration",
            3,
        ),
        (
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/runtime/function-alias-before-initialization.telora"
            ))
            .expect("read test source"),
            "uninitialized function",
            2,
        ),
    ] {
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        let output = telora(&cwd).args(["check", "--lib"]).output().unwrap();
        assert!(!output.status.success());
        let records = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let errors = records
            .iter()
            .filter(|record| record["severity"] == "error")
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{records:?}");
        assert!(
            errors[0]["message"].as_str().unwrap().contains(message),
            "{records:?}"
        );
        assert_eq!(errors[0]["labels"][0]["location"]["line"], line);
    }
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/failure-subjects.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let output = telora(&cwd).args(["check", "--lib"]).output().unwrap();
    assert!(!output.status.success());
    let records = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let diagnostic = records
        .iter()
        .find(|r| r["message"] == "computed message")
        .unwrap();
    assert_eq!(diagnostic["labels"].as_array().unwrap().len(), 3);
    assert_eq!(diagnostic["labels"][0]["location"]["line"], 4);
    assert_eq!(diagnostic["labels"][1]["location"]["line"], 2);
    assert_eq!(diagnostic["labels"][2]["location"]["line"], 3);
    let help = telora(&cwd).args(["check", "--help"]).output().unwrap();
    assert!(!String::from_utf8(help.stdout).unwrap().contains("--native"));
    let help = telora(&cwd).args(["eval", "--help"]).output().unwrap();
    assert!(!String::from_utf8(help.stdout).unwrap().contains("--native"));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_check_injects_data_before_initialization() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        "import \"./input.json\" as input; export def answer = input.data;",
    )
    .unwrap();
    fs::write(cwd.join("src/input.json"), "{\"answer\":42}").unwrap();
    let output = telora(&cwd).args(["check", "@src/main"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
        serde_json::json!({"answer":42})
    );
    fs::write(
        cwd.join("src/main.telora"),
        "import \"std/value\" {Value}; export def answer = Value.Int(42);",
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
    fs::remove_dir_all(cwd).unwrap();
}
