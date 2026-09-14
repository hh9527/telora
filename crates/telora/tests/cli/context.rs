use super::*;

#[test]
fn run_selector_uses_the_manifest_discovery_start() {
    let cwd = fixture();
    let other = fixture();
    fs::write(
        other.join("src/app.telora"),
        r###"import "std/actor" as actor; import "std/value" {Value};
import "std/ees" as ees;
import "std/entry" as entry;
type State = struct {};
def config: entry.ContextConfig = {sources: [], envs: [], args: False};
export def run: entry.Run(State) = entry.run((State).type, config, ees.none, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            actor.Event.Request(request) => (state, [actor.reply(request.id, Value.Int(9))]),
            actor.Event.EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    ({}, reduce)
});"###,
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
