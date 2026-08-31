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
    assert_eq!(
        serde_json::from_slice::<Value>(&locked.stdout).unwrap(),
        cwd.join("telora-lock.json").to_string_lossy().as_ref()
    );
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
fn run_with_sqlite_query_actor_drives_an_ees_call() {
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
        cwd.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/entry" as entry;
import "std/sqlite-query" as sqlite;

def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
def ees: entry.Ees = {
    vars: {"tenant": "[a-z][a-z0-9-]{0,31}"},
    models: [sqlite.model("catalog", "user-data:{tenant}/catalog.sqlite")],
};

type State = enum {'Ready, 'Waiting};
export def run = entry.run(config, ees, fn(ctx) {
    let initial: State = 'Ready;
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match (state, event) {
            ('Ready, 'Request(request)) => (
                'Waiting,
                [actor.ees_call("query", request.id, sqlite.query(
                    "catalog",
                    "SELECT name, score FROM items WHERE score > ? ORDER BY score DESC",
                    ['Int(1)],
                ))],
            ),
            ('Waiting, 'EesReply(reply)) => match reply.result {
                'Ok(value) => ('Ready, [actor.reply(reply.request_id, value)]),
                'Err(message) => fail!("SQLite query failed", message),
            },
            _ => fail!("unexpected actor event", state, event),
        }
    };
    (initial, reduce)
});"#,
    )
    .unwrap();
    refresh_fixture_workspace(&cwd);
    let output = telora(&cwd)
        .args([
            "run",
            "@src/app:run",
            "--ees-var",
            "tenant=hello",
        ])
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
fn run_actor_can_sequence_multiple_ees_replies_through_explicit_state() {
    let cwd = fixture();
    let data = cwd.join("data");
    fs::create_dir_all(&data).unwrap();
    let database = data.join("catalog.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("CREATE TABLE items (score INTEGER); INSERT INTO items VALUES (1), (2), (3);")
        .unwrap();
    drop(connection);
    fs::write(
        cwd.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/entry" as entry;
import "std/sqlite-query" as sqlite;
def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
def ees: entry.Ees = {vars: {}, models: [sqlite.model("catalog", "user-data:catalog.sqlite")]};
type State = enum {'Ready, 'WaitingFirst(String), 'WaitingSecond(String)};
export def run = entry.run(config, ees, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match (state, event) {
            ('Ready, 'Request(request)) => (
                'WaitingFirst(request.id),
                [actor.ees_call("first", request.id, sqlite.query(
                    "catalog", "SELECT MAX(score) AS score FROM items", []
                ))],
            ),
            ('WaitingFirst(request_id), 'EesReply(reply)) => (
                'WaitingSecond(request_id),
                [actor.ees_call("second", request_id, sqlite.query(
                    "catalog", "SELECT MIN(score) AS score FROM items", []
                ))],
            ),
            ('WaitingSecond(request_id), 'EesReply(reply)) => match reply.result {
                'Ok(value) => ('Ready, [actor.reply(request_id, value)]),
                'Err(message) => fail!("second query failed", message),
            },
            _ => fail!("unexpected actor event", state, event),
        }
    };
    let initial: State = 'Ready;
    (initial, reduce)
});"#,
    )
    .unwrap();

    let output = telora(&cwd)
        .args(["run", "@src/app:run"])
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
        serde_json::json!({"columns": ["score"], "rows": [[1]]})
    );
}

#[test]
fn run_actor_rejects_duplicate_call_ids_and_reply_with_active_calls() {
    let cwd = fixture();
    let data = cwd.join("data");
    fs::create_dir_all(&data).unwrap();
    rusqlite::Connection::open(data.join("catalog.sqlite")).unwrap();
    for (case, effects, expected) in [
        (
            "duplicate",
            "[actor.ees_call(\"same\", request.id, query), actor.ees_call(\"same\", request.id, query)]",
            "reused an active EES call id",
        ),
        (
            "early-reply",
            "[actor.ees_call(\"query\", request.id, query), actor.reply(request.id, 'None)]",
            "replied while EES calls were still active",
        ),
        (
            "duplicate-reply",
            "[actor.reply(request.id, 'None), actor.reply(request.id, 'None)]",
            "replied more than once",
        ),
    ] {
        fs::write(
            cwd.join("src/app.telora"),
            format!(
                r#"import "std/actor" as actor;
import "std/entry" as entry;
import "std/sqlite-query" as sqlite;
def config: entry.ContextConfig = {{sources: [], envs: [], args: 'False}};
def ees: entry.Ees = {{vars: {{}}, models: [sqlite.model("catalog", "user-data:catalog.sqlite")]}};
type State = struct {{}};
export def run = entry.run(config, ees, fn(ctx) {{
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {{
        match event {{
            'Request(request) => {{
                let query = sqlite.query("catalog", "SELECT 1", []);
                (state, {effects})
            }},
            'EesReply(_) => fail!("unexpected EES reply"),
        }}
    }};
    ({{}}, reduce)
}});"#
            ),
        )
        .unwrap();
        let output = telora(&cwd)
            .args(["run", "@src/app:run"])
            .env("XDG_DATA_HOME", &data)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{case} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn ees_variables_are_declared_required_and_fully_matched() {
    let cwd = fixture();
    fs::write(
        cwd.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/entry" as entry;
import "std/sqlite-query" as sqlite;
def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
def ees: entry.Ees = {
    vars: {"tenant": "[a-z][a-z0-9-]{0,7}"},
    models: [sqlite.model("catalog", "user-data:{tenant}/catalog.sqlite")],
};

type State = struct {};
export def run = entry.run(config, ees, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (state, [actor.reply(request.id, 'None)]),
            'EesReply(_) => fail!("unexpected EES reply"),
        }
    };
    ({}, reduce)
});"#,
    )
    .unwrap();

    refresh_fixture_workspace(&cwd);
    let missing = telora(&cwd).args(["run", "@src/app:run"]).output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("were not provided"));

    let invalid = telora(&cwd)
        .args(["run", "@src/app:run", "--ees-var", "tenant=INVALID"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("does not match"));

    let unknown = telora(&cwd)
        .args([
            "run",
            "@src/app:run",
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
fn serve_with_sqlite_query_actor_correlates_concurrent_calls() {
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
        cwd.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/array" as array;
import "std/entry" as entry;
import "std/sqlite-query" as sqlite;
def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
def ees: entry.Ees = {vars: {}, models: [sqlite.model("catalog", "user-data:catalog.sqlite")]};

type State = struct {pending: Array(String)};
export def serve = entry.serve(config, ees, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => {
                let query = match request.input {
            'String(_) => sqlite.query("catalog", "SELECT missing FROM absent", []),
                    _ => sqlite.query(
                        "catalog",
                        "SELECT score FROM items WHERE score > ? ORDER BY score",
                        [request.input],
                    ),
                };
                (
                    {pending: array.push(state.pending, request.id)},
                    [actor.ees_call(request.id, request.id, query)],
                )
            },
            'EesReply(reply) => {
                let pending = array.filter(state.pending, fn(id) { id != reply.id });
                match reply.result {
                    'Ok(value) => ({pending}, [actor.reply(reply.request_id, value)]),
                    'Err(message) => fail!("SQLite query failed", message),
                }
            },
        }
    };
    ({pending: []}, reduce)
});"#,
    )
    .unwrap();
    let mut child = telora(&cwd)
        .args([
            "serve",
            "@src/app:serve",
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
        cwd.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/dict" as dict;
import "std/entry" as entry;
import "std/imos" as imos;
def config: entry.ContextConfig = {sources: ["plan"], envs: [], args: 'False};
def ees: entry.Ees = {
    vars: {},
    models: [imos.model("telora-packages", "user-cache:store", "user-data:home")],
};

type State = enum {'Ready, 'Waiting};
export def run = entry.run(config, ees, fn(ctx) {
    let plan = match dict.get(ctx.sources, "plan") {
        'Some(value) => value,
        'None => fail!("missing plan"),
    };
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match (state, event) {
            ('Ready, 'Request(request)) => (
                'Waiting,
                [actor.ees_call(
                    "install",
                    request.id,
                    imos.install_shared("telora-packages", plan),
                )],
            ),
            ('Waiting, 'EesReply(reply)) => match reply.result {
                'Ok(value) => ('Ready, [actor.reply(reply.request_id, value)]),
                'Err(message) => fail!("installation failed", message),
            },
            _ => fail!("unexpected actor event", state, event),
        }
    };
    let initial: State = 'Ready;
    (initial, reduce)
});"#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args([
            "run",
            "@src/app:run",
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
        cwd.join("src/app.telora"),
        r#"import "std/actor" as actor;
import "std/entry" as entry;
import "std/imos" as imos;
import "std/sqlite-query" as sqlite;
def config: entry.ContextConfig = {sources: [], envs: [], args: 'False};
def ees: entry.Ees = {vars: {}, models: [sqlite.model("catalog", "user-data:catalog.sqlite")]};
type State = struct {};
export def run = entry.run(config, ees, fn(ctx) {
    let reduce: Fn(State, actor.Event) -> actor.Transition(State) = fn(state, event) {
        match event {
            'Request(request) => (
                state,
                [actor.ees_call(
                    "install",
                    request.id,
                    imos.install_shared("telora-packages", 'None),
                )],
            ),
            'EesReply(reply) => (state, [actor.reply(reply.request_id, 'None)]),
        }
    };
    ({}, reduce)
});"#,
    )
    .unwrap();
    let output = telora(&cwd)
        .args(["run", "@src/app:run"])
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
