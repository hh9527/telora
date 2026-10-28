use super::*;

#[test]
fn execution_commands_reject_removed_backend_and_artifact_interfaces() {
    let cwd = fixture();
    for (command, selector) in [
        ("check", "@src/main"),
        ("eval", "@src/main:answer"),
        ("run", "@src/main:run"),
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

#[test]
fn javascript_build_uses_the_workspace_frontend_and_runs_in_node() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        "pub def answer: Fn(Int) -> Int = fn(x) { x + 1 };",
    )
    .unwrap();
    let artifact = cwd.join("model.mjs");
    let output = telora(&cwd)
        .args(["build", "--backend", "js", "@src/main:answer", "-o"])
        .arg(&artifact)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let script = format!(
        "import answer from {}; if(answer(41n)!==42n)throw Error('wrong result');",
        serde_json::to_string(&format!("file://{}", artifact.display())).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "-e", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn staged_javascript_build_publishes_a_closed_runtime_atomically() {
    if std::env::var_os("TELORA_JS_HARD_LIMIT_TESTS").is_none() {
        return;
    }
    let cwd = fixture();
    let artifact = cwd.join("staged.mjs");
    fs::write(
        cwd.join("src/main.telora"),
        r#"
use std::build as build;
use std::codec as codec;
pub def answer: Fn(Int) -> Int = build::stage(
    fn() {let offset=7; {runtime:fn(x:Int){x+offset},information:{"example":codec::encode(True)}}},
    fn(prepared){prepared}
);
"#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args([
            "build",
            "--backend",
            "js",
            "--staged",
            "@src/main:answer",
            "-o",
        ])
        .arg(&artifact)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let script = format!(
        "import answer,{{buildInformation,createService}} from {};if(answer(5n)!==12n||buildInformation.example!==true)throw Error('staged mismatch');",
        serde_json::to_string(&format!("file://{}", artifact.display())).unwrap()
    );
    let output = Command::new("node")
        .args(["--input-type=module", "-e", &script])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let previous = fs::read(&artifact).unwrap();
    fs::write(cwd.join("src/main.telora"),r#"
use std::build as build;
pub def answer: Int = build::stage(fn(){if True {fail!("prepare failed")} else {{runtime:7,information:{}}}},fn(x){x});
"#).unwrap();
    let output = telora(&cwd)
        .args([
            "build",
            "--backend",
            "js",
            "--staged",
            "@src/main:answer",
            "-o",
        ])
        .arg(&artifact)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("prepare failed"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(previous, fs::read(&artifact).unwrap());
    fs::remove_dir_all(cwd).unwrap();
}
