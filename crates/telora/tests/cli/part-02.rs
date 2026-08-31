#[test]
fn lined_stdin_emits_initialize_lines_and_one_eof() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    fs::write(
        cwd.join("src/entry/lined.telora"),
        r#"import "std/_rt" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = Int;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {
        (
            {data_srcs: {}, ees: {}, spawn_child: 'False, text_srcs: {}, vars: [], stdin: 'Lined},
            fn(resources, main) {
                (0, fn(state, event) {
                    match event {
                        'Initialize => (1, ['Output("initialize|")]),
                        'StdinLine('Some(line)) => if state == 0 {
                            fail!("line arrived before Initialize", line)
                        } else {
                            (state + 1, ['Output(`\{line}|`)])
                        },
                        'StdinLine('None) => (state, ['Output(`eof:\{state}`), 'Exit(0)]),
                        _ => fail!("unexpected event", event),
                    }
                })
            },
        )
    };"#,
    )
    .unwrap();
    let mut child = telora(&cwd)
        .args(["run-with", "@src/entry/lined", "main"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"first\nsecond\n")
        .unwrap();
    let run = child.wait_with_output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "initialize|first|second|eof:3"
    );
}

#[test]
fn missing_requested_source_stops_before_initializer() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    fs::write(
        cwd.join("src/entry/missing.telora"),
        r#"import "std/_rt" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = Int;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {
        (
            {data_srcs: {}, ees: {}, spawn_child: 'False, text_srcs: {missing: {default: 'None, src: "missing.txt"}}, vars: [], stdin: 'Null},
            fn(resources, main) {
                (0, fn(state, event) { (state, ['Output("initializer ran"), 'Exit(0)]) })
            },
        )
    };"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/missing", "main"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(run.stdout.is_empty());
    assert!(String::from_utf8_lossy(&run.stderr).contains("cannot read text source"));
}

#[test]
fn missing_sources_use_defaults_but_invalid_existing_data_fails() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    fs::write(
        cwd.join("src/entry/defaults.telora"),
        r#"import "std/_rt" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = String;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {
        (
            {
                data_srcs: {
                    input: {default: 'Some('String("from-default")), fmt: 'Json, src: "input.json"},
                },
                ees: {},
                spawn_child: 'False,
                text_srcs: {
                    message: {default: 'Some("text-default"), src: "message.txt"},
                },
                vars: [],
                stdin: 'Null,
            },
            fn(resources, main) {
                let data = match resources.data.input.data {
                    'String(value) => value,
                    _ => fail!("expected default JSON string"),
                };
                let output = `\{data}|\{resources.texts.message.data}`;
                (output, fn(state, event) {
                    match event {
                        'Initialize => (state, ['Output(state), 'Exit(0)]),
                        _ => fail!("unexpected event", event),
                    }
                })
            },
        )
    };"#,
    )
    .unwrap();

    let defaulted = telora(&cwd)
        .args(["run-with", "@src/entry/defaults", "main"])
        .output()
        .unwrap();
    assert!(
        defaulted.status.success(),
        "{}",
        String::from_utf8_lossy(&defaulted.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&defaulted.stdout),
        "from-default|text-default"
    );

    fs::write(cwd.join("input.json"), "{").unwrap();
    let invalid = telora(&cwd)
        .args(["run-with", "@src/entry/defaults", "main"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("input.json"));
}

#[test]
fn run_rejects_removed_input_option() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        "export def output = \"unused\";",
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--input", "input.json"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("unexpected argument '--input'"));
}

#[test]
fn lock_is_the_only_command_that_recreates_a_missing_workspace_lock() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "export def value = 1;").unwrap();
    let mut check = telora(&cwd);
    fs::remove_file(cwd.join("telora-lock.json")).unwrap();
    let missing = check.args(["check", "@src/lib"]).output().unwrap();
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("run `telora lock`"),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );

    let locked = telora(&cwd).arg("lock").output().unwrap();
    assert!(
        locked.status.success(),
        "{}",
        String::from_utf8_lossy(&locked.stderr)
    );
    assert!(cwd.join("telora-lock.json").is_file());
}

#[test]
fn check_warns_about_files_outside_the_authoritative_module_catalog() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "export def value = 1;").unwrap();
    let mut check = telora(&cwd);
    fs::write(cwd.join("src/extra.telora"), "export def extra = 2;").unwrap();
    let output = check.args(["check", "@src/lib"]).output().unwrap();
    assert!(output.status.success());
    let records = jsonl(&output.stdout);
    let warning = records
        .iter()
        .find(|record| record["severity"] == "warning")
        .expect("undeclared module warning");
    assert!(warning["message"].as_str().unwrap().contains("src/extra.telora"));
    assert!(warning["message"].as_str().unwrap().contains("@src/extra"));
}

#[test]
fn remote_crates_are_materialized_through_the_embedded_ees() {
    let root = fixture();
    fs::remove_file(root.join("telora-crate.json")).unwrap();
    fs::remove_file(root.join("telora-lock.json")).unwrap();
    let app = root.join("app");
    fs::create_dir_all(app.join("src/bin")).unwrap();
    let tarball = root.join("remote.tar.gz");
    {
        let file = fs::File::create(&tarball).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let files = [
            (
                "remote/telora-crate.json",
                br#"{"name":"remote","modules":["@src/lib"],"dependencies":[]}"#.as_slice(),
            ),
            ("remote/src/lib.telora", b"export def answer = 42;".as_slice()),
        ];
        for (name, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append(&header, contents).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();
    }
    let tarball_bytes = fs::read(&tarball).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            tarball_bytes.len()
        )
        .unwrap();
        stream.write_all(&tarball_bytes).unwrap();
    });
    fs::write(
        root.join("telora-config.json"),
        format!(
            r#"{{"version":1,"members":["app"],"sources":{{"remote":{{"tarball":"http://{address}/remote.tar.gz"}}}}}}"#
        ),
    )
    .unwrap();
    fs::write(
        app.join("telora-crate.json"),
        r#"{"name":"app","modules":[],"dependencies":["remote"]}"#,
    )
    .unwrap();
    fs::write(
        app.join("src/bin/main.telora"),
        "import \"remote/lib\" {answer}; export {answer};",
    )
    .unwrap();
    let store = root.join("ees-store");
    let locked = telora(&root)
        .arg("lock")
        .env("TELORA_EES_STORE", &store)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        locked.status.success(),
        "{}",
        String::from_utf8_lossy(&locked.stderr)
    );
    let queried = telora(&root)
        .args(["-C", app.to_str().unwrap(), "query", "exports", "remote/lib"])
        .env("TELORA_EES_STORE", &store)
        .output()
        .unwrap();
    assert!(
        queried.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&queried.stdout),
        String::from_utf8_lossy(&queried.stderr)
    );
    assert!(jsonl(&queried.stdout)
        .iter()
        .any(|record| record["name"] == "answer"));
    let plans = fs::read_dir(root.join(".telora/crates-refs"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(plans.len(), 1);
    let plan: Value = serde_json::from_slice(&fs::read(plans[0].path()).unwrap()).unwrap();
    assert_eq!(plan["items"][0]["kind"]["archive"], "TarGzip");
}

#[test]
fn run_context_selects_the_manifest_discovery_start() {
    let cwd = fixture();
    let other = fixture();
    fs::write(
        other.join("src/bin/tool.telora"),
        r#"import "std/value" {Value};
export def main: Fn(Dict(Value)) -> Value = fn(sources) { 'Int(9) };"#,
    )
    .unwrap();
    refresh_fixture_workspace(&other);
    let run = telora(&cwd)
        .args(["-C", other.to_str().unwrap(), "run", "tool"])
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

#[test]
fn standalone_run_uses_only_embedded_dependency_options() {
    let cwd = fixture();
    let dependency = cwd.join("dep");
    fs::create_dir_all(dependency.join("src")).unwrap();
    fs::write(
        dependency.join("src/value.telora"),
        "export def value = 12;",
    )
    .unwrap();
    let standalone = cwd.join("standalone.telora");
    fs::write(&standalone, r#"option "crate.dependency" {name: "dep", source: 'Path({path: "dep"})}; import "dep/value" {value}; import "std/value" {Value}; export def main: Fn(Dict(Value)) -> Value = fn(sources) { 'Int(value) };"#).unwrap();
    let run = telora(&cwd)
        .args(["run", "-S", standalone.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "12");
    let context_conflict = telora(&cwd)
        .args([
            "-C",
            cwd.to_str().unwrap(),
            "run",
            "-S",
            standalone.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!context_conflict.status.success());
    assert!(String::from_utf8_lossy(&context_conflict.stderr)
        .contains("-C cannot be used with -S"));
    fs::write(
        cwd.join("src/bin/standalone.telora"),
        fs::read_to_string(&standalone).unwrap(),
    )
    .unwrap();
    let crate_mode = telora(&cwd).args(["run", "standalone"]).output().unwrap();
    assert!(!crate_mode.status.success());
    assert!(
        String::from_utf8_lossy(&crate_mode.stderr).contains("only allowed in standalone mode")
    );
    let conflict = telora(&cwd)
        .args(["run", "main", "-S", standalone.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
}

#[test]
fn exec_and_build_are_unknown_subcommands() {
    let cwd = fixture();
    for command in ["exec", "build"] {
        let output = telora(&cwd).arg(command).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
    }
}

#[test]
fn run_accepts_a_pure_custom_entry() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def answer = 42;").unwrap();
    write_entry(
        &cwd,
        r#"import "std/_rt" as rt;
type Main = struct {answer: Int};
export type MainType = Main;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
type Transition = Tuple([State, Array(rt.SystemEffect)]);
type Reducer = Fn(State, rt.SystemEvent) -> Transition;
def legacy_initialize: Fn(MainType) -> Tuple([State, Reducer]) = fn(main) {
    let reduce: Reducer = fn(state, event) {
        match event {
            'Initialize => (state, ['Output("42"), 'Exit(0)]),
            _ => fail!("unexpected event", event),
        }
    };
    (main.answer, reduce)
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");
}

#[test]
fn run_is_run_with_the_default_entry_and_rejects_the_old_entry_flag() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/value" {Value};
export def main: Fn(Dict(Value)) -> Value = fn(sources) { 'String("default") };"#,
    )
    .unwrap();
    let implicit = telora(&cwd).args(["run", "main"]).output().unwrap();
    let explicit = telora(&cwd)
        .args(["run-with", "std/entry/default", "main"])
        .output()
        .unwrap();
    assert!(
        implicit.status.success(),
        "{}",
        String::from_utf8_lossy(&implicit.stderr)
    );
    assert!(
        explicit.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert_eq!(implicit.stdout, explicit.stdout);
    assert_eq!(implicit.stdout, br#""default""#);

    let old = telora(&cwd)
        .args(["run", "main", "--entry", "anything.entry.telora"])
        .output()
        .unwrap();
    assert!(!old.status.success());
    assert!(String::from_utf8_lossy(&old.stderr).contains("unexpected argument '--entry'"));
}

#[test]
fn run_injects_declared_file_and_stdin_value_sources() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/value" {Value};
import "std/dict" as dict;
option "run-ctx.sources" ["request"];
export def main: Fn(Dict(Value)) -> Value = fn(sources) {
    match dict.get(sources, "request") {
        'Some(value) => value,
        'None => fail!("missing request"),
    }
};"#,
    )
    .unwrap();
    let source = cwd.join("request.json");
    fs::write(&source, r#"{"kind":"file","value":3}"#).unwrap();
    let file = telora(&cwd)
        .args([
            "run",
            "main",
            "--source",
            &format!("request={}", source.display()),
        ])
        .output()
        .unwrap();
    assert!(
        file.status.success(),
        "{}",
        String::from_utf8_lossy(&file.stderr)
    );
    assert_eq!(file.stdout, br#"{"kind":"file","value":3}"#);

    let mut child = telora(&cwd)
        .args(["run", "main", "--source", "request=stdin+json://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"kind":"stdin","value":5}"#)
        .unwrap();
    let stdin = child.wait_with_output().unwrap();
    assert!(
        stdin.status.success(),
        "{}",
        String::from_utf8_lossy(&stdin.stderr)
    );
    assert_eq!(stdin.stdout, br#"{"kind":"stdin","value":5}"#);
}

#[test]
fn run_context_provenance_uses_the_public_key_not_the_source_locator() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/value" {Value};
option "run-ctx.sources" ["request"];
export def main: Fn(Dict(Value)) -> Value = fn(sources) { 'None };"#,
    )
    .unwrap();
    let source = cwd.join("private-input.json");
    fs::write(&source, r#"{"broken": }"#).unwrap();
    let run = telora(&cwd)
        .args([
            "run",
            "main",
            "--source",
            &format!("request={}", source.display()),
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success());
    assert!(stderr.contains("@run-ctx/request"), "{stderr}");
    assert!(!stderr.contains(source.to_string_lossy().as_ref()), "{stderr}");
}

#[test]
fn run_rejects_sources_that_do_not_match_run_context_declaration() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/value" {Value};
option "run-ctx.sources" ["request"];
export def main: Fn(Dict(Value)) -> Value = fn(sources) { 'None };"#,
    )
    .unwrap();
    let missing = telora(&cwd).args(["run", "main"]).output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("was not provided"));

    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/value" {Value};
export def main: Fn(Dict(Value)) -> Value = fn(sources) { 'None };"#,
    )
    .unwrap();
    let extra = telora(&cwd)
        .args(["run", "main", "--source", "request=stdin+json://"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!extra.status.success());
    assert!(String::from_utf8_lossy(&extra.stderr).contains("undeclared source"));
}

#[test]
fn serve_stdio_processes_jsonl_with_one_initialized_handler() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/value" {Value};
export def serve: Fn(Dict(Value)) -> Fn(Value) -> Value = fn(sources) {
    fn(request) { request }
};"#,
    )
    .unwrap();
    let mut child = telora(&cwd)
        .args(["serve", "main", "--bind", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"request\":1}\n[2,3]\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"{\"diagnostics\":[],\"error\":false,\"ok\":{\"request\":1}}\n\
          {\"diagnostics\":[],\"error\":false,\"ok\":[2,3]}\n"
    );
}

#[test]
fn serve_stdio_returns_request_local_diagnostics_and_keeps_serving() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/value" {Value};
export def serve: Fn(Dict(Value)) -> Fn(Value) -> Value = fn(sources) {
    fn(request) {
        match request {
            'Int(value) => if value < 0 {
                fail!("value must not be negative", request)
            } else {
                let warned = fn(item) {
                    if item == 0 { 'Err("zero is accepted with a warning") } else { 'Ok(item) }
                }.should_ok!(value);
                request
            },
            _ => fail!("request must be an Int", request),
        }
    }
};"#,
    )
    .unwrap();
    let mut child = telora(&cwd)
        .args(["serve", "main", "--bind", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"-1\n0\n2\n\"bad\"\n3\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0]["error"], true);
    assert_eq!(lines[0]["ok"], serde_json::Value::Null);
    assert_eq!(
        lines[0]["diagnostics"][0]["message"],
        "value must not be negative"
    );
    assert_eq!(lines[1]["error"], false);
    assert_eq!(lines[1]["ok"], 0);
    assert_eq!(
        lines[1]["diagnostics"][0]["message"],
        "zero is accepted with a warning"
    );
    assert_eq!(lines[2]["error"], false);
    assert_eq!(lines[2]["ok"], 2);
    assert_eq!(lines[3]["error"], true);
    assert_eq!(lines[3]["diagnostics"][0]["message"], "request must be an Int");
    assert_eq!(lines[4]["error"], false);
    assert_eq!(lines[4]["ok"], 3);
    assert!(output.stderr.is_empty(), "request diagnostics must be in-band");
}

#[test]
fn serve_stdio_does_not_capture_terminal_runtime_failures() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/value" {Value};
export def serve: Fn(Dict(Value)) -> Fn(Value) -> Value = fn(sources) {
    decl recurse: Fn(Int) -> Int;
    def recurse = fn(value) { 1 + recurse(value) };
    fn(request) { 'Int(recurse(0)) }
};"#,
    )
    .unwrap();
    let mut child = telora(&cwd)
        .args(["serve", "main", "--bind", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"1\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "terminal failures have no response envelope");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("call depth"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runtime_diagnostic_native_module_obeys_resolver_visibility() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/lib.telora"),
        r#"import "std/_rt" as rt;
export def leaked = rt.with_diagnostics;"#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["check", "@src/lib"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("private module"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn selected_entry_does_not_gain_native_declaration_authority() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    fs::write(
        cwd.join("src/entry/native.telora"),
        "native forbidden: Fn() -> Int; export { forbidden };",
    )
    .unwrap();

    let output = telora(&cwd)
        .args(["run-with", "@src/entry/native", "main"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("only allowed in built-in std modules"),
        "{stderr}"
    );
    assert!(stderr.contains("fixture/entry/native:1:1"), "{stderr}");
}

#[test]
fn entry_modules_are_only_selectable_by_run_with() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/entry/hidden.telora"),
        "export def hidden = 1;",
    )
    .unwrap();
    fs::write(
        cwd.join("src/lib.telora"),
        "import \"@src/entry/hidden\" {hidden}; export {hidden};",
    )
    .unwrap();

    let imported = telora(&cwd)
        .args(["check", "@src/lib"])
        .output()
        .unwrap();
    assert!(!imported.status.success());
    assert!(String::from_utf8_lossy(&imported.stdout).contains("Entry module"));

    let root = telora(&cwd)
        .args(["check", "@src/entry/hidden"])
        .output()
        .unwrap();
    assert!(!root.status.success());
    assert!(String::from_utf8_lossy(&root.stderr).contains("entry module"));
}

#[test]
fn entry_config_receives_ordered_options_environment_and_captures_configuration() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"option "app.first" 1;
option "app.second" 2;
export def marker = 0;"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/entry/env.telora"),
        r#"import "std/_rt" as rt;
import "std/array" as array;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = String;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {
        let first = match array.get(options, 0) {
            'Some(value) => value,
            'None => fail!("missing first option"),
        };
        let second = match array.get(options, 1) {
            'Some(value) => value,
            'None => fail!("missing second option"),
        };
        let arg = match array.get(env.args, 0) {
            'Some(value) => value,
            'None => fail!("missing Entry argument"),
        };
        let configured = `\{first.key},\{second.key}:\{arg}:\{env.platform.os}:\{env.platform.arch}`;
        (
            {data_srcs: {}, ees: {}, spawn_child: 'False, text_srcs: {}, vars: [], stdin: 'Null},
            fn(resources, main) {
                (configured, fn(state, event) {
                    match event {
                        'Initialize => (state, ['Output(state), 'Exit(0)]),
                        _ => fail!("unexpected event", event),
                    }
                })
            },
        )
    };"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args([
            "run-with",
            "@src/entry/env",
            "main",
            "--",
            "argument",
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let output = String::from_utf8(run.stdout).unwrap();
    assert!(
        output.starts_with("app.first,app.second:argument:"),
        "{output}"
    );
    assert!(output.contains(std::env::consts::OS), "{output}");
    assert!(output.ends_with(std::env::consts::ARCH), "{output}");
}

#[test]
fn custom_entry_can_choose_a_dynamic_main_contract() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def answer = 42;").unwrap();
    write_entry(&cwd,
        r#"import "std/_rt" as rt;
export type MainType = Dyn;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (0, fn(state, event) { (state, ['Output("dynamic"), 'Exit(0)]) })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "dynamic");
}

#[test]
fn custom_entry_rejects_a_mismatched_main_contract() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        "export def answer = \"no\";",
    )
    .unwrap();
    write_entry(&cwd,
        r#"import "std/_rt" as rt;
type Main = struct {answer: Int};
export type MainType = Main;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (main.answer, fn(state, event) { (state, ['Exit(1)]) })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("not assignable to Entry.MainType"));
}

#[test]
fn selected_entry_alone_can_import_dependency_private_modules() {
    let cwd = fixture();
    let dependency = cwd.join("dependency");
    fs::create_dir_all(dependency.join("src")).unwrap();
    fs::write(
        cwd.join("telora-config.json"),
        r#"{"version":1,"members":[".","dependency"]}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("src/_secret.telora"),
        "export def value = 42;",
    )
    .unwrap();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    fs::write(
        cwd.join("telora-crate.json"),
        r#"{"name":"app","modules":[],"dependencies":["dep"]}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("telora-crate.json"),
        r#"{"name":"dep","modules":["@src/_secret"],"dependencies":[]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("telora-lock.json"),
        r#"{"version":1,"packages":{"app":{"source":{"workspace":""},"modules":[],"dependencies":["dep"]},"dep":{"source":{"workspace":"dependency"},"modules":["@src/_secret"],"dependencies":[]}},"binaries":{"app/main":{"root":"app","packages":["app","dep"]}}}"#,
    )
    .unwrap();
    write_entry(&cwd,
        r#"import "std/_rt" as rt;
import "dep/_secret" {value};
type Main = struct {marker: Int};
export type MainType = Main;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (value, fn(state, event) {
        match event {
            'Initialize => (state, ['Output("42"), 'Exit(0)]),
            _ => fail!("unexpected event", event),
        }
    })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");

    fs::write(
        cwd.join("src/bin/main.telora"),
        "import \"dep/_secret\" {value}; export {value as output};",
    )
    .unwrap();
    let ordinary = telora(&cwd).args(["run", "main"]).output().unwrap();
    assert!(!ordinary.status.success());
    assert!(String::from_utf8_lossy(&ordinary.stderr).contains("private module"));
}

#[test]
fn entry_drives_a_stdio_child_through_host_events() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    write_entry(&cwd,
        r#"import "std/_rt" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = String;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    ("", fn(state, event) {
        match event {
            'Initialize => (state, ['SpawnStdioChild({
                key: "cat",
                opts: {bin: "/bin/cat", cwd: 'None, envs: {}, clear_env: 'False},
                stdio: {stdin: 'Piped, stdout: 'PipedToEnd, stderr: 'Null},
            })]),
            'ChildSpawnResult(spawned) => match spawned.result {
                'Ok(_) => (state, [
                    'PostStdin({key: "cat", data: 'Some("hello from child")}),
                    'PostStdin({key: "cat", data: 'None}),
                ]),
                'Err(error) => fail!("cannot spawn cat", error),
            },
            'ChildStdout(text) => match text.data {
                'Some(data) => (data, []),
                'None => (state, []),
            },
            'ChildStderr(_) => (state, []),
            'ChildExited(_) => (state, ['Output(state), 'Exit(0)]),
            'StdinLine(_) => fail!("unexpected stdin event"),
            'EesReply(_) => fail!("unexpected EES event"),
        }
    })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "hello from child");
}

#[cfg(unix)]
#[test]
fn child_capability_rejection_is_atomic_for_the_effect_batch() {
    use std::os::unix::fs::PermissionsExt;

    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    let marker = cwd.join("spawned");
    let child = cwd.join("should-not-run.sh");
    fs::write(&child, format!("#!/bin/sh\ntouch {:?}\n", marker)).unwrap();
    let mut permissions = fs::metadata(&child).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&child, permissions).unwrap();
    fs::write(
        cwd.join("src/entry/denied.telora"),
        format!(
        r#"import "std/_rt" as rt;
type Main = struct {{marker: Int}};
export type MainType = Main;
export type State = Int;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {{
        (
            {{data_srcs: {{}}, ees: {{}}, spawn_child: 'False, text_srcs: {{}}, vars: [], stdin: 'Null}},
            fn(resources, main) {{
                (0, fn(state, event) {{
                    (state, [
                        'Output("must not commit"),
                        'SpawnStdioChild({{
                            key: "denied",
                            opts: {{bin: {child:?}, cwd: 'None, envs: {{}}, clear_env: 'False}},
                            stdio: {{stdin: 'Null, stdout: 'Null, stderr: 'Null}},
                        }}),
                    ])
                }})
            }},
        )
    }};"#,
            child = child.to_string_lossy(),
        ),
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/denied", "main"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(run.stdout.is_empty());
    assert!(!marker.exists());
    assert!(String::from_utf8_lossy(&run.stderr).contains("without spawn_child capability"));
}

#[test]
fn child_spawn_failure_is_a_reducible_result_event() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    write_entry(&cwd,
        r#"import "std/_rt" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (main.marker, fn(state, event) {
        match event {
            'Initialize => (state, ['SpawnStdioChild({
                key: "missing",
                opts: {bin: "/telora/does/not/exist", cwd: 'None, envs: {}, clear_env: 'False},
                stdio: {stdin: 'Null, stdout: 'Null, stderr: 'Null},
            })]),
            'ChildSpawnResult({result: 'Err(error), key: _}) => (
                state,
                ['Output("spawn failure handled"), 'Exit(0)],
            ),
            'ChildSpawnResult({result: 'Ok(pid), key: _}) => fail!("unexpected spawn", pid),
            _ => fail!("unexpected event", event),
        }
    })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "spawn failure handled"
    );
}

#[cfg(unix)]
#[test]
fn entry_receives_line_stderr_eof_and_nonzero_child_exit_events() {
    use std::os::unix::fs::PermissionsExt;

    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    let child = cwd.join("child.sh");
    fs::write(
        &child,
        "#!/bin/sh\nprintf 'one\\ntwo\\n'\nprintf 'problem\\n' >&2\nexit 7\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&child).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&child, permissions).unwrap();
    write_entry(&cwd,
        format!(
        r#"import "std/_rt" as rt;
type Main = struct {{marker: Int}};
export type MainType = Main;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) {{ {{input: 'False}} }};
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {{
    (main.marker, fn(state, event) {{
        match event {{
            'Initialize => (state, ['SpawnStdioChild({{
                key: "worker",
                opts: {{bin: {child:?}, cwd: 'None, envs: {{}}, clear_env: 'False}},
                stdio: {{stdin: 'Null, stdout: 'PipedLine, stderr: 'PipedLine}},
            }})]),
            'ChildSpawnResult(spawned) => match spawned.result {{
                'Ok(_) => (state, []),
                'Err(error) => fail!("cannot spawn worker", error),
            }},
            'ChildStdout(text) => match text.data {{
                'Some(data) => (state, ['Output(`out:\{{data}}\\n`)]),
                'None => (state, []),
            }},
            'ChildStderr(text) => match text.data {{
                'Some(data) => (state, ['Output(`err:\{{data}}\\n`)]),
                'None => (state, []),
            }},
            'ChildExited(status) => (state, ['Output("exited"), 'Exit(0)]),
            'StdinLine(_) => fail!("unexpected stdin event"),
            'EesReply(_) => fail!("unexpected EES event"),
        }}
    }})
}};"#,
            child = child.to_string_lossy()
        ),
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let output = String::from_utf8_lossy(&run.stdout);
    assert!(output.contains("out:one\\n"), "{output:?}");
    assert!(output.contains("out:two\\n"), "{output:?}");
    assert!(output.contains("err:problem\\n"), "{output:?}");
    assert!(output.ends_with("exited"), "{output:?}");
}

#[cfg(unix)]
#[test]
fn exec_effect_replaces_the_telora_process() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    write_entry(&cwd,
        r#"import "std/_rt" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (main.marker, fn(state, event) {
        (state, ['Exec({bin: "/bin/true", cwd: 'None, envs: {}, clear_env: 'False})])
    })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(run.stdout.is_empty());
}

#[cfg(unix)]
#[test]
fn exit_waits_all_active_children_before_returning_the_status() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    let child = cwd.join("long-child.sh");
    fs::write(&child, "#!/bin/sh\nprintf '%s\\n' \"$$\"\nsleep 30\n").unwrap();
    let mut permissions = fs::metadata(&child).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&child, permissions).unwrap();
    write_entry(&cwd,
        format!(
        r#"import "std/_rt" as rt;
type Main = struct {{marker: Int}};
export type MainType = Main;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) {{ {{input: 'False}} }};
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {{
    (main.marker, fn(state, event) {{
        match event {{
            'Initialize => (state, ['SpawnStdioChild({{
                key: "long",
                opts: {{bin: {child:?}, cwd: 'None, envs: {{}}, clear_env: 'False}},
                stdio: {{stdin: 'Null, stdout: 'PipedLine, stderr: 'Null}},
            }})]),
            'ChildSpawnResult(spawned) => match spawned.result {{
                'Ok(_) => (state, []),
                'Err(error) => fail!("cannot spawn long child", error),
            }},
            'ChildStdout(text) => match text.data {{
                'Some(pid) => (state, ['Output(pid), 'Exit(7)]),
                'None => fail!("child ended before reporting its pid"),
            }},
            'ChildStderr(_) => (state, []),
            'ChildExited(_) => fail!("child exited before Entry requested Exit"),
            'StdinLine(_) => fail!("unexpected stdin event"),
            'EesReply(_) => fail!("unexpected EES event"),
        }}
    }})
}};"#,
            child = child.to_string_lossy()
        ),
    )
    .unwrap();
    let started = Instant::now();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(7));
    assert!(started.elapsed() < Duration::from_secs(5));
    let pid = String::from_utf8(run.stdout).unwrap();
    assert!(!pid.is_empty());
    let alive = Command::new("kill").args(["-0", &pid]).output().unwrap();
    assert!(
        !alive.status.success(),
        "child {pid} remained alive after Exit(7)"
    );
}

#[cfg(unix)]
#[test]
fn blocked_child_stdin_does_not_block_unrelated_events_or_exit() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    let blocked = cwd.join("blocked.sh");
    let reporter = cwd.join("reporter.sh");
    fs::write(&blocked, "#!/bin/sh\nsleep 30\n").unwrap();
    fs::write(&reporter, "#!/bin/sh\nprintf 'ready\\n'\n").unwrap();
    for child in [&blocked, &reporter] {
        let mut permissions = fs::metadata(child).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(child, permissions).unwrap();
    }
    let payload = "x".repeat(256 * 1024);
    write_entry(&cwd,
        format!(
        r#"import "std/_rt" as rt;
type Main = struct {{marker: Int}};
export type MainType = Main;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) {{ {{input: 'False}} }};
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {{
    (main.marker, fn(state, event) {{
        match event {{
            'Initialize => (state, [
                'SpawnStdioChild({{
                    key: "blocked",
                    opts: {{bin: {blocked:?}, cwd: 'None, envs: {{}}, clear_env: 'False}},
                    stdio: {{stdin: 'Piped, stdout: 'Null, stderr: 'Null}},
                }}),
                'SpawnStdioChild({{
                    key: "reporter",
                    opts: {{bin: {reporter:?}, cwd: 'None, envs: {{}}, clear_env: 'False}},
                    stdio: {{stdin: 'Null, stdout: 'PipedLine, stderr: 'Null}},
                }}),
            ]),
            'ChildSpawnResult({{key: "blocked", result: 'Ok(_)}}) => (
                state,
                ['PostStdin({{key: "blocked", data: 'Some({payload:?})}})],
            ),
            'ChildSpawnResult({{result: 'Err(error), key: _}}) => fail!("spawn failed", error),
            'ChildSpawnResult(_) => (state, []),
            'ChildStdout(text) => match text.data {{
                'Some("ready") => (state, ['Output("ready"), 'Exit(0)]),
                _ => (state, []),
            }},
            'ChildStderr(_) => (state, []),
            'ChildExited(_) => (state, []),
            'StdinLine(_) => fail!("unexpected stdin event"),
            'EesReply(_) => fail!("unexpected EES event"),
        }}
    }})
}};"#,
            blocked = blocked.to_string_lossy(),
            reporter = reporter.to_string_lossy(),
        ),
    )
    .unwrap();
    let started = Instant::now();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "ready");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn entry_protocol_failures_commit_no_buffered_output() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    let template = r#"import "std/_rt" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = Int;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (main.marker, fn(state, event) { (state, EFFECTS) })
};"#;
    for (effects, message) in [
        (
            "['Output(\"partial\"), 'Exit(0), 'Output(\"late\")]",
            "effect after a terminal effect",
        ),
        ("['Output(\"partial\")]", "made no progress"),
    ] {
        write_entry(&cwd, template.replace("EFFECTS", effects)).unwrap();
        let run = telora(&cwd)
            .args(["run-with", "@src/entry/test", "main"])
            .output()
            .unwrap();
        assert!(!run.status.success(), "{effects}");
        assert!(run.stdout.is_empty(), "{effects}");
        assert!(
            String::from_utf8_lossy(&run.stderr).contains(message),
            "{effects}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
}

#[test]
fn entry_rejects_extra_public_protocol_members() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    write_entry(&cwd,
        r#"import "std/_rt" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = Int;
export def typo = 1;
def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
def legacy_initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (main.marker, fn(state, event) { (state, ['Exit(1)]) })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/entry/test", "main"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("must export exactly"));
}
