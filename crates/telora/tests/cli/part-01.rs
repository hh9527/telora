#[test]
fn help_lists_the_public_command_surface() {
    let cwd = fixture();
    let help = telora(&cwd).arg("--help").output().unwrap();
    let output = String::from_utf8_lossy(&help.stdout);
    assert!(help.status.success());
    assert!(output.contains("lsp"));
    assert!(!output.contains("ees"));
    assert!(output.contains("query"));
    assert!(output.contains("q"));
    let query_help = telora(&cwd).args(["query", "-h"]).output().unwrap();
    let output = String::from_utf8_lossy(&query_help.stdout);
    assert!(query_help.status.success());
    assert!(output.contains("modules"));
    assert!(output.contains("exports"));
    assert!(output.contains("at"));
    assert!(output.contains("telora q modules -p std/"));
}

#[test]
fn check_preserves_property_provider_alias_and_factory_contracts() {
    let cwd = fixture();
    for definitions in [
        "def provider: Fn(Type, Option(Tag)) -> Tag = fn(target, previous) { Tag(1) }; def alias = provider;",
        "def configure: Fn(Int) -> Fn(Type, Option(Tag)) -> Tag = fn(n) { fn(target, previous) { Tag(n) } }; def alias = configure(1);",
        "def configure = fn(n: Int) { fn(target: Type, previous: Option(Tag)) { Tag(n) } }; def alias = configure(1);",
    ] {
        fs::write(cwd.join("src/provider.telora"), format!(
            "@property(PropertyTarget.Type) type Tag = struct(Int); {definitions} @alias type Item = struct(Int); def requires: for(T: Property(Tag)) Fn(TypeOf(T)) -> Int = fn(target) {{ 1 }}; trait Named {{ name: Fn(Self) -> Int }}; impl(T: Property(Tag)) Named for T {{ name: fn(value) {{ 42 }} }}; export def output = requires((Item).type); export def named = Named.name(Item(1)); export {{ Item }};"
        )).unwrap();
        for arguments in [vec!["check", "@src/provider"], vec!["check", "--only-types", "@src/provider"]] {
            let output = telora(&cwd).args(&arguments).output().unwrap();
            assert!(output.status.success(), "{arguments:?}: {definitions}\n{}\n{}",
                String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
    }
}

#[test]
fn run_and_check_select_logical_roots_from_cwd() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "export def output = \"42\";").unwrap();
    fs::write(
        cwd.join("src/app.telora"),
        r###"import "@src/lib" {output};
import "std/actor" as actor; import "std/value" {Value};
import "std/ees" as ees;
import "std/entry" as entry;
type State = struct {output: String, completed: Bool};
def config: entry.ContextConfig = {sources: [], envs: [], args: False};
export def run = entry.run((State).type, config, ees.none, fn(ctx) {
    let initial: State = {output, completed: False};
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            actor.Event.Request(request) => (
                {output: state.output, completed: True},
                [actor.reply(request.id, Value.String(state.output))],
            ),
            actor.Event.EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    (initial, reduce)
});"###,
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
fn types_only_solves_member_provider_contracts_without_execution() {
    let cwd = fixture();
    fs::write(cwd.join("src/member-provider.telora"), r#"
        @property(PropertyTarget.Field) type Tag = struct(Int);
        type Context = struct { owner: Type, index: Int, name: String, ty: Type };
        def factory = fn(n: Int) {
            fn(context: Context, previous: Option(Tag)) -> Tag {
                fail!("member provider executed")
            }
        };
        def provider = factory(1);
        type Item = struct { @provider value: Int };
        export { Item };
    "#).unwrap();
    let output = telora(&cwd).args(["check", "--only-types", "@src/member-provider"]).output().unwrap();
    assert!(output.status.success(), "{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let output = telora(&cwd).args(["check", "@src/member-provider"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("member provider executed"), "{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    fs::remove_dir_all(cwd).unwrap();
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
            "export def broken = match A { A 1, _ => 2 };",
            &["missing FatArrow"],
        ),
        (
            "two-roots",
            "export def first = (1 + 2; export def second = match A { A 1, _ => 2 };",
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
        "def reject: Fn() -> Result(Int, String) = fn() { Err(\"notice\") }; def checked = reject().ok_or_warn!(); export def output = 1;",
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
export def answer: Value = Value.Int(observed);"#,
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
type Expr = enum { Call(CallExpr), Text(String) };
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
        "type CallExpr = struct {args: Array(Expr)};\ntype Expr = enum {Text(String), Call(CallExpr)};\ntype Box(A) = struct {value: A};\nexport {CallExpr, Expr, Box};\n",
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
    assert_eq!(namespace["type"], "module fixture/types");
    assert_eq!(namespace["state"], "Known");
    assert!(namespace["type_id"].is_number());

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
            "for(EntityId) TypeOf(Entity(EntityId))"
        );
        let request = records
            .iter()
            .find(|record| record["name"] == "Request")
            .unwrap();
        assert_eq!(
            request["type"],
            "for(Id, Subject, Input) TypeOf(Request(Id, Subject, Input))"
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
#[test]
fn types_only_check_skips_execution_but_rejects_type_errors() {
    let cwd = fixture();
    fs::write(cwd.join("src/types-only.telora"), "export def answer = 1 / 0;").unwrap();
    let output = telora(&cwd).args(["check", "--only-types", "@src/types-only"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let records = String::from_utf8(output.stdout).unwrap();
    let summary: Value = serde_json::from_str(records.lines().last().unwrap()).unwrap();
    assert_eq!(summary["types_only"], true);
    assert!(summary["check_seconds"].as_f64().unwrap() >= 0.0);
    assert!(summary["catalog_seconds"].as_f64().unwrap() >= 0.0);
    let output = telora(&cwd).args(["check", "@src/types-only"]).output().unwrap();
    assert!(!output.status.success());
    fs::write(cwd.join("src/types-only.telora"), "export def answer: Int = \"wrong\";").unwrap();
    let output = telora(&cwd).args(["check", "@src/types-only", "--only-types"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Int"));
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn data_contents_are_checked_after_types_only() {
    let cwd = fixture();
    for (extension, valid, invalid) in [
        ("json", "{\"value\":1}", "{"),
        ("toml", "value = 1", "value = ["),
        ("yaml", "value: 1", "value: ["),
    ] {
        let module = format!("@src/data.{extension}");
        fs::write(cwd.join("src/data-user.telora"), format!(
            "import \"std/value\" {{ Value }}; import \"{module}\" {{ data }}; export def result: Value = data;"
        )).unwrap();
        let path = cwd.join(format!("src/data.{extension}"));
        fs::write(&path, invalid).unwrap();
        refresh_fixture_workspace(&cwd);
        for root in ["@src/data-user", module.as_str()] {
            for mode in [vec!["--only-types"], vec!["--dump-types-layout", "layout.json"]] {
                let output = telora(&cwd).arg("check").args(mode).arg(root).output().unwrap();
                assert!(output.status.success(), "{root}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            }
            let output = telora(&cwd).args(["check", root]).output().unwrap();
            assert!(!output.status.success(), "invalid {extension} must fail ordinary check");
        }
        fs::write(&path, valid).unwrap();
        let output = telora(&cwd).args(["check", "@src/data-user"]).output().unwrap();
        assert!(output.status.success(), "{extension}: {}\n{}",
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        fs::write(cwd.join("src/data-user.telora"), format!(
            "import \"{module}\" {{ data }}; export def result: Int = data;"
        )).unwrap();
        let output = telora(&cwd).args(["check", "--only-types", "@src/data-user"]).output().unwrap();
        assert!(!output.status.success(), "data must retain Value's nominal type");
        assert!(String::from_utf8_lossy(&output.stdout).contains("Int"));
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn types_only_and_ordinary_check_agree_on_open_import_resolution() {
    let cwd = fixture();
    fs::write(cwd.join("src/a.telora"), r#"
        export def shared = 1; export def True = 1;
        export type Choice = enum { done, pending }; export Choice.{done};
    "#).unwrap();
    fs::write(cwd.join("src/b.telora"), "export def shared = 2; export def done = 0;").unwrap();
    fs::write(cwd.join("src/bridge.telora"),
        "import \"./a\" *; export { shared, Choice, done };").unwrap();
    for (source, ambiguous) in [
        ("import \"./a\" *; import \"./b\" *; export def result = 0;", false),
        ("import \"./a\" *; import \"./b\" *; export def result = shared;", true),
        ("import \"./a\" *; import \"./b\" *; def shared = 3; export def result = shared;", false),
        ("import \"./a\" *; import \"./b\" *; export def result = do { let shared = 3; shared };", false),
        ("import \"./a\" { shared }; import \"./b\" *; export def result = shared;", false),
        ("import \"./a\" *; import \"./a\" *; export def result = shared;", false),
        ("import \"./a\" *; export def result: Int = True;", false),
        ("import \"./a\" *; import \"std/prelude\" *; export def result = True;", true),
        ("import \"./a\" *; import \"./b\" *; export def result = match Choice.done { done => 1, _ => 0 };", true),
        ("import \"./bridge\" *; export def result = match Choice.done { done => shared, Choice.pending => 0 };", false),
    ] {
        fs::write(cwd.join("src/open.telora"), source).unwrap();
        for types_only in [false, true] {
            let mut command = telora(&cwd);
            command.args(["check", "@src/open"]);
            if types_only { command.arg("--only-types"); }
            let output = command.output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert_eq!(output.status.success(), !ambiguous,
                "types_only={types_only}: {source}\n{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
            if ambiguous { assert!(stdout.contains("ambiguous"), "{stdout}"); }
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn unused_open_import_with_private_trait_implementation_checks_in_both_modes() {
    let cwd = fixture();
    fs::write(cwd.join("src/implementation.telora"), r#"
        trait Score { score: Fn(Self) -> Int };
        impl Score for Int { score: fn(value) { 42 } };
        export def unused = ();
    "#).unwrap();
    fs::write(cwd.join("src/consumer.telora"), r#"
        import "./implementation" *;
        export def result = 1;
    "#).unwrap();
    for types_only in [false, true] {
        let mut command = telora(&cwd);
        command.args(["check", "@src/consumer"]);
        if types_only { command.arg("--only-types"); }
        let output = command.output().unwrap();
        assert!(output.status.success(), "types_only={types_only}: {}\n{}",
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
    fs::remove_dir_all(cwd).unwrap();
}
