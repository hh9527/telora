use super::*;

#[test]
fn source_debug_events_preserve_order_location_and_result() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/debug.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let observed = telora(&cwd).args([command, &selector]).output().unwrap();
        {
            let output = &observed;
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let events = |output: &[u8]| {
            String::from_utf8_lossy(output)
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>()
        };
        let actual = events(&observed.stderr);
        assert_eq!(actual.len(), if command == "eval" { 1 } else { 3 });
        assert_eq!(actual[0]["message"], "initialize");
        assert_eq!(actual[0]["line"], 4);
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_warnings_do_not_block_publication_or_entry_output() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/warnings.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let output = telora(&cwd).args(["check", "@src/main"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let records = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let warnings = records
        .iter()
        .filter(|record| record["severity"] == "warning")
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1, "{records:?}");
    assert_eq!(warnings[0]["message"], "initialization warning");
    assert_eq!(warnings[0]["labels"].as_array().unwrap().len(), 2);
    for (command, export, expected) in [("eval", "answer", 42), ("eval-with", "main", 43)] {
        let output = telora(&cwd)
            .args([command, &format!("@src/main:{export}")])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::json!(expected)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr.contains("initialization warning"),
            command == "eval",
            "{stderr}"
        );
        if command == "eval-with" {
            assert!(stderr.contains("entry warning"), "{stderr}");
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_path_operations_survive_initialization() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/path.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let observed = telora(&cwd).args([command, &selector]).output().unwrap();
        assert!(
            observed.status.success(),
            "{}",
            String::from_utf8_lossy(&observed.stderr)
        );
        let value = serde_json::from_slice::<Value>(&observed.stdout).unwrap();
        assert_eq!(
            value[1],
            serde_json::json!([".", "a/c", "/b/c", ".", "../../a"])
        );
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_hash_states_are_persistent_across_initialization_and_entry() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/hash.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let observed = telora(&cwd).args([command, &selector]).output().unwrap();
        assert!(
            observed.status.success(),
            "{}",
            String::from_utf8_lossy(&observed.stderr)
        );
        let value = serde_json::from_slice::<Value>(&observed.stdout).unwrap();
        assert_eq!(
            value[0],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            value[1],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(
            value.as_array().unwrap()[3..]
                .iter()
                .all(|value| value == true)
        );
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_structural_equality_preserves_identity_across_worlds() {
    let cwd = fixture();
    for source in [
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/equality.telora"
        ))
        .expect("read test source"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/record-spread.telora"
        ))
        .expect("read test source"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/sequence-spread.telora"
        ))
        .expect("read test source"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/bytes-literal.telora"
        ))
        .expect("read test source"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/local-generics.telora"
        ))
        .expect("read test source"),
    ] {
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
            let selector = format!("@src/main:{export}");
            let observed = telora(&cwd).args([command, &selector]).output().unwrap();
            assert!(
                observed.status.success(),
                "{}",
                String::from_utf8_lossy(&observed.stderr)
            );
            let value = serde_json::from_slice::<Value>(&observed.stdout).unwrap();
            assert!(value.as_array().unwrap().iter().all(|value| value == true));
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_interpreter_preserves_adapter_identity_across_initialization_and_entry() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/interpreter.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let observed = telora(&cwd).args([command, &selector]).output().unwrap();
        {
            let output = &observed;
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let events = String::from_utf8_lossy(&observed.stderr)
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        // Only the selected export is evaluated; eval-with does not evaluate answer.
        assert_eq!(events.len(), 1);
        assert!(events.iter().all(|event| event["message"] == "operand"));
        let observed = serde_json::from_slice::<Value>(&observed.stdout).unwrap();
        assert_eq!(observed, serde_json::json!(vec![true; 8]));
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_test_descriptions_initialize_without_running_tests_or_fixtures() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/test-description.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let check = telora(&cwd).args(["check", "@src/main"]).output().unwrap();
    assert!(
        check.status.success(),
        "{} {}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let observed = telora(&cwd).args([command, &selector]).output().unwrap();
        assert!(
            observed.status.success(),
            "{}",
            String::from_utf8_lossy(&observed.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&observed.stdout).unwrap(),
            serde_json::json!([true, true])
        );
    }
    fs::write(
        cwd.join("src/main.telora"),
        "import \"std/test\" as test; export def invalid: test.Test = test.should_fail_with(fn() {42}, \"\");",
    )
    .unwrap();
    let check = telora(&cwd).args(["check", "@src/main"]).output().unwrap();
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stdout).contains("nonempty expectation"));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_mutual_recursive_closures_survive_initialization_and_entry() {
    let cwd = fixture();
    for source in [
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/mutual-recursive-closures.telora"
        ))
        .expect("read test source"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/generic-mutual-closures.telora"
        ))
        .expect("read test source"),
    ] {
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        for (command, selector) in [
            ("eval", "@src/main:answer"),
            ("eval-with", "@src/main:main"),
        ] {
            {
                let mut process = telora(&cwd);
                process.arg(command);
                let result = process.arg(selector).output().unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                assert_eq!(
                    serde_json::from_slice::<Value>(&result.stdout).unwrap(),
                    serde_json::json!(42)
                );
            }
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}
