use super::*;

#[test]
fn source_service_processes_many_events_and_discards_output_on_protocol_failure() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/app.telora"),
        include_str!("../../../../../tests/runtime/service-entry.telora"),
    )
    .unwrap();
    let mut child = telora(&cwd)
        .args(["serve", "@src/app:serve", "--bind", "stdio://"])
        .env("TELORA_WASM_TIMINGS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("null\n".repeat(200).as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies = jsonl(&output.stdout);
    assert_eq!(replies.len(), 200);
    assert_eq!(replies[199]["ok"], 200);
    {
        let mut command = telora(&cwd);
        command.args(["serve", "@src/app:serve", "--bind", "stdio://"]);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"null\n{broken\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(
            output.stdout.is_empty(),
            "must not publish the first response before terminal success"
        );
    }
    for command in ["run", "serve"] {
        let output = telora(&cwd).args([command, "--help"]).output().unwrap();
        assert!(!String::from_utf8_lossy(&output.stdout).contains("--native"));
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn source_services_keep_state_across_collection_and_recover_language_failures() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/app.telora"),
        include_str!("../../../../../tests/runtime/service-entry.telora"),
    )
    .unwrap();
    {
        let mut command = telora(&cwd);
        command.args(["run", "@src/app:run"]);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::json!(1)
        );
        let mut command = telora(&cwd);
        command.args(["serve", "@src/app:serve", "--bind", "stdio://"]);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"null\n\"fail\"\nnull\nnull\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let replies = jsonl(&output.stdout);
        assert_eq!(replies.len(), 4);
        assert_eq!(replies[0]["ok"], 1);
        assert_eq!(replies[1]["error"], true);
        assert_eq!(
            replies[1]["diagnostics"][0]["message"],
            "requested service failure"
        );
        assert_eq!(replies[2]["ok"], 2);
        assert_eq!(replies[3]["ok"], 3);
    }
    fs::remove_dir_all(cwd).unwrap();
}
