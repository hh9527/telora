use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use telora_core::{
    DebugEvent, DebugSink, DefinitionKind, Engine, EngineConfig, FactState, LoadedModule, Location,
    Quota, Value, WorkspaceModuleState, WorkspaceSnapshot, parse_json,
};

const EVALUATION_FUEL: usize = 1_000_000;
const STACK_SLOTS: usize = 65_536;
const ALLOCATION_BYTES: u64 = 256 * 1024 * 1024;

fn engine_config() -> EngineConfig {
    EngineConfig {
        module_quota: Quota::new(EVALUATION_FUEL, STACK_SLOTS, ALLOCATION_BYTES),
        session_quota: Quota::new(EVALUATION_FUEL, STACK_SLOTS, ALLOCATION_BYTES),
    }
}

fn engine() -> Engine {
    Engine::new(engine_config()).with_debug_sink(Arc::new(StderrDebugSink))
}

struct StderrDebugSink;

#[derive(Serialize)]
struct DebugRecord<'a> {
    name: &'a str,
    repr: &'a str,
    module: &'a str,
    line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

impl DebugSink for StderrDebugSink {
    fn emit(&self, event: DebugEvent) {
        let record = DebugRecord {
            name: &event.name,
            repr: &event.repr,
            module: &event.module,
            line: event.line,
            message: event.message.as_deref(),
        };
        if let Ok(record) = serde_json::to_string(&record) {
            eprintln!("{record}");
        }
    }
}

fn main() {
    if let Err(error) = run_cli(Cli::parse()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[derive(Parser)]
#[command(name = "telora", version, about = "The Telora language toolchain")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Run(RunArgs),
    Exec {
        #[arg(long, required = true)]
        dry_run: bool,
        module_id: String,
        #[arg(last = true)]
        arguments: Vec<String>,
    },
    Build {
        #[arg(long, required = true)]
        dry_run: bool,
        module_id: String,
    },
    Check {
        module_id: String,
    },
    Show(ShowArgs),
    Lsp,
}

#[derive(Args)]
struct RunArgs {
    #[arg(required_unless_present = "standalone", conflicts_with = "standalone", value_parser = binary_name)]
    binary: Option<String>,
    #[arg(short = 'C', value_name = "CONTEXT", conflicts_with = "standalone")]
    context: Option<PathBuf>,
    #[arg(short = 'S', value_name = "FILE", conflicts_with_all = ["binary", "context"])]
    standalone: Option<PathBuf>,
    #[arg(long)]
    input: Option<String>,
}

#[derive(Args)]
struct ShowArgs {
    module_id: String,
    #[arg(short = 'p', long = "pattern", value_parser = non_empty)]
    pattern: Option<String>,
    #[arg(short = 'k', long = "kind", value_parser = parse_kinds, conflicts_with = "exports")]
    kinds: Option<KindSet>,
    #[arg(long, conflicts_with_all = ["kinds", "at"])]
    exports: bool,
    #[arg(long, value_parser = parse_position, conflicts_with_all = ["pattern", "kinds", "exports"])]
    at: Option<ShowPosition>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ShowKind {
    Type,
    Let,
    Def,
    Import,
}

#[derive(Clone)]
struct KindSet(Vec<ShowKind>);

#[derive(Clone, Copy)]
struct ShowPosition {
    line: usize,
    column: Option<usize>,
}

fn non_empty(value: &str) -> Result<String, String> {
    (!value.is_empty())
        .then(|| value.to_owned())
        .ok_or_else(|| "pattern must not be empty".into())
}

fn binary_name(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.ends_with(".telora")
    {
        return Err("binary name must be a single name without path separators or .telora".into());
    }
    Ok(value.to_owned())
}

fn parse_kinds(value: &str) -> Result<KindSet, String> {
    let mut kinds = value
        .split(',')
        .map(|item| match item {
            "type" => Ok(ShowKind::Type),
            "let" => Ok(ShowKind::Let),
            "def" => Ok(ShowKind::Def),
            "import" => Ok(ShowKind::Import),
            _ => Err(format!("unknown definition kind {item:?}")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if kinds.is_empty() {
        return Err("kind list must not be empty".into());
    }
    kinds.sort();
    kinds.dedup();
    Ok(KindSet(kinds))
}

fn parse_position(value: &str) -> Result<ShowPosition, String> {
    let mut parts = value.split(':');
    let parse = |part: Option<&str>, name: &str| -> Result<usize, String> {
        let raw = part.ok_or_else(|| format!("missing {name}"))?;
        let number = raw
            .parse::<usize>()
            .map_err(|_| format!("invalid {name} {raw:?}"))?;
        (number > 0)
            .then_some(number)
            .ok_or_else(|| format!("{name} must be positive"))
    };
    let line = parse(parts.next(), "line")?;
    let column = parts
        .next()
        .map(|raw| parse(Some(raw), "column"))
        .transpose()?;
    if parts.next().is_some() {
        return Err("position must be line or line:column".into());
    }
    Ok(ShowPosition { line, column })
}

fn run_cli(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Run(arguments) => run_command(arguments),
        Command::Exec {
            dry_run: _,
            module_id,
            arguments,
        } => exec_command(&module_id, arguments),
        Command::Build {
            dry_run: _,
            module_id,
        } => build_command(&module_id),
        Command::Check { module_id } => check_command(&module_id),
        Command::Show(arguments) => show_command(arguments),
        Command::Lsp => lsp_command(),
    }
}

fn lsp_command() -> Result<(), String> {
    let root = env::current_dir()
        .map_err(|error| format!("cannot determine current directory: {error}"))?;
    telora::lsp::run_stdio(root, engine_config()).map_err(|error| error.to_string())
}

fn run_command(arguments: RunArgs) -> Result<(), String> {
    let mut bindings = BTreeMap::new();
    if let Some(input) = arguments.input.as_deref() {
        bindings.insert("input".into(), read_input(input)?);
    }
    let engine = engine();
    let module = if let Some(path) = arguments.standalone {
        engine.load_standalone(path, bindings)
    } else {
        let context = arguments
            .context
            .map_or_else(env::current_dir, Ok)
            .map_err(|error| format!("cannot determine context: {error}"))?;
        let module_id = format!(
            "@bin/{}.telora",
            arguments.binary.expect("required by clap")
        );
        engine.load_module_id(context, &module_id, bindings)
    }
    .map_err(|error| error.to_string())?;
    let exports = engine.execute(&module).map_err(|error| error.to_string())?;
    let result = select_host_entry(&module, exports, "run", "output")?;
    println!("{result}");
    Ok(())
}

fn select_host_entry(
    module: &LoadedModule,
    module_value: Value,
    mode: &str,
    name: &str,
) -> Result<Value, String> {
    if !module.uses_explicit_exports() {
        return Ok(module_value);
    }
    let Value::Dict(exports) = module_value else {
        return Err(format!(
            "telora {mode} expected an explicit export record, found {}",
            module_value.type_name()
        ));
    };
    exports.get(name).cloned().ok_or_else(|| {
        format!("telora {mode} requires the explicit export {name:?} in the selected root")
    })
}

fn exec_command(module_id: &str, request_args: Vec<String>) -> Result<(), String> {
    let engine = engine();
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let pending = engine
        .prepare_module_id_with_arguments(&cwd, module_id, request_args)
        .map_err(|error| error.to_string())?;
    let argument = pending.entry_argument();
    let entry = engine
        .load_entry_id(&cwd, module_id, "entry/exec.telora", BTreeMap::new())
        .map_err(|error| format!("cannot load exec entry: {error}"))?;
    let exports = engine.execute(&entry).map_err(|error| error.to_string())?;
    let entry_function = select_host_entry(&entry, exports, "exec", "entry")?;
    let result = engine
        .invoke(&entry, &entry_function, &[argument])
        .map_err(|error| format!("exec entry failed: {error}"))?;
    let Value::Dict(result) = result else {
        return Err("exec entry result must be a record".into());
    };
    if result.shape().fields() != ["exec_opts", "install"] {
        return Err("exec entry result must contain exactly exec_opts and install".into());
    }
    let install = expect_string_export(&result, "install")?;
    let exec_opts = expect_string_export(&result, "exec_opts")?;
    println!(r#"{{"install":{install},"exec_opts":{exec_opts}}}"#);
    Ok(())
}

fn expect_string_export<'a>(exports: &'a telora_core::Dict, name: &str) -> Result<&'a str, String> {
    match exports.get(name) {
        Some(Value::String(value)) => Ok(value),
        Some(value) => Err(format!(
            "exec entry export {name:?} must be a String, found {}",
            value.type_name()
        )),
        None => Err(format!("exec entry omitted export {name:?}")),
    }
}

fn build_command(module_id: &str) -> Result<(), String> {
    let engine = engine();
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let module = engine
        .load_module_id(cwd, module_id, BTreeMap::new())
        .map_err(|error| error.to_string())?;
    let exports = engine.execute(&module).map_err(|error| error.to_string())?;
    let entry = select_host_entry(&module, exports, "build", "build")?;
    let plan = engine
        .invoke(&module, &entry, &[])
        .map_err(|error| error.to_string())?;
    println!("{}", canonical_build_json(&plan)?);
    Ok(())
}

fn canonical_build_json(value: &Value) -> Result<String, String> {
    fn expect_dict<'a>(
        value: &'a Value,
        path: &str,
        fields: &[&str],
    ) -> Result<&'a telora_core::Dict, String> {
        let Value::Dict(dict) = value else {
            return Err(format!(
                "{path} must be a Dict, found {}",
                value.type_name()
            ));
        };
        if !dict
            .shape()
            .fields()
            .iter()
            .map(String::as_str)
            .eq(fields.iter().copied())
        {
            return Err(format!("{path} has an invalid field shape"));
        }
        Ok(dict)
    }

    fn expect_string<'a>(value: &'a Value, path: &str) -> Result<&'a str, String> {
        let Value::String(value) = value else {
            return Err(format!(
                "{path} must be a String, found {}",
                value.type_name()
            ));
        };
        Ok(value)
    }

    fn write_string(output: &mut String, value: &str) {
        output.push('"');
        for character in value.chars() {
            match character {
                '"' => output.push_str("\\\""),
                '\\' => output.push_str("\\\\"),
                '\n' => output.push_str("\\n"),
                '\r' => output.push_str("\\r"),
                '\t' => output.push_str("\\t"),
                character if character.is_control() => {
                    write!(output, "\\u{:04x}", u32::from(character)).unwrap();
                }
                character => output.push(character),
            }
        }
        output.push('"');
    }

    fn validate_path(path: &str, location: &str) -> Result<(), String> {
        if path.is_empty()
            || path.starts_with('/')
            || path.contains('\\')
            || path
                .split('/')
                .any(|component| matches!(component, "" | "." | ".."))
        {
            return Err(format!(
                "{location} must be a normalized relative path using / separators"
            ));
        }
        Ok(())
    }

    let plan = expect_dict(value, "OutputPlan", &["files"])?;
    let Value::Array(files) = plan.get("files").expect("field checked") else {
        return Err("OutputPlan.files must be an Array".into());
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut output = String::from("{\"files\":[");
    for (index, artifact) in files.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        let Value::Tagged { tag, payload } = artifact else {
            return Err(format!(
                "OutputPlan.files[{index}] must be an Artifact Tagged value"
            ));
        };
        if tag.name() != "TextFile" {
            return Err(format!(
                "OutputPlan.files[{index}] has unknown Artifact variant {:?}",
                tag.name()
            ));
        }
        let location = format!("OutputPlan.files[{index}].TextFile");
        let file = expect_dict(payload, &location, &["content", "path"])?;
        let content = expect_string(
            file.get("content").expect("field checked"),
            &format!("{location}.content"),
        )?;
        let path = expect_string(
            file.get("path").expect("field checked"),
            &format!("{location}.path"),
        )?;
        validate_path(path, &format!("{location}.path"))?;
        if !seen.insert(path) {
            return Err(format!("OutputPlan contains duplicate path {path:?}"));
        }
        output.push_str("{\"TextFile\":{\"content\":");
        write_string(&mut output, content);
        output.push_str(",\"path\":");
        write_string(&mut output, path);
        output.push_str("}}");
    }
    output.push_str("]}");
    Ok(output)
}

fn check_command(module_id: &str) -> Result<(), String> {
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let module = engine()
        .load_module_id(cwd, module_id, BTreeMap::new())
        .map_err(|error| error.to_string())?;
    println!("ok ({} dependencies)", module.dependencies.len());
    Ok(())
}

fn show_command(arguments: ShowArgs) -> Result<(), String> {
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let workspace = engine()
        .recover_workspace_id(cwd, &arguments.module_id)
        .map_err(|error| error.to_string())?;
    let root = workspace
        .modules()
        .iter()
        .find(|module| module.name == arguments.module_id)
        .ok_or_else(|| {
            format!(
                "selected module {:?} is absent from the workspace",
                arguments.module_id
            )
        })?;
    if let Some(position) = arguments.at {
        show_at(&workspace, root.id, &arguments.module_id, position)
    } else if arguments.exports {
        show_exports(
            &workspace,
            root.id,
            &arguments.module_id,
            arguments.pattern.as_deref(),
        )
    } else {
        show_definitions(
            &workspace,
            root.id,
            &arguments.module_id,
            arguments.pattern.as_deref(),
            arguments.kinds.as_ref().map(|set| set.0.as_slice()),
        )
    }
}

fn kind_of(kind: DefinitionKind) -> Option<ShowKind> {
    match kind {
        DefinitionKind::Type => Some(ShowKind::Type),
        DefinitionKind::Let => Some(ShowKind::Let),
        DefinitionKind::DefinitionSlot => Some(ShowKind::Def),
        DefinitionKind::Import => Some(ShowKind::Import),
        _ => None,
    }
}
fn kind_name(kind: ShowKind) -> &'static str {
    match kind {
        ShowKind::Type => "type",
        ShowKind::Let => "let",
        ShowKind::Def => "def",
        ShowKind::Import => "import",
    }
}
fn authority(state: &FactState) -> &'static str {
    if matches!(state, FactState::Known) {
        "authoritative"
    } else {
        "recovery"
    }
}
fn location_json(workspace: &WorkspaceSnapshot, location: Location) -> serde_json::Value {
    let source = workspace.sources().get(location.source);
    let start = source.position(location.start);
    let end = source.position(location.end);
    json!({"line":start.line,"column":start.column,"end_line":end.line,"end_column":end.column})
}
fn emit(record: serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(&record).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn show_definitions(
    workspace: &WorkspaceSnapshot,
    module: telora_core::WorkspaceModuleId,
    module_name: &str,
    pattern: Option<&str>,
    kinds: Option<&[ShowKind]>,
) -> Result<(), String> {
    let mut definitions = workspace
        .definitions()
        .iter()
        .filter(|d| d.module == module && d.top_level)
        .filter_map(|d| kind_of(d.kind).map(|kind| (d, kind)))
        .filter(|(d, kind)| {
            pattern.is_none_or(|p| d.name.contains(p)) && kinds.is_none_or(|ks| ks.contains(kind))
        })
        .collect::<Vec<_>>();
    definitions.sort_by_key(|(d, kind)| (&d.name, *kind, d.location.start));
    for (d, kind) in definitions {
        let ty = d
            .scheme
            .clone()
            .or_else(|| d.ty.value.and_then(|id| workspace.types().display(id)));
        emit(
            json!({"schema":"telora.show/v1","module":module_name,"record":"definition","authority":authority(&d.ty.state),"name":d.name,"kind":kind_name(kind),"type":ty,"location":location_json(workspace,d.location)}),
        )?;
    }
    Ok(())
}
fn show_exports(
    workspace: &WorkspaceSnapshot,
    module: telora_core::WorkspaceModuleId,
    module_name: &str,
    pattern: Option<&str>,
) -> Result<(), String> {
    let authority = match workspace.module(module).map(|item| item.state) {
        Some(WorkspaceModuleState::Known) => "authoritative",
        _ => "recovery",
    };
    let mut exports = workspace.exports_of(module);
    exports.retain(|e| pattern.is_none_or(|p| e.name.contains(p)));
    exports.sort_by(|a, b| a.name.cmp(&b.name));
    for export in exports {
        emit(
            json!({"schema":"telora.show/v1","module":module_name,"record":"export","authority":authority,"name":export.name,"type":workspace.types().display(export.ty)}),
        )?;
    }
    Ok(())
}
fn show_at(
    workspace: &WorkspaceSnapshot,
    module: telora_core::WorkspaceModuleId,
    module_name: &str,
    at: ShowPosition,
) -> Result<(), String> {
    let source_id = workspace
        .module(module)
        .and_then(|m| m.source)
        .ok_or_else(|| "selected module has no source".to_owned())?;
    let source = workspace.sources().get(source_id);
    let (start, end) = if let Some(column) = at.column {
        let offset = source
            .offset(at.line, column)
            .ok_or_else(|| format!("position {}:{} is outside {module_name}", at.line, column))?;
        (offset, offset)
    } else {
        let start = source
            .offset(at.line, 1)
            .ok_or_else(|| format!("line {} is outside {module_name}", at.line))?;
        let end = source
            .offset(at.line + 1, 1)
            .unwrap_or(source.text().byte_len() as u32);
        (start, end)
    };
    let intersects = |loc: Location| {
        loc.source == source_id
            && if at.column.is_some() {
                loc.start <= start && start <= loc.end
            } else {
                loc.start < end && start <= loc.end
            }
    };
    for d in workspace
        .definitions()
        .iter()
        .filter(|d| d.module == module && intersects(d.location))
    {
        if let Some(kind) = kind_of(d.kind) {
            emit(
                json!({"schema":"telora.show/v1","module":module_name,"record":"definition","authority":authority(&d.ty.state),"name":d.name,"kind":kind_name(kind),"type":d.scheme.clone().or_else(||d.ty.value.and_then(|id|workspace.types().display(id))),"location":location_json(workspace,d.location)}),
            )?;
        }
    }
    for r in workspace
        .references()
        .iter()
        .filter(|r| r.module == module && intersects(r.location))
    {
        emit(
            json!({"schema":"telora.show/v1","module":module_name,"record":"reference","authority":if r.definition.is_some()||r.external{"authoritative"}else{"recovery"},"name":r.name,"resolved":r.definition.is_some(),"external":r.external,"location":location_json(workspace,r.location)}),
        )?;
    }
    for e in workspace
        .expressions()
        .iter()
        .filter(|e| e.module == module && intersects(e.location))
    {
        emit(
            json!({"schema":"telora.show/v1","module":module_name,"record":"expression","authority":"debug","state":format!("{:?}",e.ty.state),"type":e.ty.value.and_then(|id|workspace.types().display(id)),"location":location_json(workspace,e.location)}),
        )?;
    }
    Ok(())
}

fn read_input(path: &str) -> Result<Value, String> {
    let (source_name, source) = if path == "-" {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| format!("cannot read standard input: {error}"))?;
        ("<stdin>".to_owned(), source)
    } else {
        let source = fs::read_to_string(path)
            .map_err(|error| format!("cannot read input {}: {error}", Path::new(path).display()))?;
        (path.to_owned(), source)
    };
    parse_json(&source_name, &source).map_err(|error| error.to_string())
}
