use super::*;

#[test]
fn language_acceptance_fixtures_pass() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = repository.join("scripts/test-language.sh");
    // Execute CRLF checkouts too, without relying on the OS to launch .sh files.
    // Language and data fixtures themselves are passed through intact.
    let script_text = fs::read_to_string(&script).unwrap().replace("\r\n", "\n");
    let output = Command::new("bash")
        .arg("-c")
        .arg(script_text)
        .arg(script.to_string_lossy().replace('\\', "/"))
        .env("TELORA_BIN", env!("CARGO_BIN_EXE_telora").replace('\\', "/"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
