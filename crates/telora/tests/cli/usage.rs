use super::*;

#[test]
fn execution_usage_is_an_opt_in_diagnostic_with_decimal_limits() {
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
    assert_eq!(usage["linear_memory"]["limit_bytes"], 64_000_000);
    let bytes = usage["linear_memory"]["bytes"].as_u64().unwrap();
    assert!(bytes > 0 && bytes <= 64_000_000 && bytes % 65536 == 0);
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
