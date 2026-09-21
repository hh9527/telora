use super::*;

#[test]
fn help_lists_the_public_command_surface() {
    let cwd = fixture();
    let help = telora(&cwd).arg("--help").output().unwrap();
    let output = String::from_utf8_lossy(&help.stdout);
    assert!(help.status.success());
    assert!(output.contains("lsp"));
    assert!(!output.contains("ees"));
    assert!(output.contains("query"));
    assert!(output.contains("q"));
    let query_help = telora(&cwd).args(["query", "-h"]).output().unwrap();
    let output = String::from_utf8_lossy(&query_help.stdout);
    assert!(query_help.status.success());
    assert!(output.contains("modules"));
    assert!(output.contains("exports"));
    assert!(output.contains("at"));
    assert!(output.contains("telora q modules -p std/"));
}

#[test]
fn run_and_check_select_logical_roots_from_cwd() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/lib.telora"),
        "export def output: String = \"42\";",
    )
    .unwrap();
    fs::write(
        cwd.join("src/app.telora"),
        runtime_source("transform-import.telora"),
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);
    let run = execute_value(&cwd, "run", "@src/app");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "\"42\"");
    let check = telora(&cwd).args(["check", "@src/lib"]).output().unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn public_cli_rejects_physical_paths_and_missing_manifests() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "export def output: Int = 1;").unwrap();
    let physical = telora(&cwd)
        .args(["run", "src/lib.telora"])
        .output()
        .unwrap();
    assert!(!physical.status.success());
    let outside = fixture();
    fs::remove_file(outside.join("telora-config.json")).unwrap();
    let missing = telora(&outside)
        .args(["check", "@src/lib"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("cannot find telora-config.json"));
}

#[test]
fn test_roots_are_selectable_but_not_importable() {
    let cwd = fixture();
    fs::write(
        cwd.join("tests/codec.telora"),
        "export def output: Int = 7;",
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["check", "@test/codec"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    fs::write(
        cwd.join("src/lib.telora"),
        "import \"@test/codec\" as codec; export def output = codec;",
    )
    .unwrap();
    let check = telora(&cwd).args(["check", "@src/lib"]).output().unwrap();
    assert!(!check.status.success());
}
