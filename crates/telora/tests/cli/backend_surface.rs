use super::*;

#[test]
fn execution_commands_reject_removed_backend_and_artifact_interfaces() {
    let cwd = fixture();
    for (command, selector) in [
        ("check", "@src/main"),
        ("eval", "@src/main:answer"),
        ("run", "@src/main:run"),
        ("serve", "@src/main:serve"),
        ("test", "main"),
    ] {
        for flag in ["--native", "--wasm", "--best-effort"] {
            let output = telora(&cwd)
                .args([command, flag, selector])
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(2));
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains("unexpected argument") && error.contains(flag),
                "{error}"
            );
            assert!(output.stdout.is_empty());
        }
    }
    let output = telora(&cwd)
        .args(["wasm", "build", "@src/main:answer"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
    assert!(output.stdout.is_empty());
    fs::remove_dir_all(cwd).unwrap();
}
