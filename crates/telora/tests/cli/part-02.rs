#[test]
fn run_requires_an_entry_run_export() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/app.telora"),
        r#"import "std/value" {Value};
export def value: Value = 'Int(1);"#,
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);

    let output = telora(&cwd)
        .args(["run", "@src/app:value"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("expected Run(State)"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn run_context_admits_declared_sources_env_and_args() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/array" as array;
import "std/dict" as dict;
import "std/entry" as entry;

type State = struct {answer: Int};
def config: entry.ContextConfig = {
    sources: ["request"],
    envs: ["TELORA_CONTEXT_TEST"],
    args: 'True,
};
export def run = entry.run(config, entry.no_ees, fn(ctx) {
    let source_ok = match dict.get(ctx.sources, "request") {
        'Some('Int(value)) => value == 7,
        _ => 'False,
    };
    let env_ok = dict.get(ctx.env, "TELORA_CONTEXT_TEST") == 'Some("visible");
    let args_ok = array.length(ctx.args) == 1 && array.get(ctx.args, 0) == 'Some("arg");
    let initial: State = {answer: if source_ok && env_ok && args_ok { 42 } else { 0 }};
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (state, [actor.reply(request.id, 'Int(state.answer))]),
            'EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    (initial, reduce)
});"#,
    )
    .unwrap();
    let input = cwd.join("request.json");
    fs::write(&input, "7").unwrap();
    refresh_fixture_workspace(&cwd);

    let output = telora(&cwd)
        .args([
            "run",
            "@src/app:run",
            "--source",
            &format!("request={}", input.display()),
            "--",
            "arg",
        ])
        .env("TELORA_CONTEXT_TEST", "visible")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
}

#[test]
fn run_context_rejects_undeclared_inputs() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/entry" as entry;
type State = struct {};
def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
export def run = entry.run(config, entry.no_ees, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (state, [actor.reply(request.id, 'None)]),
            'EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    ({}, reduce)
});"#,
    )
    .unwrap();
    let input = cwd.join("request.json");
    fs::write(&input, "null").unwrap();
    refresh_fixture_workspace(&cwd);

    let source = telora(&cwd)
        .args([
            "run",
            "@src/app:run",
            "--source",
            &format!("request={}", input.display()),
        ])
        .output()
        .unwrap();
    assert!(!source.status.success());
    assert!(
        String::from_utf8_lossy(&source.stderr).contains("undeclared source"),
        "{}",
        String::from_utf8_lossy(&source.stderr)
    );

    let args = telora(&cwd)
        .args(["run", "@src/app:run", "--", "unexpected"])
        .output()
        .unwrap();
    assert!(!args.status.success());
    assert!(String::from_utf8_lossy(&args.stderr).contains("arguments"));
}

#[test]
fn serve_stdio_reuses_one_typed_state() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/entry" as entry;
type State = struct {next: Int};
def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
export def serve = entry.serve(config, entry.no_ees, fn(ctx) {
    let initial: State = {next: 1};
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (
                {next: state.next + 1},
                [actor.reply(request.id, 'Int(state.next))],
            ),
            'EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    (initial, reduce)
});"#,
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);
    let mut child = telora(&cwd)
        .args(["serve", "@src/app:serve", "--bind", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"null\nnull\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies = jsonl(&output.stdout);
    assert_eq!(replies[0]["ok"], 1);
    assert_eq!(replies[1]["ok"], 2);
}
