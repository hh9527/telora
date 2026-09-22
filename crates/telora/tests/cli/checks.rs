use super::*;

#[test]
fn initialization_diagnostic_separates_rule_subject_and_triggering_root() {
    let cwd = fixture();
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/initialization-origin");
    for name in ["shared", "good", "bad"] {
        fs::copy(
            assets.join(format!("{name}.telora")),
            cwd.join(format!("src/{name}.telora")),
        )
        .unwrap();
    }
    let output = telora(&cwd).args(["check", "--lib"]).output().unwrap();
    assert!(!output.status.success());
    let records = jsonl(&output.stdout);
    let errors = records
        .iter()
        .filter(|r| r["record"] == "diagnostic" && r["severity"] == "error")
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1, "{records:?}");
    let error = errors[0];
    assert_eq!(error["message"], "shared rejection");
    assert_eq!(error["session"], "--lib");
    assert!(error["module"].as_str().unwrap().ends_with("/shared"));
    let root = &error["initialization"];
    assert!(
        root["module"].as_str().unwrap().ends_with("/bad"),
        "{error}"
    );
    assert_eq!(root["name"], "wrong");
    assert!(root["symbol"].as_u64().is_some() && root["node"].as_u64().is_some());
    assert!(
        error["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|label| label["primary"] == false
                && label["source"].as_str().unwrap().ends_with("/bad"))
    );
    assert_eq!(records.last().unwrap()["status"], "error");
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn check_collects_independent_roots_without_running_failed_continuations() {
    let cwd = fixture();
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/check-roots");
    for name in ["main", "helper"] {
        fs::copy(
            assets.join(format!("{name}.telora")),
            cwd.join(format!("src/{name}.telora")),
        )
        .unwrap();
    }
    let output = telora(&cwd).args(["check", "@src/main"]).output().unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let errors = records
        .iter()
        .filter(|r| r["record"] == "diagnostic" && r["severity"] == "error")
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 4, "{text}");
    for message in ["root-local", "root-module", "root-facade", "root-closure"] {
        assert_eq!(
            errors
                .iter()
                .filter(|r| r["message"].as_str().is_some_and(|m| m.contains(message)))
                .count(),
            1,
            "{text}"
        );
    }
    assert!(
        !text.contains("false-continuation") && !text.contains("false-dependent"),
        "{text}"
    );
    assert_eq!(records.last().unwrap()["status"], "error");
    let output = telora(&cwd)
        .args(["eval", "@src/main:answer"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        ["root-local", "root-module", "root-facade", "root-closure"]
            .iter()
            .filter(|message| error.contains(**message))
            .count(),
        1,
        "{error}"
    );
    assert!(
        !error.contains("false-continuation") && !error.contains("false-dependent"),
        "{error}"
    );
    let output = telora(&cwd)
        .args(["check", "--only-types", "@src/main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn check_preserves_property_provider_alias_and_factory_contracts() {
    let cwd = fixture();
    for definitions in [
        "def provider: Fn(Type, Option(Tag)) -> Tag = fn(target, previous) { Tag(1) }; def alias: Fn(Type, Option(Tag)) -> Tag = provider;",
        "def configure: Fn(Int) -> Fn(Type, Option(Tag)) -> Tag = fn(n) { fn(target, previous) { Tag(n) } }; def alias: Fn(Type, Option(Tag)) -> Tag = configure(1);",
        "def configure: Fn(Int) -> Fn(Type, Option(Tag)) -> Tag = fn(n: Int) { fn(target: Type, previous: Option(Tag)) { Tag(n) } }; def alias: Fn(Type, Option(Tag)) -> Tag = configure(1);",
    ] {
        fs::write(cwd.join("src/provider.telora"), format!(
            "@property(PropertyTarget.Type) type Tag = struct(Int); {definitions} @alias pub type Item = struct(Int); def requires: for(T: Property(Tag)) Fn(TypeOf(T)) -> Int = fn(target) {{ 1 }}; trait Named {{ name: Fn(Self) -> Int }}; impl(T: Property(Tag)) Named for T {{ name: fn(value) {{ 42 }} }}; pub def output: Int = requires((Item).type); pub def named: Int = Named.name(Item(1));"
        )).unwrap();
        for arguments in [
            vec!["check", "@src/provider"],
            vec!["check", "--only-types", "@src/provider"],
        ] {
            let output = telora(&cwd).args(&arguments).output().unwrap();
            assert!(
                output.status.success(),
                "{arguments:?}: {definitions}\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn types_only_solves_member_provider_contracts_without_execution() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/member-provider.telora"),
        r#"
        @property(PropertyTarget.Field) type Tag = struct(Int);
        type Context = struct { owner: Type, index: Int, name: String, ty: Type };
        def factory: Fn(Int) -> Fn(Context, Option(Tag)) -> Tag = fn(n: Int) {
            fn(context: Context, previous: Option(Tag)) -> Tag {
                fail!("member provider executed")
            }
        };
        def provider: Fn(Context, Option(Tag)) -> Tag = factory(1);
        type Item = struct { @provider value: Int };
        pub use self::{ Item };
    "#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["check", "--only-types", "@src/member-provider"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = telora(&cwd)
        .args(["check", "@src/member-provider"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("member provider executed"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn check_rejects_concrete_runtime_errors_without_synthetic_finalization() {
    let cwd = fixture();
    let cases = [
        ("failed", "pub def output: Int = fail!(\"boom\", 1);"),
        ("division", "pub def output: Int = 1 / 0;"),
        ("index", "pub def output: Int = [1][2];"),
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
            "pub def broken: Int = match True { True 1, _ => 2 };",
            &["invalid syntax, expected one of: '=>', 'if'"],
        ),
        (
            "two-roots",
            "pub def first: Int = (1 + 2; pub def second: Int = match True { True 1, _ => 2 };",
            &[
                "invalid syntax, expected one of: ',', ')'",
                "invalid syntax, expected one of: '=>', 'if'",
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
        "def reject: Fn() -> Result(Int, String) = fn() { Err(\"notice\") }; def checked: Option(Int) = reject().ok_or_warn!(); pub def output: Int = 1;",
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
    assert_eq!(records[1]["dependencies"], 1);
}

#[test]
fn check_keeps_recursive_type_metadata_inside_the_semantic_boundary() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/recursive.telora"),
        r#"type CallExpr = struct { args: Array(Expr) };
type Expr = enum { Call(CallExpr), Text(String) };
def identity: Fn(Expr) -> Expr = fn(value) { value };
pub use self::{ CallExpr, Expr, identity };"#,
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
fn types_only_check_skips_execution_but_rejects_type_errors() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/types-only.telora"),
        "pub def answer: Int = 1 / 0;",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["check", "--only-types", "@src/types-only"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let records = String::from_utf8(output.stdout).unwrap();
    let summary: Value = serde_json::from_str(records.lines().last().unwrap()).unwrap();
    assert_eq!(summary["types_only"], true);
    assert!(summary["check_seconds"].as_f64().unwrap() >= 0.0);
    assert!(summary["catalog_seconds"].as_f64().unwrap() >= 0.0);
    let output = telora(&cwd)
        .args(["check", "@src/types-only"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    fs::write(
        cwd.join("src/types-only.telora"),
        "pub def answer: Int = \"wrong\";",
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["check", "@src/types-only", "--only-types"])
        .output()
        .unwrap();
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
            "use std::value::{{ Value }}; data data = import({extension}) \"./data.{extension}\"; pub def result: Value = data;"
        )).unwrap();
        let path = cwd.join(format!("src/data.{extension}"));
        fs::write(&path, invalid).unwrap();
        refresh_fixture_workspace(&cwd);
        for root in ["@src/data-user", module.as_str()] {
            for mode in [
                vec!["--only-types"],
                vec!["--dump-types-layout", "layout.json"],
            ] {
                let output = telora(&cwd)
                    .arg("check")
                    .args(mode)
                    .arg(root)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{root}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let output = telora(&cwd).args(["check", root]).output().unwrap();
            assert!(
                !output.status.success(),
                "invalid {extension} must fail ordinary check"
            );
        }
        fs::write(&path, valid).unwrap();
        let output = telora(&cwd)
            .args(["check", "@src/data-user"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{extension}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        fs::write(
            cwd.join("src/data-user.telora"),
            format!("data data = import({extension}) \"./data.{extension}\"; pub def result: Int = data;"),
        )
        .unwrap();
        let output = telora(&cwd)
            .args(["check", "--only-types", "@src/data-user"])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "data must retain Value's nominal type"
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("Int"));
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn types_only_and_ordinary_check_agree_on_explicit_use_resolution() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/a.telora"),
        r#"
        pub def shared: Int = 1; pub def True: Int = 1;
        pub type Choice = enum { done, pending }; pub use Choice::{done};
    "#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/b.telora"),
        "pub def shared: Int = 2; pub def done: Int = 0;",
    )
    .unwrap();
    fs::write(
        cwd.join("src/bridge.telora"),
        "pub use crate::a::{ shared, Choice, done };",
    )
    .unwrap();
    for (source, ambiguous) in [
        (
            "use crate::a::{ shared as left }; use crate::b::{ shared as right }; pub def result: Int = left + right;",
            false,
        ),
        (
            "use crate::a::{ shared }; use crate::b::{ shared }; pub def result: Int = shared;",
            true,
        ),
        (
            "use crate::a::{ shared }; def shared: Int = 3; pub def result: Int = shared;",
            true,
        ),
        (
            "use crate::a::{ shared }; pub def result: Int = do { let shared = 3; shared };",
            false,
        ),
        (
            "use crate::a::{ shared }; use crate::b::{ shared as other }; pub def result: Int = shared + other;",
            false,
        ),
        (
            "use crate::a::{ shared }; use crate::a::{ shared }; pub def result: Int = shared;",
            true,
        ),
        ("use crate::a::{ True }; pub def result: Int = True;", false),
        (
            "use crate::a::{ True as authored }; pub def result: Int = authored;",
            false,
        ),
        (
            "use crate::a::{ Choice, done }; use crate::b::{ done as other }; pub def result: Int = match Choice::done { done => 1, _ => other };",
            false,
        ),
        (
            "use crate::bridge::{ shared, Choice, done }; pub def result: Int = match Choice::done { done => shared, Choice::pending => 0 };",
            false,
        ),
    ] {
        fs::write(cwd.join("src/open.telora"), source).unwrap();
        for types_only in [false, true] {
            let mut command = telora(&cwd);
            command.args(["check", "@src/open"]);
            if types_only {
                command.arg("--only-types");
            }
            let output = command.output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert_eq!(
                output.status.success(),
                !ambiguous,
                "types_only={types_only}: {source}\n{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            if ambiguous {
                assert!(stdout.contains("duplicate definition"), "{stdout}");
            }
        }
    }
    fs::remove_dir_all(cwd).unwrap();
}

#[test]
fn unused_module_with_private_trait_implementation_checks_in_both_modes() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/implementation.telora"),
        r#"
        trait Score { score: Fn(Self) -> Int };
        impl Score for Int { score: fn(value) { 42 } };
        pub def unused: () = ();
    "#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/consumer.telora"),
        r#"
        use crate::implementation as implementation;
        pub def result: Int = 1;
    "#,
    )
    .unwrap();
    for types_only in [false, true] {
        let mut command = telora(&cwd);
        command.args(["check", "@src/consumer"]);
        if types_only {
            command.arg("--only-types");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "types_only={types_only}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(cwd).unwrap();
}
