use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

mod backend_surface;
mod declaration_shapes;
mod usage;
mod wasm;

fn fixture() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("telora-cli-{unique}"));
    fs::create_dir_all(path.join("src")).unwrap();
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
    let lib = root.join("src/lib.telora");
    const GENERATED_ROOT: &str = "# generated test root\n";
    let generated = !lib.exists()
        || fs::read_to_string(&lib).is_ok_and(|source| source.starts_with(GENERATED_ROOT));
    if generated {
        let mut modules = fs::read_dir(root.join("src"))
            .unwrap()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                (path.is_file() && path.extension().is_some_and(|ext| ext == "telora"))
                    .then(|| path.file_stem()?.to_str().map(str::to_owned))
                    .flatten()
            })
            .filter(|name| {
                name != "lib"
                    && name.chars().enumerate().all(|(index, ch)| {
                        ch == '_'
                            || ch.is_ascii_alphanumeric() && (index > 0 || !ch.is_ascii_digit())
                    })
            })
            .collect::<Vec<_>>();
        modules.sort();
        let source = if modules.is_empty() {
            format!("{GENERATED_ROOT}export type Fixture = struct {{}};\n")
        } else {
            format!(
                "{GENERATED_ROOT}{}export {{ {} }};\n",
                modules
                    .iter()
                    .map(|name| format!("mod {name};\n"))
                    .collect::<String>(),
                modules.join(", ")
            )
        };
        fs::write(&lib, source).unwrap();
    }
    fs::write(
        root.join("telora-crate.json"),
        serde_json::to_vec(&serde_json::json!({
            "name": "fixture",
            "dependencies": [],
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("telora-lock.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "packages": {
                "fixture": {
                    "source": {"workspace":""},
                    "dependencies": [],
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

fn jsonl(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn input_command(mut command: Command, input: &[u8]) -> std::process::Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn execute_value(cwd: &Path, mode: &str, selector: &str) -> std::process::Output {
    let mut command = telora(cwd);
    command.args([mode, selector]);
    input_command(command, b"null")
}

fn runtime_source(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/runtime")
            .join(name),
    )
    .unwrap()
}

mod source_runtime;

mod checks;
mod codegen_stack_safety;
mod command_surface;
mod context;
mod entry_services;
mod evaluation;
mod language;
mod queries;
mod static_mir;
mod test_command;
