use super::*;

#[test]
fn run_selector_uses_the_manifest_discovery_start() {
    let cwd = fixture();
    let other = fixture();
    fs::write(
        other.join("src/app.telora"),
        runtime_source("bytes-literal.telora"),
    )
    .unwrap();
    refresh_fixture_workspace(&other);
    let run = telora(&cwd)
        .args([
            "-C",
            other.to_str().unwrap(),
            "serve",
            "@src/app",
            "--bind",
            "stdio+jsonl://",
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(run.stdout.is_empty());
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
