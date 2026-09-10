#[test]
fn static_mir_query_returns_known_unknown_and_conflicted_without_evaluation() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/main.telora"),
        r#"
import "./data.json" { data };
export def answer = 1 / 0;
export def unknown = missing;
export def bad: Int = "wrong";
"#,
    )
    .unwrap();
    // An invalid data document must not be parsed in either static CLI consumer.
    fs::write(cwd.join("src/data.json"), "THIS IS NOT JSON").unwrap();
    let output = telora(&cwd)
        .args(["query", "exports", "@src/main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = jsonl(&output.stdout);
    let export = |name| {
        records
            .iter()
            .find(|r| r["record"] == "export" && r["name"] == name)
            .unwrap()
    };
    assert_eq!(export("answer")["type"], "Int");
    assert!(export("answer")["type_id"].is_number());
    assert_eq!(export("unknown")["state"], "Unknown");
    assert_eq!(export("bad")["state"], "Conflicted");
    assert!(records.iter().any(|r| {
        r["record"] == "diagnostic"
            && r["message"]
                .as_str()
                .is_some_and(|m| m.contains("unresolved symbol"))
    }));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("division by zero"));

    {
        let output = telora(&cwd)
            .args(["check", "--only-types", "@src/main"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let records = jsonl(&output.stdout);
        let summary = records.iter().find(|r| r["record"] == "summary").unwrap();
        assert_eq!(summary["types_only"], true);
        assert!(summary["type_conflicts"].as_u64().unwrap() > 0);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("division by zero"));
        assert!(!records.iter().any(|r| {
            r["labels"]
                .as_array()
                .is_some_and(|labels| labels.iter().any(|l| l["source"] == "fixture/data.json"))
        }));
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn static_mir_query_links_imports_and_source_positions() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/math.telora"),
        "export def inc = fn(x) { x + 1 };",
    )
    .unwrap();
    fs::write(
        cwd.join("src/main.telora"),
        "import \"@src/math\" { inc };\nexport def answer = inc(41);\n",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["query", "at", "@src/main:2:20"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let records = jsonl(&output.stdout);
    assert!(records.iter().any(|r| r["record"] == "reference"
        && r["name"] == "inc"
        && r["resolution"] == "Bound"
        && r["target_id"].is_number()));
    assert!(
        records.iter().any(|r| r["record"] == "expression"
            && r["type"] == "Int"
            && r["type_slot"].is_number())
    );

    fs::write(
        cwd.join("tests/probe.telora"),
        "import \"@src/math\" { inc }; export def answer = inc(1);",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["query", "exports", "@test/probe"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        jsonl(&output.stdout)
            .iter()
            .any(|r| r["record"] == "export" && r["name"] == "answer" && r["type"] == "Int")
    );
    fs::remove_dir_all(cwd).unwrap();
}
