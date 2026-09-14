use super::*;

#[test]
fn eval_writes_contextual_debug_as_stderr_jsonl() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/debug.telora"),
        r#"import "std/value" {Value};
def var = 3;
def observed = var.dbg!("observed");
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
fn eval_with_supplies_declared_sources_env_and_trailing_args() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/pure.telora"),
        r#"import "std/array" as array;
import "std/dict" as dict;
import "std/entry" as entry;
import "std/value" {Value};
def config: entry.ContextConfig = {
    sources: ["request"],
    envs: ["TELORA_EVAL_TEST"],
    args: True,
};
export def evaluate = entry.main(config, fn(ctx) {
    let env_ok = match dict.get(ctx.env, "TELORA_EVAL_TEST") {
        Some(value) => value == "visible",
        None => False,
    };
    let args_ok = array.length(ctx.args) == 2 && array.get(ctx.args, 1) == Some("two");
    if env_ok && args_ok {
        match dict.get(ctx.sources, "request") {
            Some(value) => value,
            None => fail!("missing request"),
        }
    } else {
        fail!("invalid eval context", ctx)
    }
});"#,
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);
    let input = cwd.join("request.json");
    fs::write(&input, r#"{"accepted":true}"#).unwrap();

    let output = telora(&cwd)
        .args([
            "eval-with",
            "@src/pure:evaluate",
            "--source",
            &format!("request={}", input.display()),
            "--",
            "one",
            "two",
        ])
        .env("TELORA_EVAL_TEST", "visible")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!({"accepted": true})
    );

    fs::write(&input, r#"{"broken": }"#).unwrap();
    let invalid = telora(&cwd)
        .args([
            "eval-with",
            "@src/pure:evaluate",
            "--source",
            &format!("request={}", input.display()),
        ])
        .env("TELORA_EVAL_TEST", "visible")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(!invalid.status.success());
    assert!(stderr.contains("@eval-ctx/request"), "{stderr}");
    // No physical filename may leak, regardless of separators or JSON escaping.
    assert!(!stderr.contains(input.file_name().unwrap().to_str().unwrap()), "{stderr}");
}

#[test]
fn eval_contracts_require_value_results() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/pure.telora"),
        r#"import "std/value" {Value};
export def raw = 42;
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
    assert!(records[0]["message"].as_str().unwrap().contains("expected Value"));

    let function = telora(&cwd)
        .args(["eval-with", "@src/pure:wrong"])
        .output()
        .unwrap();
    assert!(!function.status.success());
    let records = jsonl(&function.stderr);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["schema"], "telora.error/v1");
    assert_eq!(records[0]["record"], "error");
    assert!(records[0]["message"].as_str().unwrap().contains("expected Eval"));
}
