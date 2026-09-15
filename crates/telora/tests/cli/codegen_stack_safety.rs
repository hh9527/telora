use super::*;

#[test]
fn codegen_handles_long_chains_on_a_small_stack() {
    let cwd = fixture();
    let template = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codegen-stack/value.telora"),
    )
    .unwrap();
    // These flat forms are accepted by the existing parser. Parser depth
    // guards and grammar changes are deliberately not prerequisites here.
    for value in [
        format!("{}1", "1 + ".repeat(2000)),
        format!("1{}", ".ty!(Int)".repeat(2000)),
        format!("1{}", ".dbg!()".repeat(2000)),
        format!("1{}", " |> fn(x) { x }".repeat(2000)),
        format!(
            "do {{ def ident: Fn(Int) -> Int = fn(x) {{ x }}; ident{}(1) }}",
            "\\(_)".repeat(2000)
        ),
        format!(
            "do {{ type R = struct {{ a: Int }}; let r: R = {{ a: 1 }}; (r{}).a }}",
            " <~ { a: 1 }".repeat(2000)
        ),
        format!(
            "do {{ type R = struct {{ a: Int }}; let r: R = {{ a: 1 }}; r{}.a }}",
            ".{a}.ty!(R)".repeat(2000)
        ),
        format!(
            "do {{ type R = struct {{ next: Fn() -> Array(R), a: Int }}; def make: Fn() -> Array(R) = fn() {{ [{{ next: make, a: 1 }}] }}; make()[0]{}.a }}",
            ".next()[0]".repeat(2000)
        ),
    ] {
        fs::write(
            cwd.join("src/main.telora"),
            template.replace("{{VALUE}}", &value),
        )
        .unwrap();
        refresh_fixture_workspace(&cwd);
        #[cfg(target_os = "linux")]
        let mut command = {
            let mut command = Command::new("sh");
            command.args([
                "-c",
                "ulimit -s 1024; exec \"$@\"",
                "telora-codegen-stack",
                env!("CARGO_BIN_EXE_telora"),
            ]);
            command
        };
        #[cfg(not(target_os = "linux"))]
        let mut command = Command::new(env!("CARGO_BIN_EXE_telora"));
        let output = command
            .current_dir(&cwd)
            .args(["check", "@src/main"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "check failed for {}: {}\n{}",
            &value[..value.len().min(120)],
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let records = jsonl(&output.stdout);
        assert_eq!(records.last().unwrap()["status"], "ok");
    }
    fs::remove_dir_all(cwd).unwrap();
}
