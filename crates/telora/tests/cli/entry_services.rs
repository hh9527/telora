use super::*;

fn service_fixture() -> PathBuf {
    let cwd = fixture();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/transform_service/src/main.telora"),
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
    for (name, snapshot) in [
        ("app.wasm", false),
        ("snapshot.wasm", true),
        ("runtime.wasm", true),
    ] {
        let mut build = telora(&cwd);
        build.args(["build", "@src/main", "-o", name]);
        if snapshot {
            build.arg(if name == "runtime.wasm" {
                "--snapshot"
            } else {
                "--snapshot=full"
            });
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
fn publication_compiles_modules_and_snapshot_sources_without_original_files() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        r#"
        # SOURCE_ONLY_PROGRAM_230
        use std::transform_service as service;
        use std::value::{Value};
        data js = import(json) "@src/module.json";
        data ya = import(yaml) "@src/module.yaml";
        data to = import(toml) "@src/module.toml";
        @service::source("external")
        type Model = struct {external: Value};
        impl service::TransformService for Model {
            init: fn(ctx) { {external: ctx.sources.external}.ty!(Self) },
            transform: fn(self, input) {
                Value::Object({modules: Value::Array([js, ya, to]), external: self.external, input})
            },
        };
        @service::collection
        pub type MainService = struct { @service::slot("transform") model: Model };
    "#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/module.json"),
        r#"{"number":9223372036854775807,"text":"decoded\ntext"}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/module.yaml"),
        "# SOURCE_ONLY_YAML_230\nbase: [1, 2]\ncopy: [1, 2]\n",
    )
    .unwrap();
    fs::write(
        cwd.join("src/module.toml"),
        "# SOURCE_ONLY_TOML_230\nflag = true\n",
    )
    .unwrap();
    fs::write(
        cwd.join("external.yaml"),
        "# SOURCE_ONLY_EXTERNAL_230\nvalue: \"external_value_only_230\"\n",
    )
    .unwrap();
    let mut artifacts = Vec::new();
    for (name, snapshot) in [
        ("ordinary.wasm", false),
        ("snapshot.wasm", true),
        ("runtime.wasm", true),
    ] {
        let mut build = telora(&cwd);
        build.args(["build", "@src/main", "-o", name]);
        if snapshot {
            build.arg(if name == "runtime.wasm" {
                "--snapshot"
            } else {
                "--snapshot=full"
            });
            build.args(["--source", "external=external.yaml"]);
        }
        let output = build.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = fs::read(cwd.join(name)).unwrap();
        for marker in [
            "SOURCE_ONLY_PROGRAM_230",
            "SOURCE_ONLY_YAML_230",
            "SOURCE_ONLY_TOML_230",
            "SOURCE_ONLY_EXTERNAL_230",
        ] {
            assert!(
                !bytes
                    .windows(marker.len())
                    .any(|part| part == marker.as_bytes()),
                "source leaked: {marker}"
            );
        }
        let mut manifest = None;
        let mut passive = 0;
        for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
            match payload.unwrap() {
                wasmparser::Payload::CustomSection(section) => {
                    assert!(!matches!(
                        section.name(),
                        "telora.data" | "telora.tooling" | "telora.modules" | "name"
                    ));
                    if section.name() == "telora.manifest" {
                        assert!(section.data().len() < 32);
                        manifest = Some(serde_json::from_slice::<Value>(section.data()).unwrap());
                    }
                }
                wasmparser::Payload::DataSection(reader) => {
                    for segment in reader {
                        if matches!(segment.unwrap().kind, wasmparser::DataKind::Passive) {
                            passive += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        assert_eq!(manifest.unwrap().as_object().unwrap().len(), 1);
        assert_eq!(passive, if snapshot { 0 } else { 1 });
        if !snapshot {
            assert!(
                !bytes
                    .windows(b"external_value_only_230".len())
                    .any(|part| part == b"external_value_only_230")
            );
        }
        artifacts.push((bytes, snapshot));
    }
    fs::remove_dir_all(cwd.join("src")).unwrap();
    fs::remove_file(cwd.join("external.yaml")).unwrap();
    for (bytes, snapshot) in artifacts {
        let mut runner = telora_run::Runner::load(&bytes, telora_run::Options::default()).unwrap();
        let external = telora_run::SourceInput {
            name: "external".into(),
            format: 1,
            data: br#"{"value":"external_value_only_230"}"#.to_vec(),
        };
        assert!(
            runner
                .initialize(if snapshot {
                    &[]
                } else {
                    std::slice::from_ref(&external)
                })
                .unwrap()
                .is_empty()
        );
        for input in [41, 42] {
            let response: Value = serde_json::from_slice(
                &runner
                    .request(method_request(&input.to_string()).as_bytes())
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(response["error"], false, "{response}");
            assert_eq!(
                response["ok"],
                serde_json::json!({
                    "modules": [{"number":i64::MAX,"text":"decoded\ntext"}, {"base":[1,2],"copy":[1,2]}, {"flag":true}],
                    "external":{"value":"external_value_only_230"}, "input":input,
                })
            );
        }
        if snapshot {
            let mut runner =
                telora_run::Runner::load(&bytes, telora_run::Options::default()).unwrap();
            assert!(
                runner
                    .initialize(&[external])
                    .unwrap_err()
                    .to_string()
                    .contains("already compiled")
            );
        }
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
        // Startup includes source compilation and competes with other CLI cases.
        let mut stream = (0..500)
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
    let locs = replies[1]["diagnostics"][0]["locs"].as_array().unwrap();
    assert!(
        !locs.is_empty(),
        "the static failure rule still has a location"
    );
    assert!(locs.iter().any(|loc| loc["source"] == "<input>"
        && loc["start"]["line"] == 0
        && loc["end"]["line"] == 0));
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
        // Recovery relocates only the published graph; its allocator history
        // can have a smaller high-water mark than the original live instance.
        assert!(
            reports[2]["usage"]["linear_memory"]["bytes"]
                .as_u64()
                .unwrap()
                <= reports[0]["usage"]["linear_memory"]["bytes"]
                    .as_u64()
                    .unwrap()
        );
        assert_eq!(
            reports[0]["usage"]["linear_memory"]["limit_bytes"],
            reports[2]["usage"]["linear_memory"]["limit_bytes"]
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
    assert!(
        replies[1]["diagnostics"][0]["locs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|loc| loc["source"] == "<input>" && loc["start"]["line"] == 0)
    );
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

#[test]
fn build_phase_timings_are_opt_in_and_preserve_artifacts() {
    let cwd = service_fixture();
    for snapshot in [false, true] {
        let build = |report: bool| {
            let mut command = telora(&cwd);
            command.env_remove("TELORA_WASM_TIMINGS").args([
                "build",
                "@src/main",
                "-o",
                "timed.wasm",
            ]);
            if snapshot {
                command.arg("--snapshot");
            }
            if report {
                command.arg("--report-usage");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            (output, fs::read(cwd.join("timed.wasm")).unwrap())
        };
        let (plain, plain_bytes) = build(false);
        assert!(plain.stderr.is_empty());
        let (timed, timed_bytes) = build(true);
        assert_eq!(plain_bytes, timed_bytes);
        let diagnostics: Vec<Value> = String::from_utf8(timed.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let phases: Vec<&str> = diagnostics
            .iter()
            .filter(|d| d["schema"] == "telora.build.timing/v1")
            .map(|d| {
                assert_eq!(d["schema"], "telora.build.timing/v1");
                assert_eq!(d["code"], "build-phase-timing");
                assert_eq!(d["record"], "diagnostic");
                let phase = d["phase"].as_str().unwrap();
                let parent = if phase == "total" {
                    Value::Null
                } else if phase == "runtime_specialization" {
                    serde_json::json!("snapshot")
                } else if phase.starts_with("load_") {
                    serde_json::json!("engine_load")
                } else {
                    serde_json::json!("total")
                };
                assert_eq!(d["parent_phase"], parent);
                assert!(d["elapsed_seconds"].as_f64().unwrap() >= 0.0);
                d["phase"].as_str().unwrap()
            })
            .collect();
        let mut expected = vec!["frontend", "codegen_link", "data_bundle"];
        if snapshot {
            expected.extend([
                "load_metadata",
                "load_engine",
                "load_module",
                "load_store",
                "load_instance_start",
                "load_exports",
                "engine_load",
                "service_prepare",
                "service_initialize",
                "service_seal",
                "runtime_specialization",
                "snapshot",
            ]);
        }
        expected.extend(["artifact_output", "total"]);
        assert_eq!(phases, expected);
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn specialized_snapshot_preserves_type_identity_and_runtime_checks() {
    let cwd = fixture();
    fs::write(cwd.join("src/main.telora"), r#"
        use std::transform_service as service;
        use std::codec as codec;
        use std::value::{Value};
        def minimum: Int = 0;
        @check(fn(value) { if value > minimum { Ok(()) } else { Err(blame!("positive required", value)) } })
        type Id = struct(Int);
        def offsets: Array(Int) = [10, 20];
        use std::type_property as props;
        @property(PropertyTarget.Type) type Mark = struct { value: Int };
        def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: 0} };
        @mark type Tagged = struct {};
        type Untagged = struct {};
        def even: Fn(Int) -> Bool = fn(n) { if n == 0 { True } else { odd(n - 1) } };
        def odd: Fn(Int) -> Bool = fn(n) { if n == 0 { False } else { even(n - 1) } };
        type Model = struct { kind: Type, offset: Int, choices: Array(Fn(Int) -> Bool) };
        impl service::TransformService for Model {
            init: fn(ctx) { {kind: Id.type, offset: offsets[1], choices: [even, odd]}.ty!(Self) },
            transform: fn(self, input) {
                match codec::decode@[Id](input) {
                    Ok(id) => if self.kind == Id.type && self.choices[0](4) && self.choices[1](3)
                        && props::optional@[Mark, Untagged]() == None {
                        Value::Int(id.0 + self.offset + props::required@[Mark, Tagged]().value)
                    } else { fail!("type identity or captured behavior changed") },
                    Err(_) => Value::False,
                }
            },
        };
        @service::collection
        pub type MainService = struct { @service::slot("transform") model: Model };
    "#).unwrap();
    let mut build = telora(&cwd);
    build.env("TELORA_WASM_TIMINGS", "1");
    build.args(["build", "@src/main", "--snapshot", "-o", "runtime.wasm"]);
    let output = build.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["schema"] == "telora.runtime-specialization/v1")
        .unwrap();
    assert!(report["prepared_getters"].as_u64().unwrap() > 0, "{report}");
    let bytes = fs::read(cwd.join("runtime.wasm")).unwrap();
    fs::remove_dir_all(cwd.join("src")).unwrap();
    let mut runner = telora_run::Runner::load(&bytes, telora_run::Options::default()).unwrap();
    assert!(runner.initialize(&[]).unwrap().is_empty());
    for (input, expected) in [
        ("1", serde_json::json!(21)),
        ("0", serde_json::json!(false)),
        ("null", serde_json::json!(false)),
        ("2", serde_json::json!(22)),
    ] {
        let response: Value =
            serde_json::from_slice(&runner.request(method_request(input).as_bytes()).unwrap())
                .unwrap();
        assert_eq!(response["error"], false, "{response}");
        assert_eq!(response["ok"], expected);
    }
}

#[test]
fn specialized_snapshot_does_not_publish_failed_initialization() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        r#"
        use std::transform_service as service;
        type Model = struct {};
        impl service::TransformService for Model {
            init: fn(ctx) { fail!("A failed") },
            transform: fn(self, input) { input },
        };
        @service::collection
        pub type MainService = struct { @service::slot("transform") model: Model };
    "#,
    )
    .unwrap();
    fs::write(cwd.join("runtime.wasm"), b"previous artifact").unwrap();
    let mut build = telora(&cwd);
    build.args(["build", "@src/main", "--snapshot", "-o", "runtime.wasm"]);
    let output = build.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("A failed"));
    assert_eq!(
        fs::read(cwd.join("runtime.wasm")).unwrap(),
        b"previous artifact"
    );
}

#[test]
fn specialized_snapshot_recovers_after_recursive_fuel_and_memory_traps() {
    let cwd = service_fixture();
    let output = telora(&cwd)
        .args(["build", "@src/main", "--snapshot", "-o", "runtime.wasm"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(cwd.join("runtime.wasm")).unwrap();
    for (fuel, memory, input) in [
        (10_000, 64 * 1024 * 1024, "\"loop\""),
        (1_000_000_000, 8 * 1024 * 1024, "\"grow\""),
    ] {
        let mut runner = telora_run::Runner::load(
            &bytes,
            telora_run::Options {
                request_fuel: Some(fuel),
                memory_limit: Some(memory),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(runner.initialize(&[]).unwrap().is_empty());
        let first: Value =
            serde_json::from_slice(&runner.request(method_request("42").as_bytes()).unwrap())
                .unwrap();
        assert_eq!(first["ok"], 42);
        let failed = runner
            .request(method_request(input).as_bytes())
            .unwrap_err();
        let reason = failed.to_string();
        assert!(
            reason.contains(if input == "\"loop\"" {
                "fuel"
            } else {
                "growth"
            }),
            "{reason}"
        );
        let recovered: Value =
            serde_json::from_slice(&runner.request(method_request("43").as_bytes()).unwrap())
                .unwrap();
        assert_eq!(recovered["ok"], 43);
    }
}

#[test]
fn snapshot_specialization_preserves_conservative_provider_failures() {
    let cwd = service_fixture();
    let mut source = fs::read_to_string(cwd.join("src/main.telora")).unwrap();
    source.push_str(r#"
        @property(PropertyTarget.Type) type Mark = struct { value: Int };
        def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { fail!("provider must run in A") };
        @mark type Unused = struct {};
    "#);
    fs::write(cwd.join("src/main.telora"), source).unwrap();
    for mode in ["--snapshot", "--snapshot=full"] {
        fs::write(cwd.join("existing.wasm"), b"previous artifact").unwrap();
        let output = telora(&cwd)
            .args(["build", "@src/main", mode, "-o", "existing.wasm"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("provider must run in A"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read(cwd.join("existing.wasm")).unwrap(),
            b"previous artifact"
        );
    }
}

fn service_metadata(bytes: &[u8]) -> Value {
    let sections = wasmparser::Parser::new(0)
        .parse_all(bytes)
        .filter_map(|part| match part.unwrap() {
            wasmparser::Payload::CustomSection(section)
                if section.name() == "telora.service.metadata" =>
            {
                Some(serde_json::from_slice::<Value>(section.data()).unwrap())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(sections.len(), 1);
    sections.into_iter().next().unwrap()
}

#[test]
fn snapshot_service_metadata_is_static_and_matches_both_modes() {
    let cwd = service_fixture();
    let source = fs::read_to_string(cwd.join("src/main.telora"))
        .unwrap()
        .replace(
            "init: fn(ctx) { {}.ty!(Self) }",
            r#"init: fn(ctx) {
            service::publish_metadata("facts", Value::Object({answer: Value::Int(42)}));
            service::publish_metadata("labels", Value::String("实体"));
            {}.ty!(Self)
        }"#,
        );
    fs::write(cwd.join("src/main.telora"), source).unwrap();
    for mode in ["--snapshot", "--snapshot=full"] {
        let output = telora(&cwd)
            .args(["build", "@src/main", mode, "-o", "metadata.wasm"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = fs::read(cwd.join("metadata.wasm")).unwrap();
        assert_eq!(
            service_metadata(&bytes),
            serde_json::json!({"version":1,"entries":{"facts":{"answer":42},"labels":"实体"}})
        );
        let mut runner = telora_run::Runner::load(&bytes, telora_run::Options::default()).unwrap();
        assert!(runner.initialize(&[]).unwrap().is_empty());
        let response: Value =
            serde_json::from_slice(&runner.request(method_request("42").as_bytes()).unwrap())
                .unwrap();
        assert_eq!(response["ok"], 42);
    }
}

#[test]
fn snapshot_service_metadata_rejects_duplicate_and_empty_names_without_overwrite() {
    for name in ["", "facts"] {
        let cwd = service_fixture();
        let source = fs::read_to_string(cwd.join("src/main.telora"))
            .unwrap()
            .replace(
                "init: fn(ctx) { {}.ty!(Self) }",
                &format!(
                    r#"init: fn(ctx) {{
                service::publish_metadata("facts", Value::Int(42));
                service::publish_metadata("{name}", Value::Int(43));
                {{}}.ty!(Self)
            }}"#
                ),
            );
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        for mode in ["--snapshot", "--snapshot=full"] {
            fs::write(cwd.join("existing.wasm"), b"previous artifact").unwrap();
            let output = telora(&cwd)
                .args(["build", "@src/main", mode, "-o", "existing.wasm"])
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("unique nonempty name"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                fs::read(cwd.join("existing.wasm")).unwrap(),
                b"previous artifact"
            );
        }
    }
}

#[test]
fn snapshot_service_metadata_cannot_be_published_by_runtime_handler() {
    let cwd = service_fixture();
    let source = fs::read_to_string(cwd.join("src/main.telora"))
        .unwrap()
        .replace(
            "transform: fn(self, input) {",
            "transform: fn(self, input) { service::publish_metadata(\"late\", input);",
        );
    fs::write(cwd.join("src/main.telora"), source).unwrap();
    for mode in ["--snapshot", "--snapshot=full"] {
        let output = telora(&cwd)
            .args(["build", "@src/main", mode, "-o", "late.wasm"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = fs::read(cwd.join("late.wasm")).unwrap();
        let mut runner = telora_run::Runner::load(&bytes, telora_run::Options::default()).unwrap();
        assert!(runner.initialize(&[]).unwrap().is_empty());
        let response = runner.request(method_request("42").as_bytes()).unwrap();
        assert!(
            String::from_utf8_lossy(&response).contains("unique nonempty name"),
            "{}",
            String::from_utf8_lossy(&response)
        );
    }
}
