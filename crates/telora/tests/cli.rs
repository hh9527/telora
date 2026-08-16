use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("telora-cli-{unique}"));
    fs::create_dir_all(path.join("src/bin")).unwrap();
    fs::create_dir_all(path.join("tests")).unwrap();
    fs::write(path.join("telora-deps.json"), "{}").unwrap();
    path
}

fn telora(cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_telora"));
    command.current_dir(cwd);
    command
}

fn jsonl(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn help_is_clap_owned_and_types_is_removed() {
    let cwd = fixture();
    let help = telora(&cwd).arg("--help").output().unwrap();
    let output = String::from_utf8_lossy(&help.stdout);
    assert!(help.status.success());
    assert!(output.contains("lsp"));
    assert!(!output.contains("types"));
    let types = telora(&cwd)
        .args(["types", "@src/lib.telora"])
        .output()
        .unwrap();
    assert!(!types.status.success());
}

#[test]
fn run_and_check_select_logical_roots_from_cwd() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "export let output = \"42\";").unwrap();
    fs::write(
        cwd.join("src/bin/main.telora"),
        "import \"@src/lib.telora\" {output}; export {output};",
    )
    .unwrap();
    let nested = cwd.join("src/bin");
    let run = telora(&nested).args(["run", "main"]).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");
    let check = telora(&nested)
        .args(["check", "@src/lib.telora"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn check_requires_complete_runtime_finalization() {
    let cwd = fixture();
    let cases = [
        ("failed", "export let output = fail!(\"boom\", 1);"),
        ("division", "export let output = 1 / 0;"),
        ("index", "export let output = [1][2];"),
    ];
    for (name, source) in cases {
        fs::write(cwd.join(format!("src/{name}.telora")), source).unwrap();
        let module_id = format!("@src/{name}.telora");
        let check = telora(&cwd)
            .args(["check", module_id.as_str()])
            .output()
            .unwrap();
        assert!(
            !check.status.success(),
            "{name} unexpectedly passed: {}",
            String::from_utf8_lossy(&check.stdout)
        );
        let records = jsonl(&check.stdout);
        assert!(
            records
                .iter()
                .any(|record| record["record"] == "diagnostic"),
            "{name} emitted no diagnostic"
        );
        assert_eq!(records.last().unwrap()["record"], "summary");
        assert_eq!(records.last().unwrap()["status"], "error");
        assert!(check.stderr.is_empty(), "{name} mixed text into stderr");
    }
}

#[test]
fn check_accepts_a_complete_module_with_warnings() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/warning.telora"),
        "let reject: Fn() -> Result(Int, String) = fn() { 'Err(\"notice\") }; let checked = reject.should_ok!(); export let output = 1;",
    )
    .unwrap();
    let check = telora(&cwd)
        .args(["check", "@src/warning.telora"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let records = jsonl(&check.stdout);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema"], "telora.check/v1");
    assert_eq!(records[0]["module"], "@src/warning.telora");
    assert_eq!(records[0]["record"], "diagnostic");
    assert_eq!(records[0]["severity"], "warning");
    assert_eq!(records[1]["record"], "summary");
    assert_eq!(records[1]["status"], "ok");
    assert_eq!(records[1]["dependencies"], 0);
}

#[test]
fn run_writes_contextual_debug_as_stderr_jsonl() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        "let var = 3; let observed = var.dbg!(\"observed\"); export let output = \"3\";",
    )
    .unwrap();
    let run = telora(&cwd).args(["run", "main"]).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "3");
    let records = jsonl(&run.stderr);
    assert_eq!(
        records.len(),
        2,
        "module initialization and execution observe once each"
    );
    for record in records {
        assert_eq!(record["name"], "var");
        assert_eq!(record["repr"], "3");
        assert_eq!(record["module"], "@bin/main.telora");
        assert_eq!(record["line"], 1);
        assert_eq!(record["message"], "observed");
    }
}

#[test]
fn best_effort_run_collects_main_diagnostics_before_starting_entry() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/array" as array;
def transform: Fn(Int) -> Int = fn(item) {
    if item == 2 { fail!("two", item) }
    else if item == 4 { fail!("four", item) }
    else { item }
};
let broken = array.map([1, 2, 3, 4], transform);
export let output = if array.length(broken) > 0 { "unexpected" } else { "empty" };"#,
    )
    .unwrap();

    let run = telora(&cwd)
        .args(["run", "main", "--best-effort"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(run.stdout.is_empty(), "Entry started for an invalid Main");
    let records = jsonl(&run.stderr);
    assert!(records.iter().any(|record| record["message"] == "two"));
    assert!(records.iter().any(|record| record["message"] == "four"));
    assert_eq!(records.last().unwrap()["schema"], "telora.run/v1");
    assert_eq!(records.last().unwrap()["record"], "summary");
    assert_eq!(records.last().unwrap()["status"], "error");
}

#[test]
fn recursive_type_metadata_does_not_add_recovery_errors() {
    let cwd = fixture();
    let source = r#"@struct type CallExpr = { args: Array(Expr) };
@enum type Expr = { Call: CallExpr, Text: String };
def reject: Fn(Int) -> Expr = fn(value) { fail!("expected failure", value) };
let failed = reject(1);
export let output = "unreachable";"#;
    fs::write(cwd.join("src/bin/main.telora"), source).unwrap();

    let check = telora(&cwd)
        .args(["check", "@bin/main.telora"])
        .output()
        .unwrap();
    assert!(!check.status.success());
    let check_records = jsonl(&check.stdout);
    assert!(
        check_records
            .iter()
            .any(|record| record["message"] == "expected failure")
    );
    assert!(!check_records.iter().any(|record| {
        record["message"]
            .as_str()
            .is_some_and(|message| message.contains("cannot be partially evaluated"))
    }));

    let run = telora(&cwd)
        .args(["run", "main", "--best-effort"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(run.stdout.is_empty());
    let run_records = jsonl(&run.stderr);
    assert!(
        run_records
            .iter()
            .any(|record| record["message"] == "expected failure")
    );
    assert!(!run_records.iter().any(|record| {
        record["message"]
            .as_str()
            .is_some_and(|message| message.contains("cannot be partially evaluated"))
    }));
}

#[test]
fn public_cli_rejects_physical_paths_and_missing_manifests() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "export let output = 1;").unwrap();
    let physical = telora(&cwd)
        .args(["run", "src/lib.telora"])
        .output()
        .unwrap();
    assert!(!physical.status.success());
    let outside = fixture();
    fs::remove_file(outside.join("telora-deps.json")).unwrap();
    let missing = telora(&outside)
        .args(["check", "@src/lib.telora"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("cannot find telora-deps.json"));
}

#[test]
fn show_named_queries_emit_stable_jsonl() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "type Name = String; let hidden = 1; def make: Fn(Int) -> Int = fn(value) { value }; export {Name, make};").unwrap();
    let show = telora(&cwd)
        .args(["show", "@src/lib.telora", "-p", "a", "-k", "type,def"])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let records = jsonl(&show.stdout);
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(
        |record| record["schema"] == "telora.show/v1" && record["module"] == "@src/lib.telora"
    ));
    assert_eq!(records[0]["name"], "Name");
    assert_eq!(records[1]["name"], "make");

    let exports = telora(&cwd)
        .args(["show", "@src/lib.telora", "--exports"])
        .output()
        .unwrap();
    let records = jsonl(&exports.stdout);
    assert_eq!(
        records
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Name", "make"]
    );
}

#[test]
fn show_namespace_imports_reference_exact_module_interfaces() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/types.telora"),
        "@struct type CallExpr = {args: Array(Expr)};\n@enum type Expr = {Text: String, Call: CallExpr};\n@struct type Box(A) = {value: A};\nexport {CallExpr, Expr, Box};\n",
    )
    .unwrap();
    fs::write(
        cwd.join("src/lib.telora"),
        "import \"@src/types.telora\" as types;\nimport \"@src/types.telora\" { Expr };\nexport {types, Expr};\n",
    )
    .unwrap();

    let show = telora(&cwd)
        .args(["show", "@src/lib.telora", "-k", "import"])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let records = jsonl(&show.stdout);
    let namespace = records
        .iter()
        .find(|record| record["name"] == "types")
        .unwrap();
    assert_eq!(namespace["authority"], "authoritative");
    assert_eq!(namespace["target"], "@src/types.telora");
    assert!(namespace.get("type").is_none());

    let selective = records
        .iter()
        .find(|record| record["name"] == "Expr")
        .unwrap();
    let ty = selective["type"].as_str().unwrap();
    assert!(ty.contains("TypeOf(enum"), "{ty}");
    assert!(!ty.contains("Any"), "{ty}");
}

#[test]
fn show_position_and_conflicts_are_structured() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/lib.telora"),
        "let answer = 42;\nexport {answer};\n",
    )
    .unwrap();
    let at = telora(&cwd)
        .args(["show", "@src/lib.telora", "--at", "1:5"])
        .output()
        .unwrap();
    assert!(
        at.status.success(),
        "{}",
        String::from_utf8_lossy(&at.stderr)
    );
    assert!(
        jsonl(&at.stdout)
            .iter()
            .any(|record| record["record"] == "definition" && record["name"] == "answer")
    );
    let conflict = telora(&cwd)
        .args(["show", "@src/lib.telora", "--at", "1", "-p", "a"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    let bad_kind = telora(&cwd)
        .args(["show", "@src/lib.telora", "-k", "let,"])
        .output()
        .unwrap();
    assert!(!bad_kind.status.success());
}

#[test]
fn test_roots_are_selectable_but_not_importable() {
    let cwd = fixture();
    fs::write(cwd.join("tests/codec.telora"), "export let output = 7;").unwrap();
    let run = telora(&cwd)
        .args(["check", "@test/codec.telora"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    fs::write(
        cwd.join("src/lib.telora"),
        "import \"@test/codec.telora\" as codec; export let output = codec;",
    )
    .unwrap();
    let check = telora(&cwd)
        .args(["check", "@src/lib.telora"])
        .output()
        .unwrap();
    assert!(!check.status.success());
}

#[test]
fn run_accepts_external_json() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        "export let output: String = input;",
    )
    .unwrap();
    fs::write(cwd.join("input.json"), r#""accepted""#).unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--input", "input.json"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "accepted");
}

#[test]
fn run_context_selects_the_manifest_discovery_start() {
    let cwd = fixture();
    let other = fixture();
    fs::write(
        other.join("src/bin/tool.telora"),
        "export let output = \"9\";",
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run", "tool", "-C", other.to_str().unwrap()])
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
fn check_and_show_context_select_the_manifest_discovery_start() {
    let cwd = fixture();
    let other = fixture();
    fs::write(
        other.join("src/lib.telora"),
        "type Answer = Int; export {Answer};",
    )
    .unwrap();

    let check = telora(&cwd)
        .args(["check", "@src/lib.telora", "-C", other.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );

    let show = telora(&cwd)
        .args([
            "show",
            "@src/lib.telora",
            "-C",
            other.to_str().unwrap(),
            "--exports",
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
}

#[test]
fn standalone_run_uses_only_embedded_dependency_options() {
    let cwd = fixture();
    let dependency = cwd.join("dep");
    fs::create_dir_all(dependency.join("src")).unwrap();
    fs::write(
        dependency.join("src/value.telora"),
        "export let value = \"12\";",
    )
    .unwrap();
    let standalone = cwd.join("standalone.telora");
    fs::write(&standalone, r#"option "crate.dependency" {name: "dep", source: 'Path({path: "dep"})}; import "dep/value.telora" {value}; export {value as output};"#).unwrap();
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
    fs::write(cwd.join("src/bin/main.telora"), "export let answer = 42;").unwrap();
    fs::write(
        cwd.join("entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
@struct type Main = {answer: Int};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
type Transition = Tuple([State, Array(rt.SystemEffect)]);
type Reducer = Fn(State, rt.SystemEvent) -> Transition;
export def initialize: Fn(MainType) -> Tuple([State, Reducer]) = fn(main) {
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
        .args(["run", "main", "--entry", "entry.telora"])
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
fn custom_entry_can_choose_a_dynamic_main_contract() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export let answer = 42;").unwrap();
    fs::write(
        cwd.join("entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
export type MainType = Dyn;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (0, fn(state, event) { (state, ['Output("dynamic"), 'Exit(0)]) })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--entry", "entry.telora"])
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
        "export let answer = \"no\";",
    )
    .unwrap();
    fs::write(
        cwd.join("entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
@struct type Main = {answer: Int};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (main.answer, fn(state, event) { (state, ['Exit(1)]) })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--entry", "entry.telora"])
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
        cwd.join("telora-deps.json"),
        r#"{"dependencies":{"dep":{"path":"dependency"}}}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("src/secret.priv.telora"),
        "export let value = 42;",
    )
    .unwrap();
    fs::write(cwd.join("src/bin/main.telora"), "export let marker = 0;").unwrap();
    fs::write(
        cwd.join("entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
import "dep/secret.priv.telora" {value};
@struct type Main = {marker: Int};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
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
        .args(["run", "main", "--entry", "entry.telora"])
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
        "import \"dep/secret.priv.telora\" {value}; export {value as output};",
    )
    .unwrap();
    let ordinary = telora(&cwd).args(["run", "main"]).output().unwrap();
    assert!(!ordinary.status.success());
    assert!(String::from_utf8_lossy(&ordinary.stderr).contains("private module"));
}

#[test]
fn entry_input_capability_is_required_before_main_initialization() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export {input as output};").unwrap();
    fs::write(
        cwd.join("entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
@struct type Main = {output: Int};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'True} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (0, fn(state, event) { (state, ['Exit(1)]) })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--entry", "entry.telora"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("received no --input"));
}

#[test]
fn entry_drives_a_stdio_child_through_host_events() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export let marker = 0;").unwrap();
    fs::write(
        cwd.join("entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
@struct type Main = {marker: Int};
export type MainType = Main;
export type State = String;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
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
        }
    })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--entry", "entry.telora"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "hello from child");
}

#[test]
fn child_spawn_failure_is_a_reducible_result_event() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export let marker = 0;").unwrap();
    fs::write(
        cwd.join("entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
@struct type Main = {marker: Int};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
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
        .args(["run", "main", "--entry", "entry.telora"])
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
    fs::write(cwd.join("src/bin/main.telora"), "export let marker = 0;").unwrap();
    let child = cwd.join("child.sh");
    fs::write(
        &child,
        "#!/bin/sh\nprintf 'one\\ntwo\\n'\nprintf 'problem\\n' >&2\nexit 7\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&child).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&child, permissions).unwrap();
    fs::write(
        cwd.join("entry.telora"),
        format!(
            r#"import "std/rt.priv.telora" as rt;
@struct type Main = {{marker: Int}};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) {{ {{input: 'False}} }};
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {{
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
        }}
    }})
}};"#,
            child = child.to_string_lossy()
        ),
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--entry", "entry.telora"])
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
    fs::write(cwd.join("src/bin/main.telora"), "export let marker = 0;").unwrap();
    fs::write(
        cwd.join("entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
@struct type Main = {marker: Int};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (main.marker, fn(state, event) {
        (state, ['Exec({bin: "/bin/true", cwd: 'None, envs: {}, clear_env: 'False})])
    })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--entry", "entry.telora"])
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
    fs::write(cwd.join("src/bin/main.telora"), "export let marker = 0;").unwrap();
    let child = cwd.join("long-child.sh");
    fs::write(&child, "#!/bin/sh\nprintf '%s\\n' \"$$\"\nsleep 30\n").unwrap();
    let mut permissions = fs::metadata(&child).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&child, permissions).unwrap();
    fs::write(
        cwd.join("entry.telora"),
        format!(
            r#"import "std/rt.priv.telora" as rt;
@struct type Main = {{marker: Int}};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) {{ {{input: 'False}} }};
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {{
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
        }}
    }})
}};"#,
            child = child.to_string_lossy()
        ),
    )
    .unwrap();
    let started = Instant::now();
    let run = telora(&cwd)
        .args(["run", "main", "--entry", "entry.telora"])
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
    fs::write(cwd.join("src/bin/main.telora"), "export let marker = 0;").unwrap();
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
    fs::write(
        cwd.join("entry.telora"),
        format!(
            r#"import "std/rt.priv.telora" as rt;
@struct type Main = {{marker: Int}};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) {{ {{input: 'False}} }};
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {{
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
        .args(["run", "main", "--entry", "entry.telora"])
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
    fs::write(cwd.join("src/bin/main.telora"), "export let marker = 0;").unwrap();
    let template = r#"import "std/rt.priv.telora" as rt;
@struct type Main = {marker: Int};
export type MainType = Main;
export type State = Int;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (main.marker, fn(state, event) { (state, EFFECTS) })
};"#;
    for (effects, message) in [
        (
            "['Output(\"partial\"), 'Exit(0), 'Output(\"late\")]",
            "effect after a terminal effect",
        ),
        ("['Output(\"partial\")]", "made no progress"),
    ] {
        fs::write(
            cwd.join("entry.telora"),
            template.replace("EFFECTS", effects),
        )
        .unwrap();
        let run = telora(&cwd)
            .args(["run", "main", "--entry", "entry.telora"])
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
    fs::write(cwd.join("src/bin/main.telora"), "export let marker = 0;").unwrap();
    fs::write(
        cwd.join("entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
@struct type Main = {marker: Int};
export type MainType = Main;
export type State = Int;
export let typo = 1;
export def prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };
export def initialize: Fn(MainType) -> Tuple([State, Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)])]) = fn(main) {
    (main.marker, fn(state, event) { (state, ['Exit(1)]) })
};"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--entry", "entry.telora"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("must export exactly"));
}
