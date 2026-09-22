use super::*;

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
        dependency.join("src/lib.telora"),
        "mod public; pub use self::{ public };",
    )
    .unwrap();
    fs::write(
        cwd.join("telora-config.json"),
        r#"{"version":1,"members":[".","dependency"]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("telora-crate.json"),
        r#"{"name":"app","dependencies":["dep"]}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("telora-crate.json"),
        r#"{"name":"dep","dependencies":[]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("telora-lock.json"),
        r#"{"version":1,"packages":{"app":{"source":{"workspace":""},"dependencies":["dep"]},"dep":{"source":{"workspace":"dependency"},"dependencies":[]}}}"#,
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
    assert!(names.contains(&"app"));
    assert!(names.contains(&"dep"));
    assert!(!names.contains(&"dep/_hidden"));
    assert!(!names.contains(&"dep/bin/tool"));
    assert!(records.iter().all(|record| {
        record["schema"] == "telora.query/v1"
            && record["record"] == "module"
            && record["format"] == "telora"
    }));
    let root = records
        .iter()
        .find(|record| record["module"] == "app")
        .unwrap();
    assert_eq!(root["origin"], "crate");
    assert_eq!(root["visibility"], "public");

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
        "type Plan = struct {sql: String}; pub use self::{Plan};",
    )
    .unwrap();
    fs::write(
        dependency.join("src/lib.telora"),
        "pub type Root = struct {};",
    )
    .unwrap();
    fs::write(
        cwd.join("telora-crate.json"),
        r#"{"name":"app","dependencies":["query-builder"]}"#,
    )
    .unwrap();
    fs::write(
        dependency.join("telora-crate.json"),
        r#"{"name":"query-builder","dependencies":[]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("telora-lock.json"),
        r#"{"version":1,"packages":{"app":{"source":{"workspace":""},"dependencies":["query-builder"]},"query-builder":{"source":{"workspace":"query-builder"},"dependencies":[]}}}"#,
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
        .args(["query", "at", "query-builder/query-builder", "-p", "Plan"])
        .output()
        .unwrap();
    assert!(found.status.success());
    assert_eq!(jsonl(&found.stdout).len(), 1);

    let no_match = telora(&cwd)
        .args(["query", "at", "query-builder/query-builder", "-p", "Absent"])
        .output()
        .unwrap();
    assert!(no_match.status.success());
    assert!(no_match.stdout.is_empty());
    assert!(no_match.stderr.is_empty());
}

#[test]
fn named_queries_emit_stable_jsonl() {
    let cwd = fixture();
    fs::write(cwd.join("src/lib.telora"), "type Name = String; def hidden: Int = 1; def make: Fn(Int) -> Int = fn(value) { value }; pub use self::{Name, make};").unwrap();
    let show = telora(&cwd)
        .args(["query", "at", "@src/lib", "-p", "a", "-k", "type,def"])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let records = jsonl(&show.stdout);
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record["schema"] == "telora.query/v1" && record["module"] == "fixture")
    );
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
fn query_namespace_uses_reference_exact_module_interfaces() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/types.telora"),
        "pub type CallExpr = struct {args: Array(Expr)};\npub type Expr = enum {Text(String), Call(CallExpr)};\npub type Box(A) = struct {value: A};\n",
    )
    .unwrap();
    fs::write(
        cwd.join("src/lib.telora"),
        "pub mod types;\npub use self::types::{ Expr };\n",
    )
    .unwrap();

    let show = telora(&cwd)
        .args(["query", "at", "@src/lib", "-k", "use"])
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
        r#"pub type Entity(EntityId) = struct {id: EntityId, label: String};
pub type Request(Id, Subject, Input) = struct {id: Id, subject: Subject, input: Input};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/selective.telora"),
        r#"use crate::model::{Entity, Request};
pub use self::{Entity as PublicEntity, Request};"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/open.telora"),
        r#"pub use crate::model::{Entity, Request};"#,
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
        assert_eq!(entity["type"], "for(EntityId) TypeOf(Entity(EntityId))");
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
    fs::write(cwd.join("src/lib.telora"), "pub def answer: Int = 42;\n").unwrap();
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
        "def other: Int = 42;\npub def value: (String, Int) = (\"中\", other);\n",
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
    assert_eq!(records[0]["location"]["column"], 8);
    assert_eq!(records[0]["location"]["end_line"], 2);
    assert_eq!(records[0]["location"]["end_column"], 13);

    for selector in ["@src/lib:2:0", "@src/lib:2:39"] {
        let output = telora(&cwd)
            .args(["query", "at", selector])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if selector.ends_with(":39") {
            let records = jsonl(&output.stdout);
            let reference = records
                .iter()
                .find(|record| record["record"] == "reference" && record["name"] == "other")
                .unwrap();
            assert_eq!(reference["location"]["line"], 2);
            assert_eq!(reference["location"]["column"], 39);
            assert_eq!(reference["location"]["end_column"], 44);
        }
    }
    let inside_scalar = telora(&cwd)
        .args(["query", "at", "@src/lib:2:34"])
        .output()
        .unwrap();
    assert!(!inside_scalar.status.success());
    assert!(String::from_utf8_lossy(&inside_scalar.stderr).contains("outside"));

    let at_end = telora(&cwd)
        .args(["query", "at", "@src/lib:2:44"])
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
