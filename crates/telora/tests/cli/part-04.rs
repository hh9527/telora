#[test]
fn eval_reads_a_value_export_without_an_entry() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/pure.telora"),
        r#"import "std/value" {Value};
export def answer: Value = 'Object({"kind": 'String("pure"), "value": 'Int(42)});"#,
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
    args: 'True,
};
export def evaluate = entry.main(config, fn(ctx) {
    let env_ok = match dict.get(ctx.env, "TELORA_EVAL_TEST") {
        'Some(value) => value == "visible",
        'None => 'False,
    };
    let args_ok = array.length(ctx.args) == 2 && array.get(ctx.args, 1) == 'Some("two");
    if env_ok && args_ok {
        match dict.get(ctx.sources, "request") {
            'Some(value) => value,
            'None => fail!("missing request"),
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
    assert!(!stderr.contains(input.to_string_lossy().as_ref()), "{stderr}");
}

#[test]
fn eval_contracts_require_value_results() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/pure.telora"),
        r#"import "std/value" {Value};
export def raw = 42;
export def wrong: Fn(Int) -> Value = fn(value) { 'Int(value) };"#,
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

#[test]
fn language_acceptance_fixtures_pass() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(repository.join("scripts/test-language.sh"))
        .env("TELORA_BIN", env!("CARGO_BIN_EXE_telora"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn run_selector_uses_the_manifest_discovery_start() {
    let cwd = fixture();
    let other = fixture();
    fs::write(
        other.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/entry" as entry;
type State = struct {};
def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
export def run = entry.run(config, entry.no_ees, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (state, [actor.reply(request.id, 'Int(9))]),
            'EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    ({}, reduce)
});"#,
    )
    .unwrap();
    refresh_fixture_workspace(&other);
    let run = telora(&cwd)
        .args(["-C", other.to_str().unwrap(), "run", "@src/app:run"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "9");
}

#[test]
fn check_and_query_context_select_the_manifest_discovery_start() {
    let cwd = fixture();
    let other = fixture();
    fs::write(
        other.join("src/lib.telora"),
        "type Answer = Int; export {Answer};",
    )
    .unwrap();
    refresh_fixture_workspace(&other);

    let check = telora(&cwd)
        .args(["-C", other.to_str().unwrap(), "check", "@src/lib"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );

    let show = telora(&cwd)
        .args([
            "-C",
            other.to_str().unwrap(),
            "query",
            "exports",
            "@src/lib",
        ])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let records = jsonl(&show.stdout);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["name"], "Answer");

    let postfix = telora(&cwd)
        .args(["check", "@src/lib", "-C", other.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!postfix.status.success());
    assert!(String::from_utf8_lossy(&postfix.stderr).contains("unexpected argument '-C'"));

    let duplicate = telora(&cwd)
        .args([
            "-C",
            cwd.to_str().unwrap(),
            "-C",
            other.to_str().unwrap(),
            "check",
            "@src/lib",
        ])
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
}
