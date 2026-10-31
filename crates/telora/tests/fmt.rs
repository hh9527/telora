use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn cli(root: &Path) -> Command {
    let mut cli = Command::new(env!("CARGO_BIN_EXE_telora"));
    cli.current_dir(root);
    cli
}
fn stdin(root: &Path, args: &[&str], input: &str) -> Output {
    let mut process = cli(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    process
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    process.wait_with_output().unwrap()
}
#[test]
fn stdin_is_plain_source_and_does_not_need_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let output = stdin(dir.path(), &["fmt", "-"], "pub def x=1+2;");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"pub def x = 1 + 2;\n");
    assert!(output.stderr.is_empty());
    let dirty = stdin(dir.path(), &["fmt", "--check", "-"], "def x=1;");
    assert_eq!(dirty.status.code(), Some(1));
    assert!(dirty.stdout.is_empty());
    let clean = stdin(dir.path(), &["fmt", "--check", "-"], "def x = 1;\n");
    assert!(clean.status.success());
}
#[test]
fn recursive_in_place_check_and_stdout_modes() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/sub")).unwrap();
    fs::create_dir_all(dir.path().join("target")).unwrap();
    fs::write(dir.path().join("src/a.telora"), "def a=1;").unwrap();
    fs::write(dir.path().join("src/sub/b.telora"), "def b=2;").unwrap();
    fs::write(dir.path().join("target/ignored.telora"), "not valid").unwrap();
    fs::write(dir.path().join("src/data.json"), "{}").unwrap();
    let dirty = cli(dir.path()).args(["fmt", "--check"]).output().unwrap();
    assert_eq!(dirty.status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(dir.path().join("src/a.telora")).unwrap(),
        "def a=1;"
    );
    let stdout = cli(dir.path())
        .args(["fmt", "--stdout", "src/a.telora"])
        .output()
        .unwrap();
    assert!(stdout.status.success());
    assert_eq!(stdout.stdout, b"def a = 1;\n");
    assert_eq!(
        fs::read_to_string(dir.path().join("src/a.telora")).unwrap(),
        "def a=1;"
    );
    let written = cli(dir.path())
        .args(["fmt", "src", "src/a.telora"])
        .output()
        .unwrap();
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    let clean = cli(dir.path()).args(["fmt", "--check"]).output().unwrap();
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("src/sub/b.telora")).unwrap(),
        "def b = 2;\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("src/data.json")).unwrap(),
        "{}"
    );
}
#[test]
fn invalid_batch_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.telora"), "def a=1;").unwrap();
    fs::write(dir.path().join("b.telora"), "def b=[;").unwrap();
    let output = cli(dir.path()).args(["fmt"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("b.telora"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.telora")).unwrap(),
        "def a=1;"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("b.telora")).unwrap(),
        "def b=[;"
    );
}
#[test]
fn argument_errors_and_context() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.telora"), "def a=1;").unwrap();
    for args in [
        vec!["fmt", "-", "a.telora"],
        vec!["fmt", "--check", "--stdout"],
        vec!["fmt", "missing.telora"],
        vec!["fmt", "--width", "0"],
    ] {
        assert!(
            !cli(dir.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_telora"))
        .arg("-C")
        .arg(dir.path())
        .args(["fmt", "--stdout", "a.telora"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"def a = 1;\n");
}
#[test]
fn width_controls_collection_layout() {
    let dir = tempfile::tempdir().unwrap();
    let output = stdin(
        dir.path(),
        &["fmt", "--width", "20", "-"],
        "def x=[1111,2222,3333,4444];",
    );
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("[\n    1111,\n"));
}
#[cfg(unix)]
#[test]
fn recursive_walk_skips_symlinks_and_preserves_file_mode() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::write(external.path().join("outside.telora"), "def outside=0;").unwrap();
    fs::write(dir.path().join("a.telora"), "def a=1;").unwrap();
    fs::set_permissions(
        dir.path().join("a.telora"),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    symlink(external.path(), dir.path().join("linked")).unwrap();
    symlink(dir.path(), dir.path().join("cycle")).unwrap();
    let output = cli(dir.path()).args(["fmt"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(external.path().join("outside.telora")).unwrap(),
        "def outside=0;"
    );
    assert_eq!(
        fs::metadata(dir.path().join("a.telora"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
}
