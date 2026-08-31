#[test]
fn help_is_clap_owned_and_types_is_removed() {
    let cwd = fixture();
    let help = telora(&cwd).arg("--help").output().unwrap();
    let output = String::from_utf8_lossy(&help.stdout);
    assert!(help.status.success());
    assert!(output.contains("lsp"));
    assert!(!output.contains("ees"));
    assert!(!output.contains("types"));
    assert!(!output.contains("run-with"));
    let types = telora(&cwd)
        .args(["types", "@src/lib"])
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
        cwd.join("src/app.telora"),
        r#"import "@src/lib" {output};
import "std/actor" as actor;
import "std/entry" as entry;
type State = struct {output: String, completed: Bool};
def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
export def run = entry.run(config, entry.no_ees, fn(ctx) {
    let initial: State = {output, completed: 'False};
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (
                {output: state.output, completed: 'True},
                [actor.reply(request.id, 'String(state.output))],
            ),
            'EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    (initial, reduce)
});"#,
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);
    let run = telora(&cwd)
        .args(["run", "@src/app:run"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "\"42\"");
    let check = telora(&cwd)
        .args(["check", "@src/lib"])
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
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&string.stdout),
        String::from_utf8_lossy(&string.stderr),
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

    let private = telora(&cwd)
        .args(["query", "exports", "std/_rt"])
        .output()
        .unwrap();
    assert!(!private.status.success());
    let records = jsonl(&private.stdout);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["record"], "diagnostic");
    assert_eq!(records[0]["message"], "unknown built-in module \"std/_rt\"");
}

#[test]
fn query_modules_lists_the_crate_view_as_stable_jsonl() {
    let cwd = fixture();
    let dependency = cwd.join("dependency");
    fs::create_dir_all(cwd.join("src/bin")).unwrap();
    fs::create_dir_all(dependency.join("src/bin")).unwrap();
    fs::write(cwd.join("src/lib.telora"), "0").unwrap();
    fs::write(cwd.join("src/_local.telora"), "0").unwrap();
    fs::write(cwd.join("src/local-native.telora"), "0").unwrap();
    fs::write(cwd.join("src/bin/main.telora"), "0").unwrap();
    fs::write(cwd.join("tests/query.telora"), "0").unwrap();
    fs::write(dependency.join("src/public.telora"), "0").unwrap();
    fs::write(dependency.join("src/_hidden.telora"), "0").unwrap();
    fs::write(dependency.join("src/bin/tool.telora"), "0").unwrap();
    fs::write(
        cwd.join("telora-config.json"),
        r#"{"version":1,"members":[".","dependency"]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("telora-crate.json"),
        r#"{"name":"app","modules":["@src/_local","@src/lib","@src/local-native"],"dependencies":["dep"]}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("telora-crate.json"),
        r#"{"name":"dep","modules":["@src/_hidden","@src/public"],"dependencies":[]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("telora-lock.json"),
        r#"{"version":1,"packages":{"app":{"source":{"workspace":""},"modules":["@src/_local","@src/lib","@src/local-native"],"dependencies":["dep"]},"dep":{"source":{"workspace":"dependency"},"modules":["@src/_hidden","@src/public"],"dependencies":[]}}}"#,
    )
    .unwrap();

    let output = telora(&cwd).args(["q", "modules"]).output().unwrap();
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
    assert!(names.contains(&"app/_local"));
    assert!(names.contains(&"app/local-native"));
    assert!(names.contains(&"dep/public"));
    assert!(!names.contains(&"dep/_hidden"));
    assert!(!names.contains(&"dep/bin/tool"));
    assert!(records.iter().all(|record| {
        record["schema"] == "telora.query/v1"
            && record["record"] == "module"
            && record["format"] == "telora"
    }));
    let private = records
        .iter()
        .find(|record| record["module"] == "app/_local")
        .unwrap();
    assert_eq!(private["origin"], "crate");
    assert_eq!(private["visibility"], "private");

    assert!(
        !telora(&cwd)
            .args(["query", "modules", "@src/lib"])
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
        let module_id = format!("@src/{name}");
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
        let module_id = format!("@src/{name}");
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
        .args(["check", "@src/warning"])
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
    assert_eq!(records[0]["module"], "fixture/warning");
    assert_eq!(records[0]["record"], "diagnostic");
    assert_eq!(records[0]["severity"], "warning");
    assert_eq!(records[1]["record"], "summary");
    assert_eq!(records[1]["status"], "ok");
    assert_eq!(records[1]["dependencies"], 0);
}

#[test]
fn eval_writes_contextual_debug_as_stderr_jsonl() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/debug.telora"),
        r#"import "std/value" {Value};
def var = 3;
def observed = var.dbg!("observed");
export def answer: Value = 'Int(observed);"#,
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);
    let run = telora(&cwd)
        .args(["eval", "@src/debug:answer"])
        .output()
        .unwrap();
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
        assert_eq!(record["module"], "fixture/debug");
        assert_eq!(record["line"], 3);
        assert_eq!(record["message"], "observed");
    }
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
        .args(["check", "@src/recursive"])
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
        .args(["query", "exports", "@src/recursive"])
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
        cwd.join("telora-config.json"),
        r#"{"version":1,"members":[".","query-builder"]}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("src/query-builder.telora"),
        "type Plan = struct {sql: String}; export {Plan};",
    )
    .unwrap();
    fs::write(
        cwd.join("telora-crate.json"),
        r#"{"name":"app","modules":[],"dependencies":["query-builder"]}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("telora-crate.json"),
        r#"{"name":"query-builder","modules":["@src/query-builder"],"dependencies":[]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("telora-lock.json"),
        r#"{"version":1,"packages":{"app":{"source":{"workspace":""},"modules":[],"dependencies":["query-builder"]},"query-builder":{"source":{"workspace":"query-builder"},"modules":["@src/query-builder"],"dependencies":[]}}}"#,
    )
    .unwrap();

    let missing_id = "query-builder/src/query-builder";
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
            "query-builder/query-builder",
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
            "query-builder/query-builder",
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
fn public_cli_rejects_physical_paths_and_missing_manifests() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "export def output = 1;").unwrap();
    let physical = telora(&cwd)
        .args(["run", "src/lib.telora"])
        .output()
        .unwrap();
    assert!(!physical.status.success());
    let outside = fixture();
    fs::remove_file(outside.join("telora-config.json")).unwrap();
    let missing = telora(&outside)
        .args(["check", "@src/lib"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("cannot find telora-config.json"));
}

#[test]
fn named_queries_emit_stable_jsonl() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "type Name = String; def hidden = 1; def make: Fn(Int) -> Int = fn(value) { value }; export {Name, make};").unwrap();
    let show = telora(&cwd)
        .args([
            "query",
            "at",
            "@src/lib",
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
        |record| record["schema"] == "telora.query/v1" && record["module"] == "fixture/lib"
    ));
    assert_eq!(records[0]["name"], "Name");
    assert_eq!(records[1]["name"], "make");

    let exports = telora(&cwd)
        .args(["query", "exports", "@src/lib"])
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
        "import \"@src/types\" as types;\nimport \"@src/types\" { Expr };\nexport {types, Expr};\n",
    )
    .unwrap();

    let show = telora(&cwd)
        .args(["query", "at", "@src/lib", "-k", "import"])
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
    assert_eq!(namespace["target"], "fixture/types");
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
        r#"import "@src/model" {Entity, Request};
export {Entity as PublicEntity, Request};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/open.telora"),
        r#"import "@src/model" *;
export {Entity, Request};"#,
    )
    .unwrap();

    let cases = [
        ("@src/model", "Entity"),
        ("@src/selective", "PublicEntity"),
        ("@src/open", "Entity"),
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
        .args(["query", "at", "@src/lib:1:4"])
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
        .args(["query", "at", "@src/lib:1", "-p", "a"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    let bad_kind = telora(&cwd)
        .args(["query", "at", "@src/lib", "-k", "let,"])
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
        .args(["query", "at", "@src/lib", "-p", "value"])
        .output()
        .unwrap();
    assert!(named.status.success());
    let records = jsonl(&named.stdout);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["location"]["line"], 2);
    assert_eq!(records[0]["location"]["column"], 4);
    assert_eq!(records[0]["location"]["end_line"], 2);
    assert_eq!(records[0]["location"]["end_column"], 9);

    for selector in ["@src/lib:2:0", "@src/lib:2:20"] {
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
        .args(["query", "at", "@src/lib:2:15"])
        .output()
        .unwrap();
    assert!(!inside_scalar.status.success());
    assert!(String::from_utf8_lossy(&inside_scalar.stderr).contains("outside"));

    let at_end = telora(&cwd)
        .args(["query", "at", "@src/lib:2:25"])
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
            .args(["query", "at", "@src/lib:0"])
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
        .args(["check", "@test/codec"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    fs::write(
        cwd.join("src/lib.telora"),
        "import \"@test/codec\" as codec; export def output = codec;",
    )
    .unwrap();
    let check = telora(&cwd)
        .args(["check", "@src/lib"])
        .output()
        .unwrap();
    assert!(!check.status.success());
}
