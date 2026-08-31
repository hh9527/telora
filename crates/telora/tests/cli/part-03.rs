#[test]
fn lock_removes_stale_imos_plans_without_remote_sources() {
    let cwd = fixture();
    let plans = cwd.join(".telora/crates-refs");
    fs::create_dir_all(&plans).unwrap();
    fs::write(plans.join("telora-stale.json"), "{}\n").unwrap();
    fs::write(plans.join("host-note.txt"), "keep\n").unwrap();

    let locked = telora(&cwd).arg("lock").output().unwrap();
    assert!(
        locked.status.success(),
        "{}",
        String::from_utf8_lossy(&locked.stderr)
    );
    assert!(!plans.join("telora-stale.json").exists());
    assert!(plans.join("host-note.txt").exists());
}

#[test]
fn ees_serves_install_shared_requests() {
    let root = fixture();
    let home = root.join("ees-refs");
    fs::create_dir(&home).unwrap();
    let mut child = telora(&root)
        .args([
            "ees",
            "--store",
            root.join("ees-store").to_str().unwrap(),
            "--home",
            home.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let plan = serde_json::json!({
        "version": 1,
        "name": "empty.json",
        "key": "empty-v1",
        "items": []
    });
    for request in [
        serde_json::json!({
            "id": "request-1",
            "actor": "imos",
            "operation": "InstallShared",
            "input": {"plan": plan}
        }),
        serde_json::json!({
            "id": "request-2",
            "actor": "imos",
            "operation": "InstallShared",
            "input": {"plan": plan}
        }),
        serde_json::json!({
            "id": "request-bad",
            "actor": "imos",
            "operation": "InstallShared",
            "input": {"plan": {}}
        }),
    ] {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let events = jsonl(&output.stdout);
    assert_eq!(events.len(), 3);
    let first = events
        .iter()
        .find(|event| event["id"] == "request-1")
        .unwrap();
    let second = events
        .iter()
        .find(|event| event["id"] == "request-2")
        .unwrap();
    let failed = events
        .iter()
        .find(|event| event["id"] == "request-bad")
        .unwrap();
    assert_eq!(first["type"], "result");
    assert_eq!(second["type"], "result");
    assert_eq!(first["value"]["root"], second["value"]["root"]);
    assert!(first["value"]["root"].as_str().unwrap().ends_with("/root"));
    assert_eq!(failed["type"], "error");
    assert!(home.join("empty.json").is_file());
}

#[test]
fn run_with_sqlite_query_actor_drives_an_ees_task() {
    let cwd = fixture();
    let data = cwd.join("data");
    fs::create_dir_all(data.join("hello")).unwrap();
    let database = data.join("hello/catalog.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE items (name TEXT, score INTEGER);\n\
             INSERT INTO items VALUES ('low', 1), ('high', 3), ('mid', 2);",
        )
        .unwrap();
    drop(connection);
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/ees" as ees;
import "std/sqlite-query" as sqlite;
import "std/value" {Value};

option "ees.vars" {"tenant": "[a-z][a-z0-9-]{0,31}"};
option "ees.sqlite" {name: "catalog", path: "user-data:{tenant}/catalog.sqlite"};

export def main: Fn(Dict(Value)) -> ees.Task = fn(sources) {
    ees.call(
        sqlite.query(
            "catalog",
            "SELECT name, score FROM items WHERE score > ? ORDER BY score DESC",
            ['Int(1)],
        ),
        fn(result) {
            match result {
                'Ok(value) => ees.done(value),
                'Err(message) => fail!("SQLite query failed", message),
            }
        },
    )
};"#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["run", "main", "--ees-var", "tenant=hello"])
        .env("XDG_DATA_HOME", &data)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({
            "columns": ["name", "score"],
            "rows": [["high", 3], ["mid", 2]],
        })
    );
}

#[test]
fn ees_variables_are_declared_required_and_fully_matched() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/ees" as ees;
import "std/value" {Value};

option "ees.vars" {"tenant": "[a-z][a-z0-9-]{0,7}"};
option "ees.sqlite" {name: "catalog", path: "user-data:{tenant}/catalog.sqlite"};

export def main: Fn(Dict(Value)) -> ees.Task = fn(sources) { ees.done('None) };"#,
    )
    .unwrap();

    let missing = telora(&cwd).args(["run", "main"]).output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("were not provided"));

    let invalid = telora(&cwd)
        .args(["run", "main", "--ees-var", "tenant=INVALID"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("does not match"));

    let unknown = telora(&cwd)
        .args([
            "run",
            "main",
            "--ees-var",
            "tenant=hello",
            "--ees-var",
            "shard=one",
        ])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("is not declared"));
}

#[test]
fn serve_with_sqlite_query_actor_correlates_concurrent_tasks() {
    let cwd = fixture();
    let home = cwd.join("home");
    let data = home.join(".local/share");
    fs::create_dir_all(&data).unwrap();
    let database = data.join("catalog.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("CREATE TABLE items (score INTEGER); INSERT INTO items VALUES (1), (2), (3);")
        .unwrap();
    drop(connection);
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/ees" as ees;
import "std/sqlite-query" as sqlite;
import "std/value" {Value};

option "ees.sqlite" {name: "catalog", path: "user-data:catalog.sqlite"};

export def serve: Fn(Dict(Value)) -> Fn(Value) -> ees.Task = fn(sources) {
    fn(request) {
        let query = match request {
            'String(_) => sqlite.query("catalog", "SELECT missing FROM absent", []),
            _ => sqlite.query("catalog", "SELECT score FROM items WHERE score > ? ORDER BY score", [request]),
        };
        ees.call(
            query,
            fn(result) {
                match result {
                    'Ok(value) => ees.done(value),
                    'Err(message) => fail!("SQLite query failed", message),
                }
            },
        )
    }
};"#,
    )
    .unwrap();
    let mut child = telora(&cwd)
        .args([
            "serve",
            "main",
            "--bind",
            "stdio://",
        ])
        .env("HOME", &home)
        .env_remove("XDG_DATA_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"1\n\"bad\"\n2\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let replies = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 3);
    let failed = replies.iter().find(|reply| reply["error"] == true).unwrap();
    assert_eq!(failed["ok"], serde_json::Value::Null);
    assert_eq!(failed["diagnostics"][0]["message"], "SQLite query failed");
    let mut rows = replies
        .iter()
        .filter(|reply| reply["error"] == false)
        .map(|reply| reply["ok"]["rows"].clone())
        .collect::<Vec<_>>();
    rows.sort_by_key(|value| value.to_string());
    assert_eq!(rows, vec![serde_json::json!([[2], [3]]), serde_json::json!([[3]])]);
}

#[test]
fn application_imos_actor_with_package_name_stays_in_its_bound_root() {
    let cwd = fixture();
    let actor_root = cwd.join("application-materializer");
    let plan = cwd.join("plan.json");
    fs::write(
        &plan,
        r#"{"version":1,"name":"application.json","key":"application-v1","items":[]}"#,
    )
    .unwrap();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/dict" as dict;
import "std/ees" as ees;
import "std/imos" as imos;
import "std/value" {Value};

option "run-ctx.sources" ["plan"];
option "ees.imos" {
    name: "telora-packages",
    home: "user-data:home",
    store: "user-cache:store",
};

export def main: Fn(Dict(Value)) -> ees.Task = fn(sources) {
    let plan = match dict.get(sources, "plan") {
        'Some(value) => value,
        'None => fail!("missing plan"),
    };
    ees.call(imos.install_shared("telora-packages", plan), fn(result) {
        match result {
            'Ok(value) => ees.done(value),
            'Err(message) => fail!("installation failed", message),
        }
    })
};"#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args([
            "run",
            "main",
            "--source",
            &format!("plan={}", plan.display()),
        ])
        .env("XDG_DATA_HOME", &actor_root)
        .env("XDG_CACHE_HOME", &actor_root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let root = serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    assert!(
        Path::new(root["root"].as_str().unwrap()).starts_with(actor_root.join("store")),
        "{root}"
    );
    assert!(actor_root.join("home/application.json").is_file());
    assert!(!cwd.join(".telora/crates-refs/application.json").exists());
}

#[test]
fn application_cannot_address_the_package_actor_without_its_own_binding() {
    let cwd = fixture();
    let data = cwd.join("data");
    fs::create_dir_all(&data).unwrap();
    let database = data.join("catalog.sqlite");
    rusqlite::Connection::open(&database).unwrap();
    fs::write(
        cwd.join("src/bin/main.telora"),
        r#"import "std/ees" as ees;
import "std/imos" as imos;
import "std/value" {Value};
option "ees.sqlite" {name: "catalog", path: "user-data:catalog.sqlite"};
export def main: Fn(Dict(Value)) -> ees.Task = fn(sources) {
    ees.call(imos.install_shared("telora-packages", 'None), fn(result) { ees.done('None) })
};"#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["run", "main"])
        .env("XDG_DATA_HOME", &data)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("undeclared actor"),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
