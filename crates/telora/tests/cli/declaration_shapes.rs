use super::*;

#[test]
fn declaration_shapes_are_not_inferred_from_the_first_use() {
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/issues/191");
    for root in ["main", "decorated", "local", "local-late", "reversed", "empty", "batch"] {
        for only_types in [true, false] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_telora"));
            command.current_dir(&cwd).args(["check", &format!("@src/{root}")]);
            if only_types { command.arg("--only-types"); }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{root}, only_types={only_types}: {} {}",
                String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            let records = String::from_utf8(output.stdout).unwrap().lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
            let summary = records.last().unwrap();
            assert_eq!(summary["status"], "ok");
            assert_eq!(summary["unknown_types"], 0);
            assert_eq!(summary["type_conflicts"], 0);
        }
    }
    for (root, message) in [("bad-field", "cannot unify"), ("bad-alias", "Missing")] {
        let output = Command::new(env!("CARGO_BIN_EXE_telora"))
            .current_dir(&cwd).args(["check", "--only-types", &format!("@src/{root}")])
            .output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains(message), "{stdout}");
    }
}
