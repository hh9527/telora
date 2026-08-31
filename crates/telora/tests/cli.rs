use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("telora-cli-{unique}"));
    fs::create_dir_all(path.join("src/bin")).unwrap();
    fs::create_dir_all(path.join("src/entry")).unwrap();
    fs::create_dir_all(path.join("tests")).unwrap();
    fs::write(
        path.join("telora-config.json"),
        r#"{"version":1,"members":["."]}"#,
    )
    .unwrap();
    refresh_fixture_workspace(&path);
    path
}

fn telora(cwd: &Path) -> Command {
    if let Some(root) = cwd
        .ancestors()
        .find(|directory| directory.join("telora-config.json").is_file())
    {
        let managed = fs::read_to_string(root.join("telora-config.json"))
            .ok()
            .and_then(|source| serde_json::from_str::<Value>(&source).ok())
            .is_some_and(|config| config["members"] == serde_json::json!(["."]));
        if managed {
            refresh_fixture_workspace(root);
        }
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_telora"));
    command.current_dir(cwd);
    command
}

fn refresh_fixture_workspace(root: &Path) {
    fn modules(root: &Path, directory: &Path, found: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if directory == root && matches!(entry.file_name().to_str(), Some("bin" | "entry"))
                {
                    continue;
                }
                modules(root, &path, found);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let extension = path.extension().and_then(|value| value.to_str());
            if !matches!(extension, Some("telora" | "json" | "yaml" | "yml" | "toml")) {
                continue;
            }
            let mut relative = path.strip_prefix(root).unwrap().to_owned();
            if extension == Some("telora") {
                relative.set_extension("");
            }
            found.push(format!(
                "@src/{}",
                relative.to_string_lossy().replace('\\', "/")
            ));
        }
    }

    let mut declared = Vec::new();
    modules(&root.join("src"), &root.join("src"), &mut declared);
    declared.sort();
    fs::write(
        root.join("telora-crate.json"),
        serde_json::to_vec(&serde_json::json!({
            "name": "fixture",
            "modules": declared,
            "dependencies": [],
        }))
        .unwrap(),
    )
    .unwrap();
    let mut binaries = serde_json::Map::new();
    if let Ok(entries) = fs::read_dir(root.join("src/bin")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("telora") {
                let name = path.file_stem().unwrap().to_string_lossy();
                binaries.insert(
                    format!("fixture/{name}"),
                    serde_json::json!({"root":"fixture","packages":["fixture"]}),
                );
            }
        }
    }
    fs::write(
        root.join("telora-lock.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "packages": {
                "fixture": {
                    "source": {"workspace":""},
                    "modules": declared,
                    "dependencies": [],
                }
            },
            "binaries": binaries,
        }))
        .unwrap(),
    )
    .unwrap();
}

fn write_entry(cwd: &Path, source: impl AsRef<str>) -> std::io::Result<()> {
    let source = source
        .as_ref()
        .replace(
            "def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'False} };",
            "",
        )
        .replace(
            "def legacy_prepare: Fn(rt.SystemOptions) -> rt.SystemCaps = fn(options) { {input: 'True} };",
            "",
        );
    let spawn_child = if source.contains("'SpawnStdioChild") || source.contains("'PostStdin") {
        "'True"
    } else {
        "'False"
    };
    let source = format!(
        r#"{source}
type EntryInitializer = Fn(rt.SystemResources, MainType) -> Tuple([
    State,
    Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]),
]);
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, EntryInitializer])
    = fn(options, env) {{
    (
        {{
            data_srcs: {{}},
            ees: {{}},
            spawn_child: {spawn_child},
            text_srcs: {{}},
            vars: [],
            stdin: 'Null,
        }},
        fn(resources, main) {{ legacy_initialize(main) }},
    )
}};"#
    );
    fs::write(cwd.join("src/entry/test.telora"), source)
}

fn jsonl(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

include!("cli/part-01.rs");
include!("cli/part-02.rs");
include!("cli/part-03.rs");
include!("cli/part-04.rs");
