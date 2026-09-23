use super::*;

fn service_fixture() -> PathBuf {
    let cwd = fixture();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/transform-service/src/main.telora"),
        cwd.join("src/main.telora"),
    )
    .unwrap();
    cwd
}

fn method_request(raw: &str) -> String {
    match serde_json::from_str::<Value>(raw) {
        Ok(input) => serde_json::json!({"method":"transform","input":input}).to_string(),
        Err(_) => raw.to_owned(),
    }
}

fn method_lines(raw: &str) -> Vec<u8> {
    raw.lines()
        .map(method_request)
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

#[test]
fn direct_single_service_entry_is_rejected() {
    let cwd = fixture();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-single-service.telora"),
        cwd.join("src/main.telora"),
    )
    .unwrap();
    let mut command = telora(&cwd);
    command.args(["build", "@src/main", "-o", "app.wasm"]);
    let output = command.output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ServiceCollection"), "{stderr}");
}

#[test]
fn collection_runs_from_source_artifact_and_snapshot() {
    let cwd = fixture();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/service-collection.telora"),
        cwd.join("src/main.telora"),
    )
    .unwrap();
    let query = br#"{"method":"increment","input":41}"#;
    let mut command = telora(&cwd);
    command.args(["run", "@src/main"]);
    let output = input_command(command, query);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), 42);
    for (name, snapshot) in [("app.wasm", false), ("snapshot.wasm", true)] {
        let mut build = telora(&cwd);
        build.args(["build", "@src/main", "-o", name]);
        if snapshot {
            build.arg("--snapshot");
        }
        let output = build.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = fs::read(cwd.join(name)).unwrap();
        let mut runner = telora_run::Runner::load(&bytes, telora_run::Options::default()).unwrap();
        assert!(runner.initialize(&[]).unwrap().is_empty());
        let response: Value = serde_json::from_slice(&runner.request(query).unwrap()).unwrap();
        assert_eq!(response["ok"], 42);
        let http = br#"{"http":{"method":"GET","path":"/details/abc","query":""},"input":null}"#;
        let response: Value = serde_json::from_slice(&runner.request(http).unwrap()).unwrap();
        assert_eq!(
            response["ok"],
            serde_json::json!({"path":{"id":"abc"},"query":""})
        );
        let missing = br#"{"http":{"method":"GET","path":"/missing"},"input":null}"#;
        let response: Value = serde_json::from_slice(&runner.request(missing).unwrap()).unwrap();
        assert_eq!(response["httpStatus"], 404);
    }
}

#[test]
fn collection_http_transport_routes_and_reports_missing_endpoints() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let cwd = fixture();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/service-collection.telora"),
        cwd.join("src/main.telora"),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let mut command = telora(&cwd);
    command.args(["run", "@src/main", "--serve", &format!("http://{addr}")]);
    let mut server = Server(command.spawn().unwrap());
    let mut send = |method: &str, path: &str, body: &str| {
        let mut stream = (0..100)
            .find_map(|_| match TcpStream::connect(addr) {
                Ok(stream) => Some(stream),
                Err(_) => {
                    if let Some(status) = server.0.try_wait().unwrap() {
                        panic!("HTTP server exited early: {status}");
                    }
                    std::thread::sleep(Duration::from_millis(20));
                    None
                }
            })
            .expect("HTTP server did not start");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let result = send("POST", "/increment", "41");
    assert!(result.starts_with("HTTP/1.1 200"), "{result}");
    assert_eq!(
        serde_json::from_str::<Value>(result.split("\r\n\r\n").nth(1).unwrap()).unwrap()["ok"],
        42
    );
    let result = send("GET", "/details/abc?x=7&x=8&name=a+b", "");
    assert!(result.starts_with("HTTP/1.1 200"), "{result}");
    assert_eq!(
        serde_json::from_str::<Value>(result.split("\r\n\r\n").nth(1).unwrap()).unwrap()["ok"],
        serde_json::json!({"path":{"id":"abc"},"query":{"name":["a b"],"x":["7","8"]}})
    );
    let result = send("GET", "/absent", "");
    assert!(result.starts_with("HTTP/1.1 404"), "{result}");
    let result = send("GET", "/increment", "");
    assert!(result.starts_with("HTTP/1.1 405"), "{result}");
    assert!(
        result.to_ascii_lowercase().contains("allow: post\r\n"),
        "{result}"
    );
}

#[test]
fn checked_dyn_construction_runs_in_source_and_published_service() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        runtime_source("transform-dyn-fields.telora"),
    )
    .unwrap();
    let mut command = telora(&cwd);
    command.args(["run", "@src/main"]);
    let output = input_command(command, method_request("null").as_bytes());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), 42);
    let artifact = telora(&cwd)
        .args(["build", "@src/main", "-o", "app.wasm"])
        .output()
        .unwrap();
    assert!(
        artifact.status.success(),
        "{}",
        String::from_utf8_lossy(&artifact.stderr)
    );
    let bytes = fs::read(cwd.join("app.wasm")).unwrap();
    let mut runner = telora_run::Runner::load(&bytes, telora_run::Options::default()).unwrap();
    assert!(runner.initialize(&[]).unwrap().is_empty());
    let response: Value =
        serde_json::from_slice(&runner.request(method_request("null").as_bytes()).unwrap())
            .unwrap();
    assert_eq!(response["ok"], 42);
    let response: Value =
        serde_json::from_slice(&runner.request(method_request("0").as_bytes()).unwrap()).unwrap();
    assert_eq!(response["ok"], 42);
    let snapshot = telora(&cwd)
        .args(["build", "@src/main", "--snapshot", "-o", "snap.wasm"])
        .output()
        .unwrap();
    assert!(
        snapshot.status.success(),
        "{}",
        String::from_utf8_lossy(&snapshot.stderr)
    );
    let bytes = fs::read(cwd.join("snap.wasm")).unwrap();
    let mut runner = telora_run::Runner::load(&bytes, telora_run::Options::default()).unwrap();
    assert!(runner.initialize(&[]).unwrap().is_empty());
    let response: Value =
        serde_json::from_slice(&runner.request(method_request("null").as_bytes()).unwrap())
            .unwrap();
    assert_eq!(response["ok"], 42);
    let response: Value =
        serde_json::from_slice(&runner.request(method_request("0").as_bytes()).unwrap()).unwrap();
    assert_eq!(response["ok"], 42);
}

#[test]
fn build_publishes_final_fuel_budgets_and_runner_can_override_them() {
    let cwd = service_fixture();
    fs::write(
        cwd.join("telora-config.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "members": ["."],
            "runtime": {"initializationFuel": 11, "requestFuel": 13}
        }))
        .unwrap(),
    )
    .unwrap();
    let output = telora(&cwd)
        .args([
            "--initialization-fuel",
            "23",
            "--request-fuel",
            "31",
            "build",
            "@src/main",
            "-o",
            "app.wasm",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(cwd.join("app.wasm")).unwrap();
    let mut runner = telora_run::Runner::load(&bytes, telora_run::Options::default()).unwrap();
    assert_eq!(runner.publication.initialization_fuel, 23_000_000);
    assert_eq!(runner.publication.request_fuel, 31_000_000);
    assert_eq!(runner.usage().fuel_limit, 31_000_000);
    let mut runner = telora_run::Runner::load(
        &bytes,
        telora_run::Options {
            initialization_fuel: Some(41),
            request_fuel: Some(43),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(runner.usage().fuel_limit, 43_000_000);
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn run_and_serve_share_the_same_static_entry_and_preserve_diagnostics() {
    let cwd = service_fixture();
    let mut command = telora(&cwd);
    command.args(["run", "@src/main"]);
    let result = input_command(command, method_request("42").as_bytes());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(serde_json::from_slice::<Value>(&result.stdout).unwrap(), 42);
    let mut command = telora(&cwd);
    command.args(["run", "@src/main", "--serve", "stdio+jsonl://"]);
    let result = input_command(command, &method_lines("42\nnull\n43\n{bad}\n44\n"));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let replies = jsonl(&result.stdout);
    assert_eq!(replies.len(), 5);
    assert_eq!(replies[0]["ok"], 42);
    assert_eq!(replies[1]["error"], true);
    assert_eq!(replies[1]["diagnostics"][0]["message"], "missing input");
    let labels = replies[1]["diagnostics"][0]["labels"].as_array().unwrap();
    assert!(
        !labels.is_empty(),
        "the static failure rule still has a location"
    );
    assert!(
        labels
            .iter()
            .all(|label| label["location"]["source"] != "@request")
    );
    assert_eq!(replies[2]["ok"], 43);
    assert_eq!(replies[3]["error"], true);
    assert_eq!(replies[4]["ok"], 44);
}

#[test]
fn service_type_can_be_reexported_across_modules() {
    let cwd = service_fixture();
    fs::rename(cwd.join("src/main.telora"), cwd.join("src/provider.telora")).unwrap();
    fs::write(
        cwd.join("src/main.telora"),
        runtime_source("transform-reexport.telora"),
    )
    .unwrap();
    let mut command = telora(&cwd);
    command.args(["run", "@src/main"]);
    let output = input_command(command, method_request("42").as_bytes());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), 42);
}

#[test]
fn service_resets_after_request_resource_exhaustion() {
    let cwd = service_fixture();
    for (request_fuel, memory, payload, reason) in [
        ("1", "64", "42\n\"loop\"\n43\n", "fuel"),
        ("1000", "8", "42\n\"grow\"\n43\n", "growth"),
    ] {
        let mut command = telora(&cwd);
        command.args([
            "--report-usage",
            "--request-fuel",
            request_fuel,
            "--with-memory-limit",
            memory,
            "run",
            "@src/main",
            "--serve",
            "stdio+jsonl://",
        ]);
        let output = input_command(command, &method_lines(payload));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let replies = jsonl(&output.stdout);
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0]["ok"], 42);
        assert_eq!(replies[1]["error"], true);
        assert!(
            replies[1]["diagnostics"][0]["message"]
                .as_str()
                .unwrap()
                .contains(reason)
        );
        assert_eq!(replies[2]["ok"], 43);
        let reports = jsonl(&output.stderr)
            .into_iter()
            .filter(|record| record["code"] == "execution-usage")
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), 3);
        assert_eq!(
            reports[0]["usage"]["fuel"]["limit"],
            reports[2]["usage"]["fuel"]["limit"]
        );
        assert!(reports[2]["usage"]["fuel"]["remaining"].as_u64().unwrap() > 0);
        assert_eq!(
            reports[0]["usage"]["linear_memory"],
            reports[2]["usage"]["linear_memory"]
        );
    }
}

#[test]
fn request_fuel_limit_stops_loop_without_poisoning_next_request() {
    let cwd = service_fixture();
    let mut command = telora(&cwd);
    command.args([
        "--request-fuel",
        "1",
        "run",
        "@src/main",
        "--serve",
        "stdio+jsonl://",
    ]);
    let output = input_command(command, &method_lines("42\n\"loop\"\n43\n"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies = jsonl(&output.stdout);
    assert_eq!(replies[0]["ok"], 42);
    assert_eq!(replies[1]["error"], true);
    assert!(
        replies[1]["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("fuel")
    );
    assert_eq!(replies[2]["ok"], 43);
}

#[test]
fn entry_validation_precedes_user_initialization_and_rejects_old_protocols() {
    let cwd = fixture();
    for (file, message) in [
        ("transform-no-impl.telora", "TransformService"),
        ("transform-value-entry.telora", "type"),
        (
            "transform-duplicate-source.telora",
            "duplicate service source",
        ),
        (
            "transform-init-failure.telora",
            "service initialization failed",
        ),
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
    fs::write(
        cwd.join("src/main.telora"),
        runtime_source("transform-input.telora"),
    )
    .unwrap();
    fs::write(cwd.join("src/base.json"), "{\"loaded\":true}").unwrap();
    fs::write(cwd.join("config.json"), "{\"prefix\":42}").unwrap();
    let mut command = telora(&cwd);
    command.args([
        "run",
        "@src/main",
        "--source",
        "config=config.json",
        "--serve",
        "stdio+jsonl://",
    ]);
    let output = input_command(
        command,
        &method_lines(
            "{\"answer\":1,\"endpoint\":\"localhost:42\"}\n{\"answer\":0,\"endpoint\":\"localhost:42\"}\n{\"answer\":2,\"endpoint\":\"localhost:42\"}\n",
        ),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies = jsonl(&output.stdout);
    assert_eq!(replies.len(), 3);
    assert_eq!(
        replies[0]["ok"],
        serde_json::json!([{"prefix":42},{"loaded":true},{"answer":1,"endpoint":"localhost:42"}])
    );
    assert_eq!(replies[1]["error"], true);
    assert!(
        replies[1]["diagnostics"]
            .to_string()
            .contains("positive input required")
    );
    assert!(!replies[1]["diagnostics"].to_string().contains("@request"));
    assert_eq!(replies[2]["ok"][2]["answer"], 2);
    for extra in [
        vec![],
        vec!["--source", "other=config.json"],
        vec!["--source", "config=stdin+json://"],
    ] {
        let output = telora(&cwd)
            .args(["run", "@src/main"])
            .args(extra)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
    fs::write(cwd.join("config.json"), "{bad}").unwrap();
    let output = telora(&cwd)
        .args(["run", "@src/main", "--source", "config=config.json"])
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("@service/config"), "{error}");
    assert!(!error.contains("config.json"), "{error}");
}
