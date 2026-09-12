#[test]
fn native_debug_events_preserve_order_location_and_result() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/debug.telora")).unwrap();
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let native = telora(&cwd).args([command, "--native", &selector]).output().unwrap();
        let default = telora(&cwd).args([command, &selector]).output().unwrap();
        for output in [&native, &default] {
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
        assert_eq!(native.stdout, default.stdout);
        let events = |output: &[u8]| String::from_utf8_lossy(output).lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
        let actual = events(&native.stderr);
        assert_eq!(actual, events(&default.stderr));
        assert_eq!(actual.len(), if command == "eval" { 1 } else { 3 });
        assert_eq!(actual[0]["message"], "initialize");
        assert_eq!(actual[0]["line"], 4);
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_warnings_do_not_block_publication_or_entry_output() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/warnings.telora")).unwrap();
    let output = telora(&cwd).args(["check", "--native", "@src/main"]).output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let records = String::from_utf8(output.stdout).unwrap().lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
    let warnings = records.iter().filter(|record| record["severity"] == "warning").collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1, "{records:?}");
    assert_eq!(warnings[0]["message"], "initialization warning");
    assert_eq!(warnings[0]["labels"].as_array().unwrap().len(), 2);
    for (command, export, expected) in [("eval", "answer", 42), ("eval-with", "main", 43)] {
        let output = telora(&cwd).args([command, "--native", &format!("@src/main:{export}")]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), serde_json::json!(expected));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("initialization warning"), "{stderr}");
        if command == "eval-with" { assert!(stderr.contains("entry warning"), "{stderr}"); }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_json_schema_matches_default_for_closed_type_graphs() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/schema.telora")).unwrap();
    let native = telora(&cwd).args(["eval", "--native", "@src/main:answer"]).output().unwrap();
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let default = telora(&cwd).args(["eval", "@src/main:answer"]).output().unwrap();
    assert!(default.status.success(), "{}", String::from_utf8_lossy(&default.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&native.stdout).unwrap(), serde_json::from_slice::<Value>(&default.stdout).unwrap());
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_path_operations_match_default_and_survive_publication() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/path.telora")).unwrap();
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let native = telora(&cwd).args([command, "--native", &selector]).output().unwrap();
        assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
        let default = telora(&cwd).args([command, &selector]).output().unwrap();
        assert!(default.status.success(), "{}", String::from_utf8_lossy(&default.stderr));
        let value = serde_json::from_slice::<Value>(&native.stdout).unwrap();
        assert_eq!(value, serde_json::from_slice::<Value>(&default.stdout).unwrap());
        assert_eq!(value[1], serde_json::json!([".", "a/c", "/b/c", ".", "../../a"]));
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_hash_states_are_persistent_across_initialization_and_entry() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/hash.telora")).unwrap();
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let native = telora(&cwd).args([command, "--native", &selector]).output().unwrap();
        assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
        let default = telora(&cwd).args([command, &selector]).output().unwrap();
        assert!(default.status.success(), "{}", String::from_utf8_lossy(&default.stderr));
        let value = serde_json::from_slice::<Value>(&native.stdout).unwrap();
        assert_eq!(value, serde_json::from_slice::<Value>(&default.stdout).unwrap());
        assert_eq!(value[0], "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(value[1], "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(value.as_array().unwrap()[3..].iter().all(|value| value == true));
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_structural_equality_matches_default_across_worlds() {
    let cwd = fixture();
    for source in [
        include_str!("../../../telora-native/tests/fixtures/equality.telora"),
        include_str!("../../../telora-native/tests/fixtures/record-spread.telora"),
        include_str!("../../../telora-native/tests/fixtures/sequence-spread.telora"),
        include_str!("../../../telora-native/tests/fixtures/bytes-literal.telora"),
        include_str!("../../../telora-native/tests/fixtures/local-generics.telora"),
    ] {
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
            let selector = format!("@src/main:{export}");
            let native = telora(&cwd).args([command, "--native", &selector]).output().unwrap();
            assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
            let default = telora(&cwd).args([command, &selector]).output().unwrap();
            assert!(default.status.success(), "{}", String::from_utf8_lossy(&default.stderr));
            let value = serde_json::from_slice::<Value>(&native.stdout).unwrap();
            assert_eq!(value, serde_json::from_slice::<Value>(&default.stdout).unwrap());
            assert!(value.as_array().unwrap().iter().all(|value| value == true));
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_interpreter_preserves_adapter_identity_across_initialization_and_entry() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/interpreter.telora")).unwrap();
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let native = telora(&cwd).args([command, "--native", &selector]).output().unwrap();
        let default = telora(&cwd).args([command, &selector]).output().unwrap();
        for output in [&native, &default] {
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
        let events = String::from_utf8_lossy(&native.stderr).lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
        assert_eq!(events.len(), if command == "eval" { 1 } else { 2 });
        assert!(events.iter().all(|event| event["message"] == "operand"));
        let native = serde_json::from_slice::<Value>(&native.stdout).unwrap();
        let default = serde_json::from_slice::<Value>(&default.stdout).unwrap();
        assert_eq!(native, serde_json::json!(vec![true; 8]));
        // Legacy publication does not retain the interpreter memo table. Native
        // explicitly retains identity across this boundary (item zero).
        assert_eq!(&native.as_array().unwrap()[1..], &default.as_array().unwrap()[1..]);
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_checked_cast_preserves_payloads_and_sealed_identity() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/checked-cast.telora")).unwrap();
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let output = telora(&cwd).args([command, "--native", &format!("@src/main:{export}")]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let value = serde_json::from_slice::<Value>(&output.stdout).unwrap();
        assert_eq!(value, serde_json::json!(vec![true; 19]));
    }
    let native = telora(&cwd).args(["eval", "--native", "@src/main:cast_data"]).output().unwrap();
    let default = telora(&cwd).args(["eval", "@src/main:cast_data"]).output().unwrap();
    for output in [&native, &default] {
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    assert_eq!(serde_json::from_slice::<Value>(&native.stdout).unwrap(), serde_json::from_slice::<Value>(&default.stdout).unwrap());
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_test_descriptions_initialize_without_running_tests_or_fixtures() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/test-description.telora")).unwrap();
    let check = telora(&cwd).args(["check", "--native", "@src/main"]).output().unwrap();
    assert!(check.status.success(), "{} {}", String::from_utf8_lossy(&check.stdout), String::from_utf8_lossy(&check.stderr));
    for (command, export) in [("eval", "answer"), ("eval-with", "main")] {
        let selector = format!("@src/main:{export}");
        let native = telora(&cwd).args([command, "--native", &selector]).output().unwrap();
        assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
        assert_eq!(serde_json::from_slice::<Value>(&native.stdout).unwrap(), serde_json::json!([true, true]));
    }
    fs::write(cwd.join("src/main.telora"), "import \"std/test\" as test; export def invalid = test.should_fail_with(fn() {42}, \"\");").unwrap();
    let check = telora(&cwd).args(["check", "--native", "@src/main"]).output().unwrap();
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stdout).contains("nonempty expectation"));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_data_depth_limit_applies_before_materialization() {
    let cwd = fixture();
    let input = format!("{}0{}", "[".repeat(256), "]".repeat(256));
    fs::write(cwd.join("src/deep.json"), &input).unwrap();
    fs::write(cwd.join("src/main.telora"), "import \"./deep.json\" as data; export def answer = data.data;").unwrap();
    let output = telora(&cwd).args(["check", "--native", "@src/main"]).output().unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("depth"), "{stdout}");
    fs::write(cwd.join("src/main.telora"), "import \"std/entry\" as entry; import \"std/value\" {Value}; export def answer = entry.main({sources: [\"input\"], envs: [], args: False}, fn(ctx) { Value.Int(42) });").unwrap();
    let output = telora(&cwd).args(["eval-with", "--native", "@src/main:answer", "--source", "input=src/deep.json"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("depth"));
    assert!(output.stdout.is_empty());
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_eval_with_rejects_invalid_config_and_preserves_execution_diagnostics() {
    let cwd = fixture();
    for (source, extra, message) in [
        ("import \"std/entry\" as entry; import \"std/value\" {Value}; export def answer = entry.main({sources: [], envs: [], args: False}, fn(ctx) { Value.Int(42) });", vec!["--", "unexpected"], "does not accept command-line arguments"),
        ("import \"std/entry\" as entry; import \"std/value\" {Value}; export def answer = entry.main({sources: [\"dup\", \"dup\"], envs: [], args: False}, fn(ctx) { Value.Int(42) });", vec![], "unique non-empty names"),
        ("import \"std/entry\" as entry; import \"std/value\" {Value}; export def answer = entry.main({sources: [], envs: [\"TELORA_NATIVE_MISSING_ENV\"], args: False}, fn(ctx) { Value.Int(42) });", vec![], "cannot read declared environment variable"),
        ("import \"std/entry\" as entry; export def answer = entry.main({sources: [], envs: [], args: False}, fn(ctx) {\n fail!(\"native entry failed\");\n});", vec![], "native entry failed"),
        ("export def answer = 42;", vec![], "expected Eval"),
    ] {
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        let output = telora(&cwd).env_remove("TELORA_NATIVE_MISSING_ENV").args(["eval-with", "--native", "@src/main:answer"]).args(extra).output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message), "{stderr}");
        if message == "native entry failed" { assert!(stderr.contains("main:2:") || stderr.contains("main.telora:2:"), "{stderr}"); }
        assert!(output.stdout.is_empty());
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn native_eval_with_initializes_then_injects_declared_context() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/eval-with.telora")).unwrap();
    fs::write(cwd.join("src/base.json"), "{\"loaded\":true}").unwrap();
    fs::write(cwd.join("input.json"), "{\"answer\":42}").unwrap();
    let formats = telora(&cwd).args(["eval-with", "--native", "@src/main:formats"]).output().unwrap();
    assert!(formats.status.success(), "{}", String::from_utf8_lossy(&formats.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&formats.stdout).unwrap(), serde_json::json!([{ "answer": 42 }, { "answer": 43 }, { "answer": 44 }]));
    let schema = telora(&cwd).args(["eval-with", "--native", "@src/main:schema"]).output().unwrap();
    assert!(schema.status.success(), "{}", String::from_utf8_lossy(&schema.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&schema.stdout).unwrap(), serde_json::json!({"type":"string", "$schema":"https://json-schema.org/draft/2020-12/schema"}));
    let output = telora(&cwd).env("TELORA_NATIVE_TEST_ENV", "selected")
        .args(["eval-with", "--native", "@src/main:answer", "--source", "input=input.json", "--", "hello", "中"])
        .output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), serde_json::json!([
        {"args":["hello","中"], "env":{"TELORA_NATIVE_TEST_ENV":"selected"}, "sources":{"input":{"answer":42}}},
        {"loaded":true}
    ]));
    let selected = telora(&cwd).env("TELORA_NATIVE_TIMINGS", "1").args(["eval-with", "--native", "@src/main:selected", "--source", "input=input.json"]).output().unwrap();
    assert!(selected.status.success(), "{}", String::from_utf8_lossy(&selected.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&selected.stdout).unwrap(), serde_json::json!({"answer":42}));
    let phases = String::from_utf8_lossy(&selected.stderr).lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
    assert_eq!(phases.iter().map(|phase| phase["native_phase"].as_str().unwrap()).collect::<Vec<_>>(),
        ["frontend", "codegen", "runtime_setup", "initialize", "entry_input", "execute", "output"]);
    assert!(phases.iter().all(|phase| phase["elapsed_ns"].as_u64().is_some()));
    let decoded = telora(&cwd).args(["eval-with", "--native", "@src/main:decoded", "--source", "input=input.json"]).output().unwrap();
    assert!(decoded.status.success(), "{}", String::from_utf8_lossy(&decoded.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&decoded.stdout).unwrap(), serde_json::json!({"answer":42}));
    fs::write(cwd.join("renamed.json"), "{\"answerValue\":42}").unwrap();
    let renamed = telora(&cwd).args(["eval-with", "--native", "@src/main:property_decoded", "--source", "input=renamed.json"]).output().unwrap();
    assert!(renamed.status.success(), "{}", String::from_utf8_lossy(&renamed.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&renamed.stdout).unwrap(), serde_json::json!({"answerValue":42}));
    fs::write(cwd.join("text.json"), "\"localhost:42\"").unwrap();
    let text = telora(&cwd).args(["eval-with", "--native", "@src/main:text_decoded", "--source", "input=text.json"]).output().unwrap();
    assert!(text.status.success(), "{}", String::from_utf8_lossy(&text.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&text.stdout).unwrap(), serde_json::json!("localhost:42"));
    fs::write(cwd.join("invalid.json"), "{\"answer\":\"wrong\"}").unwrap();
    let rejected = telora(&cwd).args(["eval-with", "--native", "@src/main:decoded", "--source", "input=invalid.json"]).output().unwrap();
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&rejected.stderr);
    assert!(diagnostic.contains("expected Int"), "{diagnostic}");
    assert!(diagnostic.contains("@eval-ctx/input:1:11"), "{diagnostic}");
    assert!(diagnostic.contains("fixture/main:"), "{diagnostic}");
    fs::write(cwd.join("rejected.json"), "{\"answer\":0}").unwrap();
    let rejected = telora(&cwd).args(["eval-with", "--native", "@src/main:decoded", "--source", "input=rejected.json"]).output().unwrap();
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&rejected.stderr);
    assert!(diagnostic.contains("positive input required"), "{diagnostic}");
    assert!(diagnostic.contains("@eval-ctx/input:1:11"), "{diagnostic}");
    let output = telora(&cwd).args(["eval-with", "--native", "@src/main:answer"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("eval sources do not match"));
    assert!(output.stdout.is_empty());
    let help = telora(&cwd).args(["eval-with", "--help"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&help.stdout).contains("--native"));
    fs::remove_dir_all(cwd).unwrap();
}

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
    fs::write(
        cwd.join("src/main.telora"),
        include_str!("../../../telora-native/tests/fixtures/failure-subjects.telora"),
    )
    .unwrap();
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
#[test]
fn native_mutual_recursive_closures_survive_initialization_and_entry() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), include_str!("../../../telora-native/tests/fixtures/mutual-recursive-closures.telora")).unwrap();
    for (command, selector) in [("eval", "@src/main:answer"), ("eval-with", "@src/main:main")] {
        for native in [false, true] {
            let mut process = telora(&cwd);
            process.arg(command);
            if native { process.arg("--native"); }
            let result = process.arg(selector).output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            assert_eq!(serde_json::from_slice::<Value>(&result.stdout).unwrap(), serde_json::json!(42));
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}
