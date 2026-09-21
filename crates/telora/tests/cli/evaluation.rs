use super::*;

#[test]
fn eval_writes_contextual_debug_as_stderr_jsonl() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/debug.telora"),
        r#"import "std/value" {Value};
def var: Int = 3;
def observed: Int = var.dbg!("observed");
export def answer: Value = Value.Int(observed);"#,
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);
    let run = telora(&cwd)
        .args(["eval", "@src/debug:answer"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "3");
    let records = jsonl(&run.stderr);
    assert_eq!(records.len(), 1, "finalization must not repeat dbg! events");
    for record in records {
        assert_eq!(record["name"], "var");
        assert_eq!(record["repr"], "3");
        assert_eq!(record["module"], "fixture/debug");
        assert_eq!(record["line"], 3);
        assert_eq!(record["message"], "observed");
    }
}

#[test]
fn eval_reads_a_value_export_without_an_entry() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/pure.telora"),
        r#"import "std/value" {Value};
export def answer: Value = Value.Object({"kind": Value.String("pure"), "value": Value.Int(42)});"#,
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);

    let output = telora(&cwd)
        .args(["eval", "@src/pure:answer"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!({"kind": "pure", "value": 42})
    );
}

#[test]
fn eval_contracts_require_value_results() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/pure.telora"),
        r#"import "std/value" {Value};
export def raw: Int = 42;
export def wrong: Fn(Int) -> Value = fn(value) { Value.Int(value) };"#,
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);

    let value = telora(&cwd)
        .args(["eval", "@src/pure:raw"])
        .output()
        .unwrap();
    assert!(!value.status.success());
    let records = jsonl(&value.stderr);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["schema"], "telora.error/v1");
    assert_eq!(records[0]["record"], "error");
    assert!(
        records[0]["message"]
            .as_str()
            .unwrap()
            .contains("expected Value")
    );
}
