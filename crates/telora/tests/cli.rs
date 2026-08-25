use serde_json::Value;
use std::fs;
use std::io::Write;
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
    fs::write(path.join("telora-deps.json"), r#"{"name":"fixture"}"#).unwrap();
    path
}

fn telora(cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_telora"));
    command.current_dir(cwd);
    command
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
