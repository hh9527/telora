use super::*;

fn service_fixture() -> PathBuf {
    let cwd = fixture();
    fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transform-service/src/main.telora"),
        cwd.join("src/main.telora")).unwrap();
    cwd
}

#[test]
fn run_and_serve_share_the_same_static_entry_and_preserve_diagnostics() {
    let cwd = service_fixture();
    let mut command = telora(&cwd);
    command.args(["run", "@src/main"]);
    let result = input_command(command, b"42");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&result.stdout).unwrap(), 42);
    let mut command = telora(&cwd);
    command.args(["serve", "@src/main", "--bind", "stdio+jsonl://"]);
    let result = input_command(command, b"42\nnull\n43\n{bad}\n44\n");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let replies = jsonl(&result.stdout);
    assert_eq!(replies.len(), 5);
    assert_eq!(replies[0]["ok"], 42);
    assert_eq!(replies[1]["error"], true);
    assert_eq!(replies[1]["diagnostics"][0]["message"], "missing input");
    let labels = replies[1]["diagnostics"][0]["labels"].as_array().unwrap();
    assert!(!labels.is_empty(), "the static failure rule still has a location");
    assert!(labels.iter().all(|label| label["location"]["source"] != "@request"));
    assert_eq!(replies[2]["ok"], 43);
    assert_eq!(replies[3]["error"], true);
    assert_eq!(replies[4]["ok"], 44);
}

#[test]
fn service_type_can_be_reexported_across_modules() {
    let cwd = service_fixture();
    fs::rename(cwd.join("src/main.telora"), cwd.join("src/provider.telora")).unwrap();
    fs::write(cwd.join("src/main.telora"), runtime_source("transform-reexport.telora")).unwrap();
    let mut command = telora(&cwd);
    command.args(["run", "@src/main"]);
    let output = input_command(command, b"42");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), 42);
}

#[test]
fn service_resets_after_request_resource_exhaustion() {
    let cwd = service_fixture();
    for (fuel, memory, payload, reason) in [
        ("1", "64", "42\n\"loop\"\n43\n", "fuel"),
        ("1000", "8", "42\n\"grow\"\n43\n", "growth"),
    ] {
        let mut command = telora(&cwd);
        command.args(["--report-usage", "--with-fuel", fuel, "--with-memory-limit", memory, "serve", "@src/main", "--bind", "stdio+jsonl://"]);
        let output = input_command(command, payload.as_bytes());
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let replies = jsonl(&output.stdout);
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0]["ok"], 42);
        assert_eq!(replies[1]["error"], true);
        assert!(replies[1]["diagnostics"][0]["message"].as_str().unwrap().contains(reason));
        assert_eq!(replies[2]["ok"], 43);
        let reports = jsonl(&output.stderr).into_iter()
            .filter(|record| record["code"] == "execution-usage").collect::<Vec<_>>();
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0]["usage"]["fuel"]["limit"], reports[2]["usage"]["fuel"]["limit"]);
        assert!(reports[2]["usage"]["fuel"]["remaining"].as_u64().unwrap() > 0);
        assert_eq!(reports[0]["usage"]["linear_memory"], reports[2]["usage"]["linear_memory"]);
    }
}

#[test]
fn entry_validation_precedes_user_initialization_and_rejects_old_protocols() {
    let cwd = fixture();
    for (file, message) in [
        ("transform-no-impl.telora", "TransformService"),
        ("transform-value-entry.telora", "type"),
        ("transform-duplicate-source.telora", "duplicate service source"),
        ("transform-init-failure.telora", "service initialization failed"),
    ] {
        fs::write(cwd.join("src/main.telora"), runtime_source(file)).unwrap();
        let output = execute_value(&cwd, "run", "@src/main");
        assert!(!output.status.success(), "{file}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(message), "{file}: {error}");
        assert!(output.stdout.is_empty());
    }
    for command in ["eval-with", "ees"] {
        let output = telora(&cwd).arg(command).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn initialization_sources_are_separate_from_each_transform_input() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), runtime_source("transform-input.telora")).unwrap();
    fs::write(cwd.join("src/base.json"), "{\"loaded\":true}").unwrap();
    fs::write(cwd.join("config.json"), "{\"prefix\":42}").unwrap();
    let mut command = telora(&cwd);
    command.args(["serve", "@src/main", "--source", "config=config.json", "--bind", "stdio+jsonl://"]);
    let output = input_command(command, b"{\"answer\":1,\"endpoint\":\"localhost:42\"}\n{\"answer\":0,\"endpoint\":\"localhost:42\"}\n{\"answer\":2,\"endpoint\":\"localhost:42\"}\n");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let replies = jsonl(&output.stdout);
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["ok"], serde_json::json!([{"prefix":42},{"loaded":true},{"answer":1,"endpoint":"localhost:42"}]));
    assert_eq!(replies[1]["error"], true);
    assert!(replies[1]["diagnostics"].to_string().contains("positive input required"));
    assert!(!replies[1]["diagnostics"].to_string().contains("@request"));
    assert_eq!(replies[2]["ok"][2]["answer"], 2);
    for extra in [vec![], vec!["--source", "other=config.json"], vec!["--source", "config=stdin+json://"]] {
        let output = telora(&cwd).args(["run", "@src/main"]).args(extra).output().unwrap();
        assert!(!output.status.success());
    }
    fs::write(cwd.join("config.json"), "{bad}").unwrap();
    let output = telora(&cwd).args(["run", "@src/main", "--source", "config=config.json"]).output().unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("@service/config"), "{error}");
    assert!(!error.contains("config.json"), "{error}");
}
