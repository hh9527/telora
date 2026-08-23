#[test]
fn help_is_clap_owned_and_types_is_removed() {
    let cwd = fixture();
    let help = telora(&cwd).arg("--help").output().unwrap();
    let output = String::from_utf8_lossy(&help.stdout);
    assert!(help.status.success());
    assert!(output.contains("lsp"));
    assert!(!output.contains("types"));
    let types = telora(&cwd)
        .args(["types", "@src/lib.telora"])
        .output()
        .unwrap();
    assert!(!types.status.success());

    assert!(output.contains("query"));
    assert!(output.contains("q"));
    assert!(!output.contains("show"));
    let query_help = telora(&cwd).args(["query", "-h"]).output().unwrap();
    let output = String::from_utf8_lossy(&query_help.stdout);
    assert!(query_help.status.success());
    assert!(output.contains("modules"));
    assert!(output.contains("exports"));
    assert!(output.contains("at"));
    assert!(output.contains("telora q modules -p std/"));
}

#[test]
fn run_and_check_select_logical_roots_from_cwd() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "export def output = \"42\";").unwrap();
    fs::write(
        cwd.join("src/bin/main.telora"),
        "import \"@src/lib.telora\" {output}; export {output};",
    )
    .unwrap();
    let nested = cwd.join("src/bin");
    let run = telora(&nested).args(["run", "main"]).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");
    let check = telora(&nested)
        .args(["check", "@src/lib.telora"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn query_selects_registered_standard_library_modules() {
    let cwd = fixture();
    let string = telora(&cwd)
        .args(["query", "exports", "std/string"])
        .output()
        .unwrap();
    assert!(
        string.status.success(),
        "{}",
        String::from_utf8_lossy(&string.stderr)
    );
    let records = jsonl(&string.stdout);
    assert!(
        records
            .iter()
            .all(|record| record["module"] == "std/string")
    );
    assert!(
        records
            .iter()
            .any(|record| { record["record"] == "export" && record["name"] == "length" })
    );

    let array = telora(&cwd)
        .args(["query", "at", "std/array", "-p", "flat_map"])
        .output()
        .unwrap();
    assert!(
        array.status.success(),
        "{}",
        String::from_utf8_lossy(&array.stderr)
    );
    let records = jsonl(&array.stdout);
    assert!(
        records
            .iter()
            .any(|record| { record["record"] == "definition" && record["name"] == "flat_map" })
    );

    let missing = telora(&cwd)
        .args(["query", "exports", "std/not-present"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(missing.stderr.is_empty());
    let records = jsonl(&missing.stdout);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["record"], "diagnostic");
    assert_eq!(records[0]["module"], "std/not-present");
    assert_eq!(records[0]["severity"], "error");
    assert_eq!(
        records[0]["message"],
        "unknown built-in module \"std/not-present\""
    );
}

#[test]
fn query_modules_lists_the_crate_view_as_stable_jsonl() {
    let cwd = fixture();
    let dependency = cwd.join("dependency");
    fs::create_dir_all(dependency.join("src/bin")).unwrap();
    fs::write(cwd.join("src/lib.telora"), "0").unwrap();
    fs::write(cwd.join("src/local.priv.telora"), "0").unwrap();
    fs::write(cwd.join("src/local.native.telora"), "0").unwrap();
    fs::write(cwd.join("src/bin/main.telora"), "0").unwrap();
    fs::write(cwd.join("tests/query.telora"), "0").unwrap();
    fs::write(dependency.join("src/public.telora"), "0").unwrap();
    fs::write(dependency.join("src/hidden.priv.telora"), "0").unwrap();
    fs::write(dependency.join("src/bin/tool.telora"), "0").unwrap();
    fs::write(
        cwd.join("telora-deps.json"),
        r#"{"dependencies":{"dep":{"path":"dependency"}}}"#,
    )
    .unwrap();

    let output = telora(&cwd)
        .args(["q", "modules", "-p", "telora"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = jsonl(&output.stdout);
    let names = records
        .iter()
        .map(|record| record["module"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(names.contains(&"@src/local.priv.telora"));
    assert!(names.contains(&"@src/local.native.telora"));
    assert!(names.contains(&"dep/public.telora"));
    assert!(!names.contains(&"dep/hidden.priv.telora"));
    assert!(!names.contains(&"dep/bin/tool.telora"));
    assert!(records.iter().all(|record| {
        record["schema"] == "telora.query/v1"
            && record["record"] == "module"
            && record["format"] == "telora"
    }));
    let private = records
        .iter()
        .find(|record| record["module"] == "@src/local.priv.telora")
        .unwrap();
    assert_eq!(private["origin"], "crate");
    assert_eq!(private["visibility"], "private");

    assert!(
        !telora(&cwd)
            .args(["query", "modules", "@src/lib.telora"])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn check_rejects_concrete_runtime_errors_without_synthetic_finalization() {
    let cwd = fixture();
    let cases = [
        ("failed", "export def output = fail!(\"boom\", 1);"),
        ("division", "export def output = 1 / 0;"),
        ("index", "export def output = [1][2];"),
    ];
    for (name, source) in cases {
        fs::write(cwd.join(format!("src/{name}.telora")), source).unwrap();
        let module_id = format!("@src/{name}.telora");
        let check = telora(&cwd)
            .args(["check", module_id.as_str()])
            .output()
            .unwrap();
        assert!(
            !check.status.success(),
            "{name} unexpectedly passed: {}",
            String::from_utf8_lossy(&check.stdout)
        );
        let records = jsonl(&check.stdout);
        assert!(
            records
                .iter()
                .any(|record| record["record"] == "diagnostic"),
            "{name} emitted no diagnostic"
        );
        assert_eq!(records.last().unwrap()["record"], "summary");
        assert_eq!(records.last().unwrap()["status"], "error");
        assert!(!records.iter().any(|record| {
            record["message"]
                .as_str()
                .is_some_and(|message| message.contains("finalization is incomplete"))
        }));
        assert!(check.stderr.is_empty(), "{name} mixed text into stderr");
    }
}

#[test]
fn check_suppresses_parser_recovery_fallout_but_keeps_independent_errors() {
    let cwd = fixture();
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "one-root",
            "export def broken = match 'A { 'A 1, _ => 2 };",
            &["missing FatArrow"],
        ),
        (
            "two-roots",
            "export def first = (1 + 2; export def second = match 'A { 'A 1, _ => 2 };",
            &[
                "invalid syntax, expected one of: ',', ')'",
                "missing FatArrow",
            ],
        ),
    ];

    for (name, source, expected) in cases {
        let path = cwd.join(format!("src/{name}.telora"));
        fs::write(path, source).unwrap();
        let module_id = format!("@src/{name}.telora");
        let check = telora(&cwd)
            .args(["check", module_id.as_str()])
            .output()
            .unwrap();
        assert!(!check.status.success(), "{name} unexpectedly passed");
        assert!(check.stderr.is_empty(), "{name} mixed text into stderr");

        let records = jsonl(&check.stdout);
        let messages = records
            .iter()
            .filter(|record| record["record"] == "diagnostic")
            .map(|record| record["message"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(messages, *expected, "{name}");
        assert_eq!(records.last().unwrap()["record"], "summary");
        assert_eq!(records.last().unwrap()["status"], "error");
    }
}

#[test]
fn check_accepts_a_complete_module_with_warnings() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/warning.telora"),
        "def reject: Fn() -> Result(Int, String) = fn() { 'Err(\"notice\") }; def checked = reject.should_ok!(); export def output = 1;",
    )
    .unwrap();
    let check = telora(&cwd)
        .args(["check", "@src/warning.telora"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let records = jsonl(&check.stdout);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema"], "telora.check/v1");
    assert_eq!(records[0]["module"], "@src/warning.telora");
    assert_eq!(records[0]["record"], "diagnostic");
    assert_eq!(records[0]["severity"], "warning");
    assert_eq!(records[1]["record"], "summary");
    assert_eq!(records[1]["status"], "ok");
    assert_eq!(records[1]["dependencies"], 0);
}

#[test]
fn run_writes_contextual_debug_as_stderr_jsonl() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        "def var = 3; def observed = var.dbg!(\"observed\"); export def output = \"3\";",
    )
    .unwrap();
    let run = telora(&cwd).args(["run", "main"]).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "3");
    let records = jsonl(&run.stderr);
    assert_eq!(records.len(), 1, "finalization must not repeat dbg! events");
    for record in records {
        assert_eq!(record["name"], "var");
        assert_eq!(record["repr"], "3");
        assert_eq!(record["module"], "@bin/main.telora");
        assert_eq!(record["line"], 1);
        assert_eq!(record["message"], "observed");
    }
}

#[test]
fn best_effort_run_collects_main_diagnostics_before_starting_entry() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/array" as array;
def transform: Fn(Int) -> Int = fn(item) {
    if item == 2 { fail!("two", item) }
    else if item == 4 { fail!("four", item) }
    else { item }
};
def broken = array.map([1, 2, 3, 4], transform);
export def output = if array.length(broken) > 0 { "unexpected" } else { "empty" };"#,
    )
    .unwrap();

    let run = telora(&cwd)
        .args(["run", "main", "--best-effort"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(run.stdout.is_empty(), "Entry started for an invalid Main");
    let records = jsonl(&run.stderr);
    assert!(records.iter().any(|record| record["message"] == "two"));
    assert!(records.iter().any(|record| record["message"] == "four"));
    assert_eq!(records.last().unwrap()["schema"], "telora.run/v1");
    assert_eq!(records.last().unwrap()["record"], "summary");
    assert_eq!(records.last().unwrap()["status"], "error");
}

#[test]
fn check_and_best_effort_run_do_not_repeat_cross_module_polymorphic_failures() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/dependency.telora"),
        r#"type Plan(Revision) = struct { revision: Revision };
def ensure_plan: for(Revision) Fn(Plan(Revision), Plan(Revision)) -> Plan(Revision) = fn(left, right) {
    fail!("plan rejected", left)
};
export { Plan, ensure_plan };"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "@src/dependency.telora" as dependency;
def plan: dependency.Plan(Int) = { revision: 1 };
export def output = dependency.ensure_plan(plan, plan);"#,
    )
    .unwrap();

    let check = telora(&cwd)
        .args(["check", "@bin/main.telora"])
        .output()
        .unwrap();
    assert!(!check.status.success());
    let check_records = jsonl(&check.stdout);
    assert_eq!(
        check_records
            .iter()
            .filter(|record| record["message"] == "plan rejected")
            .count(),
        1
    );
    assert!(!check_records.iter().any(call_cascade));

    let run = telora(&cwd)
        .args(["run", "main", "--best-effort"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(run.stdout.is_empty());
    let run_records = jsonl(&run.stderr);
    assert_eq!(
        run_records
            .iter()
            .filter(|record| record["message"] == "plan rejected")
            .count(),
        1
    );
    assert!(!run_records.iter().any(call_cascade));
}

#[test]
fn best_effort_run_preserves_failure_through_imported_facade_closures() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/rules.telora"),
        r#"def reject: Fn(Int) -> Int = fn(value) {
    fail!("source rule rejected value", value)
};
export {reject};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/facade.telora"),
        r#"import "@src/rules.telora" as rules;
def lower: Fn(Int) -> Int = fn(value) { rules.reject(value) };
export {lower};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "@src/facade.telora" as facade;
def rejected = facade.lower(7);
export def output: String = "value={rejected}";"#,
    )
    .unwrap();

    let strict = telora(&cwd).args(["run", "main"]).output().unwrap();
    assert!(!strict.status.success());
    assert!(String::from_utf8_lossy(&strict.stderr).contains("source rule rejected value"));

    let recovered = telora(&cwd)
        .args(["run", "main", "--best-effort"])
        .output()
        .unwrap();
    assert!(!recovered.status.success());
    let records = jsonl(&recovered.stderr);
    assert_eq!(
        records
            .iter()
            .filter(|record| record["message"] == "source rule rejected value")
            .count(),
        1
    );
    assert!(
        records
            .iter()
            .any(|record| record.to_string().contains("src/rules.telora")),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!records.iter().any(|record| {
        record["message"]
            .as_str()
            .is_some_and(|message| message.contains("dependent computation received"))
    }));
}

#[test]
fn best_effort_run_preserves_failure_across_two_imported_computations() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/factory.telora"),
        r#"def make_rejector: Fn(Int) -> Fn(Int) -> Int = fn(base) {
    fn(value) { fail!("factory rejected value", base, value) }
};
export {make_rejector};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/transform.telora"),
        r#"def render: Fn(Int) -> String = fn(value) { "rendered={value}" };
export {render};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/facade.telora"),
        r#"import "@src/factory.telora" as factory;
import "@src/transform.telora" as transform;
def lower: Fn(Int) -> String = fn(value) {
    let rejected: Int = factory.make_rejector(3)(value);
    transform.render(rejected)
};
export {lower};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "@src/facade.telora" as facade;
export def output: String = facade.lower(7);"#,
    )
    .unwrap();

    let strict = telora(&cwd).args(["run", "main"]).output().unwrap();
    assert!(!strict.status.success());
    assert!(String::from_utf8_lossy(&strict.stderr).contains("factory rejected value"));

    let recovered = telora(&cwd)
        .args(["run", "main", "--best-effort"])
        .output()
        .unwrap();
    assert!(!recovered.status.success());
    let records = jsonl(&recovered.stderr);
    assert_eq!(
        records
            .iter()
            .filter(|record| record["message"] == "factory rejected value")
            .count(),
        1
    );
    assert!(
        records
            .iter()
            .any(|record| record.to_string().contains("src/factory.telora")),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!records.iter().any(|record| {
        record["message"]
            .as_str()
            .is_some_and(|message| message.contains("dependent computation received"))
    }));
}

fn call_cascade(record: &Value) -> bool {
    record["message"].as_str().is_some_and(|message| {
        message.contains("tag constructor")
            || message.contains("expected Func")
            || message.contains("expected Dict")
    })
}

#[test]
fn recursive_type_metadata_does_not_add_recovery_errors() {
    let cwd = fixture();
    let source = r#"type CallExpr = struct { args: Array(Expr) };
type Expr = enum { 'Call(CallExpr), 'Text(String) };
def reject: Fn(Int) -> Expr = fn(value) { fail!("expected failure", value) };
def failed = reject(1);
export def output = "unreachable";"#;
    fs::write(cwd.join("src/bin/main.telora"), source).unwrap();

    let check = telora(&cwd)
        .args(["check", "@bin/main.telora"])
        .output()
        .unwrap();
    assert!(!check.status.success());
    let check_records = jsonl(&check.stdout);
    assert!(
        check_records
            .iter()
            .any(|record| record["message"] == "expected failure")
    );
    assert!(!check_records.iter().any(|record| {
        record["message"]
            .as_str()
            .is_some_and(|message| message.contains("cannot be partially evaluated"))
    }));

    let run = telora(&cwd)
        .args(["run", "main", "--best-effort"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(run.stdout.is_empty());
    let run_records = jsonl(&run.stderr);
    assert!(
        run_records
            .iter()
            .any(|record| record["message"] == "expected failure")
    );
    assert!(!run_records.iter().any(|record| {
        record["message"]
            .as_str()
            .is_some_and(|message| message.contains("cannot be partially evaluated"))
    }));
}

#[test]
fn check_keeps_recursive_type_metadata_inside_the_semantic_boundary() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/recursive.telora"),
        r#"type CallExpr = struct { args: Array(Expr) };
type Expr = enum { 'Call(CallExpr), 'Text(String) };
def identity: Fn(Expr) -> Expr = fn(value) { value };
export { CallExpr, Expr, identity };"#,
    )
    .unwrap();

    let check = telora(&cwd)
        .args(["check", "@src/recursive.telora"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
    let records = jsonl(&check.stdout);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["record"], "summary");
    assert_eq!(records[0]["status"], "ok");
    assert!(check.stderr.is_empty());

    let show = telora(&cwd)
        .args(["query", "exports", "@src/recursive.telora"])
        .output()
        .unwrap();
    assert!(show.status.success());
    let exports = jsonl(&show.stdout);
    assert_eq!(exports.len(), 3);
    assert!(exports.iter().all(|record| {
        record["authority"] == "authoritative" && !record["type"].as_str().unwrap().contains("Any")
    }));
    assert_eq!(
        exports
            .iter()
            .find(|record| record["name"] == "identity")
            .unwrap()["type"],
        "Fn(Expr) -> Expr"
    );
}

#[test]
fn query_rejects_a_missing_dependency_module_without_leaking_its_path() {
    let cwd = fixture();
    let dependency = cwd.join("query-builder");
    fs::create_dir_all(dependency.join("src")).unwrap();
    fs::write(
        cwd.join("telora-deps.json"),
        r#"{"dependencies":{"query-builder":{"path":"query-builder"}}}"#,
    )
    .unwrap();
    fs::write(dependency.join("telora-deps.json"), "{}").unwrap();
    fs::write(
        dependency.join("src/query-builder.telora"),
        "type Plan = struct {sql: String}; export {Plan};",
    )
    .unwrap();

    let missing_id = "query-builder/src/query-builder.telora";
    let missing = telora(&cwd)
        .args(["query", "exports", missing_id])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(missing.stderr.is_empty());
    let records = jsonl(&missing.stdout);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["record"], "diagnostic");
    assert_eq!(records[0]["module"], missing_id);
    assert_eq!(
        records[0]["message"],
        format!("module {missing_id:?} not found")
    );
    assert!(
        !records[0]["message"]
            .as_str()
            .unwrap()
            .contains(cwd.to_str().unwrap())
    );

    let found = telora(&cwd)
        .args([
            "query",
            "at",
            "query-builder/query-builder.telora",
            "-p",
            "Plan",
        ])
        .output()
        .unwrap();
    assert!(found.status.success());
    assert_eq!(jsonl(&found.stdout).len(), 1);

    let no_match = telora(&cwd)
        .args([
            "query",
            "at",
            "query-builder/query-builder.telora",
            "-p",
            "Absent",
        ])
        .output()
        .unwrap();
    assert!(no_match.status.success());
    assert!(no_match.stdout.is_empty());
    assert!(no_match.stderr.is_empty());
}

#[test]
fn recursive_modules_are_consistent_across_check_query_and_run_modes() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/tree.telora"),
        r#"import "std/array" as array;
type Node = struct {value: Int, children: Array(Node)};
def total: Fn(Node) -> Int = fn(node) {
    node.value + match array.get(node.children, 0) {
        'None => 0,
        'Some(child) => total(child),
    }
};
def root: Node = {value: 1, children: [{value: 2, children: []}]};
export {Node, root, total};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "@src/tree.telora" as tree;
export def output = `\{tree.total(tree.root)}`;"#,
    )
    .unwrap();

    let check = telora(&cwd)
        .args(["check", "@src/tree.telora"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
    let records = jsonl(&check.stdout);
    assert_eq!(records.last().unwrap()["status"], "ok");

    let show = telora(&cwd)
        .args(["query", "exports", "@src/tree.telora"])
        .output()
        .unwrap();
    assert!(show.status.success());
    let exports = jsonl(&show.stdout);
    assert_eq!(exports.len(), 3);
    assert!(exports.iter().all(|record| {
        record["authority"] == "authoritative" && !record["type"].as_str().unwrap().contains("Any")
    }));

    for arguments in [vec!["run", "main"], vec!["run", "main", "--best-effort"]] {
        let run = telora(&cwd).args(arguments).output().unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "3");
    }
}

#[test]
fn public_cli_rejects_physical_paths_and_missing_manifests() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "export def output = 1;").unwrap();
    let physical = telora(&cwd)
        .args(["run", "src/lib.telora"])
        .output()
        .unwrap();
    assert!(!physical.status.success());
    let outside = fixture();
    fs::remove_file(outside.join("telora-deps.json")).unwrap();
    let missing = telora(&outside)
        .args(["check", "@src/lib.telora"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("cannot find telora-deps.json"));
}

#[test]
fn named_queries_emit_stable_jsonl() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "type Name = String; def hidden = 1; def make: Fn(Int) -> Int = fn(value) { value }; export {Name, make};").unwrap();
    let show = telora(&cwd)
        .args([
            "query",
            "at",
            "@src/lib.telora",
            "-p",
            "a",
            "-k",
            "type,def",
        ])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let records = jsonl(&show.stdout);
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(
        |record| record["schema"] == "telora.query/v1" && record["module"] == "@src/lib.telora"
    ));
    assert_eq!(records[0]["name"], "Name");
    assert_eq!(records[1]["name"], "make");

    let exports = telora(&cwd)
        .args(["query", "exports", "@src/lib.telora"])
        .output()
        .unwrap();
    let records = jsonl(&exports.stdout);
    assert_eq!(
        records
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Name", "make"]
    );
}

#[test]
fn query_namespace_imports_reference_exact_module_interfaces() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/types.telora"),
        "type CallExpr = struct {args: Array(Expr)};\ntype Expr = enum {'Text(String), 'Call(CallExpr)};\ntype Box(A) = struct {value: A};\nexport {CallExpr, Expr, Box};\n",
    )
    .unwrap();
    fs::write(
        cwd.join("src/lib.telora"),
        "import \"@src/types.telora\" as types;\nimport \"@src/types.telora\" { Expr };\nexport {types, Expr};\n",
    )
    .unwrap();

    let show = telora(&cwd)
        .args(["query", "at", "@src/lib.telora", "-k", "import"])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let records = jsonl(&show.stdout);
    let namespace = records
        .iter()
        .find(|record| record["name"] == "types")
        .unwrap();
    assert_eq!(namespace["authority"], "authoritative");
    assert_eq!(namespace["target"], "@src/types.telora");
    assert!(namespace.get("type").is_none());

    let selective = records
        .iter()
        .find(|record| record["name"] == "Expr")
        .unwrap();
    let ty = selective["type"].as_str().unwrap();
    assert!(ty.contains("TypeOf(Expr)"), "{ty}");
    assert!(!ty.contains("Any"), "{ty}");
}

#[test]
fn query_exports_preserves_type_family_binders_across_reexports() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/model.telora"),
        r#"export type Entity(EntityId) = struct {id: EntityId, label: String};
export type Request(Id, Subject, Input) = struct {id: Id, subject: Subject, input: Input};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/selective.telora"),
        r#"import "@src/model.telora" {Entity, Request};
export {Entity as PublicEntity, Request};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/open.telora"),
        r#"import "@src/model.telora" *;
export {Entity, Request};"#,
    )
    .unwrap();

    let cases = [
        ("@src/model.telora", "Entity"),
        ("@src/selective.telora", "PublicEntity"),
        ("@src/open.telora", "Entity"),
    ];
    for (module, entity_name) in cases {
        let show = telora(&cwd)
            .args(["query", "exports", module])
            .output()
            .unwrap();
        assert!(
            show.status.success(),
            "{}",
            String::from_utf8_lossy(&show.stderr)
        );
        let records = jsonl(&show.stdout);
        let entity = records
            .iter()
            .find(|record| record["name"] == entity_name)
            .unwrap();
        assert_eq!(
            entity["type"],
            "for(EntityId) Fn(TypeOf(EntityId)) -> TypeOf(Entity)"
        );
        let request = records
            .iter()
            .find(|record| record["name"] == "Request")
            .unwrap();
        assert_eq!(
            request["type"],
            "for(Id, Subject, Input) Fn(TypeOf(Id), TypeOf(Subject), TypeOf(Input)) -> TypeOf(Request)"
        );
        assert_eq!(entity["authority"], "authoritative");
        assert_eq!(request["authority"], "authoritative");
    }
}

#[test]
fn query_position_and_conflicts_are_structured() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/lib.telora"),
        "def answer = 42;\nexport {answer};\n",
    )
    .unwrap();
    let at = telora(&cwd)
        .args(["query", "at", "@src/lib.telora:1:4"])
        .output()
        .unwrap();
    assert!(
        at.status.success(),
        "{}",
        String::from_utf8_lossy(&at.stderr)
    );
    assert!(
        jsonl(&at.stdout)
            .iter()
            .any(|record| record["record"] == "definition" && record["name"] == "answer")
    );
    let conflict = telora(&cwd)
        .args(["query", "at", "@src/lib.telora:1", "-p", "a"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    let bad_kind = telora(&cwd)
        .args(["query", "at", "@src/lib.telora", "-k", "let,"])
        .output()
        .unwrap();
    assert!(!bad_kind.status.success());
}

#[test]
fn query_and_cli_jsonl_use_one_based_lines_and_zero_based_utf8_columns() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/lib.telora"),
        "def other = 42;\ndef value = (\"中\", other);\nexport {value};\n",
    )
    .unwrap();

    let named = telora(&cwd)
        .args(["query", "at", "@src/lib.telora", "-p", "value"])
        .output()
        .unwrap();
    assert!(named.status.success());
    let records = jsonl(&named.stdout);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["location"]["line"], 2);
    assert_eq!(records[0]["location"]["column"], 4);
    assert_eq!(records[0]["location"]["end_line"], 2);
    assert_eq!(records[0]["location"]["end_column"], 9);

    for selector in ["@src/lib.telora:2:0", "@src/lib.telora:2:20"] {
        let output = telora(&cwd)
            .args(["query", "at", selector])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if selector.ends_with(":20") {
            let records = jsonl(&output.stdout);
            let reference = records
                .iter()
                .find(|record| record["record"] == "reference" && record["name"] == "other")
                .unwrap();
            assert_eq!(reference["location"]["line"], 2);
            assert_eq!(reference["location"]["column"], 20);
            assert_eq!(reference["location"]["end_column"], 25);
        }
    }
    let inside_scalar = telora(&cwd)
        .args(["query", "at", "@src/lib.telora:2:15"])
        .output()
        .unwrap();
    assert!(!inside_scalar.status.success());
    assert!(String::from_utf8_lossy(&inside_scalar.stderr).contains("outside"));

    let at_end = telora(&cwd)
        .args(["query", "at", "@src/lib.telora:2:25"])
        .output()
        .unwrap();
    assert!(at_end.status.success());
    assert!(
        jsonl(&at_end.stdout)
            .iter()
            .all(|record| record["name"] != "other")
    );
    assert!(
        !telora(&cwd)
            .args(["query", "at", "@src/lib.telora:0"])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn test_roots_are_selectable_but_not_importable() {
    let cwd = fixture();
    fs::write(cwd.join("tests/codec.telora"), "export def output = 7;").unwrap();
    let run = telora(&cwd)
        .args(["check", "@test/codec.telora"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    fs::write(
        cwd.join("src/lib.telora"),
        "import \"@test/codec.telora\" as codec; export def output = codec;",
    )
    .unwrap();
    let check = telora(&cwd)
        .args(["check", "@src/lib.telora"])
        .output()
        .unwrap();
    assert!(!check.status.success());
}

#[test]
fn run_injects_external_json_as_value() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    fs::write(
        cwd.join("src/input.entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
import "std/value" as value;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = String;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {
        (
            {
                data_srcs: {input: {default: 'None, fmt: 'Json, src: "input.json"}},
                spawn_child: 'False,
                text_srcs: {},
                vars: [],
                stdin: 'Null,
            },
            fn(resources, main) {
                let item = resources.data.input;
                let output = match item.data {
                    'String(value) => value,
                    _ => fail!("expected JSON string", item.data),
                };
                (output, fn(state, event) {
                    match event {
                        'Initialize => (state, ['Output(state), 'Exit(0)]),
                        _ => fail!("unexpected event", event),
                    }
                })
            },
        )
    };"#,
    )
    .unwrap();
    fs::write(cwd.join("input.json"), r#""accepted""#).unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/input.entry.telora", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "accepted");
}

#[test]
fn entry_data_diagnostics_use_the_registered_data_source() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    fs::write(cwd.join("input.json"), r#"{"name":"Ada"}"#).unwrap();
    fs::write(
        cwd.join("src/provenance.entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = Int;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config: Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {
        (
            {
                data_srcs: {input: {default: 'None, fmt: 'Json, src: "input.json"}},
                spawn_child: 'False,
                text_srcs: {},
                vars: [],
                stdin: 'Null,
            },
            fn(resources, main) {
                match resources.data.input.data {
                    'Object(object) => fail!("review data provenance", object.name),
                    _ => fail!("expected object"),
                }
            },
        )
    };"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/provenance.entry.telora", "main"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("provenance.entry.telora:19:40: review data provenance"),
        "{stderr}"
    );
    assert!(
        stderr.contains("input.json:1:9: subject 1 originated here"),
        "{stderr}"
    );
}

#[test]
fn run_decodes_all_data_source_formats() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    fs::write(cwd.join("input.json"), r#"{"name":"json"}"#).unwrap();
    fs::write(cwd.join("input.yaml"), "name: yaml\n").unwrap();
    fs::write(cwd.join("input.toml"), "name = \"toml\"\n").unwrap();
    fs::write(
        cwd.join("src/formats.entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = String;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
def object_name = fn(value) {
    match value {
        'Object(object) => match object.name {
            'String(name) => name,
            _ => fail!("name is not a string", object.name),
        },
        _ => fail!("data source is not an object", value),
    }
};
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {
        (
            {
                data_srcs: {
                    json: {default: 'None, fmt: 'Json, src: "input.json"},
                    yaml: {default: 'None, fmt: 'Yaml, src: "input.yaml"},
                    toml: {default: 'None, fmt: 'Toml, src: "input.toml"},
                },
                spawn_child: 'False,
                text_srcs: {},
                vars: [],
                stdin: 'Null,
            },
            fn(resources, main) {
                let output = `\{object_name(resources.data.json.data)}|\{object_name(resources.data.yaml.data)}|\{object_name(resources.data.toml.data)}`;
                (output, fn(state, event) {
                    match event {
                        'Initialize => (state, ['Output(state), 'Exit(0)]),
                        _ => fail!("unexpected event", event),
                    }
                })
            },
        )
    };"#,
    )
    .unwrap();
    let run = telora(&cwd)
        .args(["run-with", "@src/formats.entry.telora", "main"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout), "json|yaml|toml");
}

#[test]
fn run_injects_text_environment_and_text_stdin() {
    let cwd = fixture();
    fs::write(cwd.join("src/bin/main.telora"), "export def marker = 0;").unwrap();
    fs::write(cwd.join("message.txt"), "from-file").unwrap();
    fs::write(
        cwd.join("src/resources.entry.telora"),
        r#"import "std/rt.priv.telora" as rt;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = String;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {
        (
            {
                data_srcs: {},
                spawn_child: 'False,
                text_srcs: {message: {default: 'None, src: "message.txt"}},
                vars: ["TELORA_TEST_VAR", "TELORA_TEST_MISSING_VAR"],
                stdin: 'Text,
            },
            fn(resources, main) {
                let input = match resources.stdin {
                    'Some(value) => value,
                    'None => fail!("missing text stdin"),
                };
                let output = `\{resources.texts.message.data}|\{resources.texts.message.src}|\{resources.vars.TELORA_TEST_VAR}|\{input}`;
                (output, fn(state, event) {
                    match event {
                        'Initialize => (state, ['Output(state), 'Exit(0)]),
                        _ => fail!("unexpected event", event),
                    }
                })
            },
        )
    };"#,
    )
    .unwrap();
    let mut child = telora(&cwd)
        .args(["run-with", "@src/resources.entry.telora", "main"])
        .env("TELORA_TEST_VAR", "from-env")
        .env_remove("TELORA_TEST_MISSING_VAR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"from-stdin")
        .unwrap();
    let run = child.wait_with_output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "from-file|message.txt|from-env|from-stdin"
    );

    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let invalid = telora(&cwd)
            .args(["run-with", "@src/resources.entry.telora", "main"])
            .env("TELORA_TEST_VAR", "from-env")
            .env("TELORA_TEST_MISSING_VAR", OsString::from_vec(vec![0xff]))
            .output()
            .unwrap();
        assert!(!invalid.status.success());
        assert!(invalid.stdout.is_empty());
        assert!(String::from_utf8_lossy(&invalid.stderr).contains("cannot read variable"));
    }
}

