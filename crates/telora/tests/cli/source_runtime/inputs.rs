use super::*;

#[test]
fn source_data_depth_limit_applies_before_materialization() {
    let cwd = fixture();
    let input = format!("{}0{}", "[".repeat(256), "]".repeat(256));
    fs::write(cwd.join("src/deep.json"), &input).unwrap();
    fs::write(
        cwd.join("src/main.telora"),
        "import \"./deep.json\" as data; import \"std/value\" {Value}; export def answer: Value = data.data;",
    )
    .unwrap();
    let output = telora(&cwd).args(["check", "@src/main"]).output().unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("depth"), "{stdout}");
    fs::remove_dir_all(cwd).unwrap();
}
