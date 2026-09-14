use super::*;

#[test]
fn language_acceptance_fixtures_pass() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(repository.join("scripts/test-language.sh"))
        .env("TELORA_BIN", env!("CARGO_BIN_EXE_telora"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
