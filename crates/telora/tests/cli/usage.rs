use super::*;

fn config(cwd: &Path, compiler: Value, runtime: Value) {
    fs::write(cwd.join("telora-config.json"), serde_json::to_vec(&serde_json::json!({
        "version": 1, "members": ["."], "compiler": compiler, "runtime": runtime,
    })).unwrap()).unwrap();
}

#[test]
fn workspace_runtime_defaults_and_explicit_cli_overrides() {
    let cwd = fixture();
    fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../telora-wasm/tests/fixtures/format.telora"),
        cwd.join("src/main.telora")).unwrap();
    for (runtime, flags, fuel, memory) in [
        (serde_json::json!({}), vec![], 100, 1024),
        (serde_json::json!({"fuel":23}), vec![], 23, 1024),
        (serde_json::json!({"memoryLimit":64}), vec![], 100, 64),
        (serde_json::json!({"fuel":23,"memoryLimit":64}), vec![], 23, 64),
        (serde_json::json!({"fuel":23,"memoryLimit":64}), vec!["--with-fuel","100"], 100, 64),
        (serde_json::json!({"fuel":23,"memoryLimit":64}), vec!["--with-memory-limit","1024"], 23, 1024),
        (serde_json::json!({"fuel":23,"memoryLimit":64}), vec!["--with-fuel","100","--with-memory-limit","1024"], 100, 1024),
    ] {
        config(&cwd, serde_json::json!({}), runtime);
        let output = telora(&cwd).args(["eval", "@src/main:answer", "--report-usage"]).args(flags).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(diagnostic["usage"]["fuel"]["limit"], fuel * 1_000_000u64);
        assert_eq!(diagnostic["usage"]["linear_memory"]["limit_bytes"], memory * (1u64 << 20));
    }
    let check = telora(&cwd).args(["check", "--lib", "--report-usage"]).output().unwrap();
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
    let diagnostic: Value = serde_json::from_slice(&check.stderr).unwrap();
    assert_eq!(diagnostic["usage"]["fuel"]["limit"], 23_000_000u64);
    assert_eq!(diagnostic["usage"]["linear_memory"]["limit_bytes"], 64u64 << 20);
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn workspace_compiler_options_apply_without_cli_switches() {
    let cwd = fixture();
    fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compiler-options.telora"),
        cwd.join("src/main.telora")).unwrap();
    for (compiler, expected) in [
        (serde_json::json!({"maxTupleItems":16}), Some("compiler.maxTupleItems = 16")),
        (serde_json::json!({"maxTypeArguments":16}), Some("compiler.maxTypeArguments = 16")),
        (serde_json::json!({"maxTypeDepth":1}), Some("compiler.maxTypeDepth = 1")),
        (serde_json::json!({"maxTupleItems":32,"maxTypeArguments":32}), None),
    ] {
        config(&cwd, compiler, serde_json::json!({}));
        let output = telora(&cwd).args(["check", "--lib", "--only-types"]).output().unwrap();
        if let Some(expected) = expected {
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stdout).contains(expected), "{}", String::from_utf8_lossy(&output.stdout));
        } else {
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        }
    }
    let output = telora(&cwd).args(["check", "--lib", "--max-type-depth", "500"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    config(&cwd, serde_json::json!({"maxTypeDepth":1}), serde_json::json!({}));
    let output = telora(&cwd).args(["check", "std/prelude", "--only-types"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("compiler.maxTypeDepth = 1"));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn execution_usage_reports_fuel_and_page_aligned_mib_limits() {
    let cwd = fixture();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../telora-wasm/tests/fixtures/format.telora"),
        cwd.join("src/main.telora"),
    ).unwrap();
    let plain = telora(&cwd).args(["eval", "@src/main:answer"]).output().unwrap();
    assert!(plain.status.success());
    assert!(plain.stderr.is_empty());
    let reported = telora(&cwd).args([
        "--with-fuel", "200", "eval", "@src/main:answer",
        "--with-memory-limit", "64", "--report-usage",
    ]).output().unwrap();
    assert!(reported.status.success(), "{}", String::from_utf8_lossy(&reported.stderr));
    assert_eq!(plain.stdout, reported.stdout);
    let diagnostic: Value = serde_json::from_slice(&reported.stderr).unwrap();
    assert_eq!(diagnostic["record"], "diagnostic");
    assert_eq!(diagnostic["severity"], "info");
    assert_eq!(diagnostic["code"], "execution-usage");
    let usage = &diagnostic["usage"];
    assert_eq!(usage["fuel"]["limit"], 200_000_000u64);
    let consumed = usage["fuel"]["consumed"].as_u64().unwrap();
    assert!(consumed > 0);
    assert_eq!(consumed + usage["fuel"]["remaining"].as_u64().unwrap(), 200_000_000);
    assert_eq!(usage["linear_memory"]["limit_bytes"], 64u64 << 20);
    let bytes = usage["linear_memory"]["bytes"].as_u64().unwrap();
    assert!(bytes > 0 && bytes <= (64 << 20) && bytes % 65536 == 0);
    let static_only = telora(&cwd).args([
        "check", "@src/main", "--only-types", "--report-usage",
    ]).output().unwrap();
    assert!(static_only.status.success());
    assert!(static_only.stderr.is_empty());
    for arguments in [
        ["--with-fuel", "0"],
        ["--with-fuel", "18446744073709551615"],
        ["--with-memory-limit", "0"],
        ["--with-memory-limit", "18446744073709551615"],
        ["--eval-fuel", "100"],
    ] {
        let output = telora(&cwd).args(arguments).args(["check", "--lib"]).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
    fs::remove_dir_all(cwd).unwrap();
}
