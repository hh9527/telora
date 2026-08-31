#[test]
fn lock_removes_stale_imos_plans_without_remote_sources() {
    let cwd = fixture();
    let plans = cwd.join(".telora/crates-refs");
    fs::create_dir_all(&plans).unwrap();
    fs::write(plans.join("telora-stale.json"), "{}\n").unwrap();
    fs::write(plans.join("host-note.txt"), "keep\n").unwrap();

    let locked = telora(&cwd).arg("lock").output().unwrap();
    assert!(
        locked.status.success(),
        "{}",
        String::from_utf8_lossy(&locked.stderr)
    );
    assert!(!plans.join("telora-stale.json").exists());
    assert!(plans.join("host-note.txt").exists());
}

#[test]
fn ees_serves_install_shared_requests() {
    let root = fixture();
    let home = root.join("ees-refs");
    fs::create_dir(&home).unwrap();
    let mut child = telora(&root)
        .args([
            "ees",
            "--store",
            root.join("ees-store").to_str().unwrap(),
            "--home",
            home.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let plan = serde_json::json!({
        "version": 1,
        "name": "empty.json",
        "key": "empty-v1",
        "items": []
    });
    for request in [
        serde_json::json!({
            "id": "request-1",
            "actor": "imos",
            "operation": "InstallShared",
            "input": {"plan": plan}
        }),
        serde_json::json!({
            "id": "request-2",
            "actor": "imos",
            "operation": "InstallShared",
            "input": {"plan": plan}
        }),
        serde_json::json!({
            "id": "request-bad",
            "actor": "imos",
            "operation": "InstallShared",
            "input": {"plan": {}}
        }),
    ] {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let events = jsonl(&output.stdout);
    assert_eq!(events.len(), 3);
    let first = events
        .iter()
        .find(|event| event["id"] == "request-1")
        .unwrap();
    let second = events
        .iter()
        .find(|event| event["id"] == "request-2")
        .unwrap();
    let failed = events
        .iter()
        .find(|event| event["id"] == "request-bad")
        .unwrap();
    assert_eq!(first["type"], "result");
    assert_eq!(second["type"], "result");
    assert_eq!(first["value"]["root"], second["value"]["root"]);
    assert!(first["value"]["root"].as_str().unwrap().ends_with("/root"));
    assert_eq!(failed["type"], "error");
    assert!(home.join("empty.json").is_file());
}
