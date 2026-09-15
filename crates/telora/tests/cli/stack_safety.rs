use super::*;

fn template(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/stack-safety")
            .join(name),
    )
    .unwrap()
}

fn check(cwd: &Path, only_types: bool, error: Option<&str>) {
    eprintln!("stack check: {cwd:?}, only_types={only_types}, expected={error:?}");
    refresh_fixture_workspace(cwd);
    // Linux main normally has a larger stack than Windows. Exercise the actual
    // binary with a 1 MiB main stack, not just a small-stack parser probe.
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "ulimit -s 1024; exec \"$@\"",
            "telora-stack",
            env!("CARGO_BIN_EXE_telora"),
        ]);
        command
    };
    #[cfg(not(target_os = "linux"))]
    let mut command = Command::new(env!("CARGO_BIN_EXE_telora"));
    command.current_dir(cwd).args(["check", "@src/main"]);
    if only_types {
        command.arg("--only-types");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.code().is_some(),
        "process crashed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = jsonl(&output.stdout);
    assert_eq!(
        output.status.success(),
        error.is_none(),
        "only_types={only_types}: {records:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        records.last().unwrap()["status"],
        if error.is_some() { "error" } else { "ok" }
    );
    if let Some(message) = error {
        assert!(
            records.iter().any(
                |r| r["message"].as_str().is_some_and(|m| m.contains(message))
                    && r["labels"]
                        .as_array()
                        .is_some_and(|labels| !labels.is_empty())
            ),
            "{records:?}"
        );
    }
}

#[test]
fn full_check_and_types_only_are_stack_safe_for_syntax_boundaries() {
    let cwd = fixture();
    for depth in [32, 33, 180, 2000] {
        let source = template("nested-functions.telora")
            .replace("{{TYPE}}", &format!("{}Int", "Fn(Int) -> ".repeat(depth)))
            .replace(
                "{{VALUE}}",
                &format!("{}0{}", "fn(x) { ".repeat(depth), " }".repeat(depth)),
            );
        fs::write(cwd.join("src/main.telora"), source).unwrap();
        for only_types in [true, false] {
            check(
                &cwd,
                only_types,
                (depth > 32).then_some("syntax nesting exceeds parser limit"),
            );
        }
    }
    for (value, error) in [
        (format!("{}1", "-".repeat(2000)), None),
        (format!("{}1", "1 + ".repeat(2000)), None),
        (format!("do {{ {}1 }}", "1; ".repeat(2000)), None),
        (
            format!("{}{{ 0 }}", "if False { 1 } else ".repeat(2000)),
            None,
        ),
        (
            format!(
                "{}{{ 0 }}",
                "if let Some(x) = Some(1) { x } else ".repeat(2000)
            ),
            None,
        ),
        (
            format!(
                "{}True{}",
                "if !".repeat(2000),
                " { 1 } else { 0 }".repeat(2000)
            ),
            Some("nested control expression requires parentheses"),
        ),
    ] {
        fs::write(
            cwd.join("src/main.telora"),
            template("value.telora").replace("{{VALUE}}", &value),
        )
        .unwrap();
        for only_types in [true, false] {
            check(&cwd, only_types, error);
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn full_check_handles_data_parser_boundaries_on_a_small_stack() {
    let cwd = fixture();
    for (file, source, error) in [
        (
            "input.json",
            format!("{}0{}", "[".repeat(32), "]".repeat(32)),
            None,
        ),
        (
            "input.json",
            format!("{}0{}", "[".repeat(2000), "]".repeat(2000)),
            Some("data syntax nesting exceeds parser limit"),
        ),
        (
            "input.toml",
            format!("items = [{}]", vec!["0"; 10000].join(",")),
            None,
        ),
        (
            "input.yaml",
            format!("{}0{}", "[".repeat(2000), "]".repeat(2000)),
            Some("data syntax nesting exceeds parser limit"),
        ),
    ] {
        fs::write(cwd.join("src").join(file), source).unwrap();
        fs::write(
            cwd.join("src/main.telora"),
            template("data.telora").replace("{{FILE}}", file),
        )
        .unwrap();
        check(&cwd, false, error);
    }
    fs::remove_dir_all(cwd).unwrap();
}
