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
    fs::write(cwd.join("src/lib.telora"), "export let output = 41 + 1;").unwrap();
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
fn run_writes_contextual_debug_as_stderr_jsonl() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        "let var = 3; export let output = var.dbg!(\"observed\");",
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
    fs::write(cwd.join("src/bin/main.telora"), "export {input as output};").unwrap();
    fs::write(cwd.join("input.json"), "[1,2,3]").unwrap();
    let run = telora(&cwd)
        .args(["run", "main", "--input", "input.json"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "[1, 2, 3]");
}

#[test]
fn run_context_selects_the_manifest_discovery_start() {
    let cwd = fixture();
    let other = fixture();
    fs::write(other.join("src/bin/tool.telora"), "export let output = 9;").unwrap();
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
        "export let value = 12;",
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
fn build_dry_run_keeps_the_pure_output_protocol() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/build.telora"), r#"let build = fn() { {files: ['TextFile({content: "ok", path: "out.txt"})]} }; export {build};"#).unwrap();
    let build = telora(&cwd)
        .args(["build", "--dry-run", "@bin/build.telora"])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&build.stdout).trim(),
        r#"{"files":[{"TextFile":{"content":"ok","path":"out.txt"}}]}"#
    );
    assert!(!cwd.join("out.txt").exists());
}
