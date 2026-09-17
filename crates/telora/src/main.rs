use clap::{Args, Parser, Subcommand};
use serde_json::json;
use std::env;
use std::path::PathBuf;
use telora_core::DataLimits;
mod eval_cli;
mod build_cli;
mod wasm_cli;
mod source_arg;
mod static_cli;
use telora::static_input;
mod test_cli;
use eval_cli::EvalArgs;
use source_arg::{NamedSource, parse_named_source};
use telora::package_host;

static EXECUTION_OPTIONS: std::sync::OnceLock<(Option<u64>, Option<u64>, bool)> = std::sync::OnceLock::new();
const QUERY_SCHEMA: &str = "telora.query/v1";

struct ExecutionConfig {
    fuel: u64,
    memory_limit: usize,
    report_usage: bool,
    data_limits: DataLimits,
}

fn execution_config() -> ExecutionConfig {
    execution_config_for(telora_core::RuntimeOptions::default()).expect("validated CLI limits")
}

fn execution_config_for(mut runtime: telora_core::RuntimeOptions) -> Result<ExecutionConfig, String> {
    let (fuel, memory, report_usage) = *EXECUTION_OPTIONS.get().unwrap_or(&(None, None, false));
    if let Some(fuel) = fuel { runtime.fuel = fuel; }
    if let Some(memory) = memory { runtime.memory_limit = memory; }
    let (fuel, memory_limit) = runtime.limits()?;
    Ok(ExecutionConfig {
        fuel,
        memory_limit,
        report_usage,
        data_limits: DataLimits::default(),
    })
}

fn main() {
    let cli = Cli::parse();
    EXECUTION_OPTIONS.set((cli.with_fuel, cli.with_memory_limit, cli.report_usage))
        .expect("execution configuration is initialized once");
    match run_cli(cli) {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            emit_stderr(json!({
                "schema": "telora.error/v1",
                "record": "error",
                "message": error,
            }))
            .expect("the CLI error record is JSON serializable");
            std::process::exit(1);
        }
    }
}

#[derive(Parser)]
#[command(name = "telora", version, about = "The Telora language toolchain")]
struct Cli {
    /// Session fuel budget in millions (1 = 1,000,000 fuel).
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..=u64::MAX / 1_000_000))]
    with_fuel: Option<u64>,
    /// Wasm linear memory limit in MiB (1 = 1,048,576 bytes).
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..=(usize::MAX as u64) / (1 << 20)))]
    with_memory_limit: Option<u64>,
    /// Emit an informational JSON diagnostic with execution usage to stderr.
    #[arg(long, global = true)]
    report_usage: bool,
    /// Find telora-config.json upward from this path (default: current directory).
    #[arg(short = 'C', value_name = "CONTEXT")]
    context: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile MainService to a portable Wasm file (experimental publication format).
    Build(build_cli::BuildArgs),
    /// Evaluate one exported Value without an Entry or effect system.
    Eval(EvalArgs),
    /// Transform one JSON input using the module's MainService.
    Run(RunArgs),
    /// Transform JSONL requests using the module's MainService.
    Serve(ServeArgs),
    /// Resolve package sources and rewrite telora-lock.json.
    Lock,
    /// Check modules through type closure or initialization and emit JSONL diagnostics.
    Check(CheckArgs),
    /// Initialize one test module and execute its directly exported Test values.
    Test(TestArgs),
    /// Query module and semantic facts as JSONL.
    #[command(visible_alias = "q")]
    Query(QueryArgs),
    /// Run the Language Server Protocol service over stdio.
    Lsp,
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    application: ApplicationArgs,
}

#[derive(Args)]
struct ApplicationArgs {
    #[arg(value_name = "MODULE")]
    module: String,
    /// Provide a named Value source: NAME=PATH or NAME=(file|stdin)+(json|yaml|toml)://PATH.
    #[arg(long = "source", value_name = "NAME=SOURCE", value_parser = parse_named_source)]
    sources: Vec<NamedSource>,

}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    application: ApplicationArgs,
    /// Request/response transport. The first version supports stdio:// JSONL.
    #[arg(long, value_name = "URI")]
    bind: String,
}

#[derive(Args)]
#[command(
    after_help = "Examples:\n  telora check @src/lib\n  telora -C examples/app check --lib\n  telora check --tests --only-types\n  telora check --lib --tests"
)]
struct CheckArgs {
    /// Solve types without executing tool, property, or runtime code.
    #[arg(long = "only-types")]
    types_only: bool,
    /// Export experimental MIR-derived layouts without execution.
    #[arg(long, hide = true, value_name = "FILENAME")]
    dump_types_layout: Option<PathBuf>,
    /// Check all declared modules in the current crate, including private modules.
    #[arg(long)]
    lib: bool,
    /// Check all modules recursively below the current crate's tests/ directory.
    #[arg(long)]
    tests: bool,
    /// Canonical module selector, such as @src/lib, @test/compiler, or std/string.
    #[arg(value_name = "MODULE_ID", required_unless_present_any = ["lib", "tests"], conflicts_with_all = ["lib", "tests"])]
    module_id: Option<String>,
}

#[derive(Args)]
struct TestArgs {
    /// Path below tests/, without .telora (for example parser/expressions).
    #[arg(value_name = "NAME", value_parser = parse_test_name)]
    name: String,
}

fn parse_test_name(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.contains(['\\', ':', '@', '*', '?', '[', ']'])
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || std::path::Path::new(value).extension().is_some()
    {
        return Err("expected a normalized test name below tests/, without a file suffix".into());
    }
    Ok(value.to_owned())
}

#[derive(Args)]
#[command(
    after_help = "Examples:\n  telora query modules\n  telora q modules -p std/\n  telora query exports @src/lib\n  telora query at @src/lib -k type,def -p Query\n  telora query at @src/lib:13:0"
)]
struct QueryArgs {
    #[command(subcommand)]
    command: QueryCommand,
}

#[derive(Subcommand)]
enum QueryCommand {
    /// List this crate's public/private modules, built-ins, and external public modules.
    Modules(QueryModulesArgs),
    /// Query a module's public interface.
    Exports(QueryExportsArgs),
    /// Query local symbols in a module, or semantic facts at a source position.
    At(QueryAtArgs),
}

#[derive(Args)]
struct QueryModulesArgs {
    /// Filter canonical module IDs by a literal substring.
    #[arg(short = 'p', long = "pattern", value_name = "SUBSTRING", value_parser = non_empty)]
    pattern: Option<String>,
}

#[derive(Args)]
struct QueryExportsArgs {
    /// Module selector, such as @src/lib or std/string.
    #[arg(value_name = "MODULE_ID")]
    module_id: String,
    /// Filter public export names by a literal substring.
    #[arg(short = 'p', long = "pattern", value_name = "SUBSTRING", value_parser = non_empty)]
    pattern: Option<String>,
}

#[derive(Args)]
struct QueryAtArgs {
    /// Module ID with an optional one-based line and zero-based UTF-8 column.
    #[arg(value_name = "MODULE_ID[:LINE[:COLUMN]]", value_parser = parse_module_selector)]
    selector: ModuleSelector,
    /// Filter local symbol names by a literal substring; invalid with a position.
    #[arg(short = 'p', long = "pattern", value_name = "SUBSTRING", value_parser = non_empty)]
    pattern: Option<String>,
    /// Query only these definition kinds: type, let, def, import.
    #[arg(short = 'k', long = "kind", value_name = "KINDS", value_parser = parse_kinds)]
    kinds: Option<KindSet>,
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
struct QueryPosition {
    line: usize,
    column: Option<usize>,
}

#[derive(Clone)]
struct ModuleSelector {
    module_id: String,
    position: Option<QueryPosition>,
}

fn non_empty(value: &str) -> Result<String, String> {
    (!value.is_empty())
        .then(|| value.to_owned())
        .ok_or_else(|| "pattern must not be empty".into())
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

fn parse_module_selector(value: &str) -> Result<ModuleSelector, String> {
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 3 || parts[0].is_empty() {
        return Err("expected MODULE_ID[:LINE[:COLUMN]]".into());
    }
    if parts.len() == 1 {
        return Ok(ModuleSelector {
            module_id: parts[0].to_owned(),
            position: None,
        });
    }
    let line = parts[1]
        .parse::<usize>()
        .map_err(|_| format!("invalid line {:?}", parts[1]))?;
    if line == 0 {
        return Err("line must be positive".into());
    }
    let column = parts
        .get(2)
        .map(|raw| {
            raw.parse::<usize>()
                .map_err(|_| format!("invalid column {raw:?}"))
        })
        .transpose()?;
    Ok(ModuleSelector {
        module_id: parts[0].to_owned(),
        position: Some(QueryPosition { line, column }),
    })
}

fn run_cli(cli: Cli) -> Result<i32, String> {
    let context = command_context(cli.context)?;
    match cli.command {
        Command::Build(arguments) => build_cli::execute(context, arguments),
        Command::Eval(arguments) => eval_cli::run(context, arguments),
        Command::Run(arguments) => wasm_cli::run::execute(context, arguments.application, false),
        Command::Serve(arguments) => {
            if arguments.bind != "stdio://" {
                return Err("serve supports only stdio://".into());
            }
            wasm_cli::run::execute(context, arguments.application, true)
        }
        Command::Lock => package_host::lock(&context)
            .and_then(|path| emit(json!(display_host_path(&path))).map(|()| 0)),
        Command::Check(arguments) => check_command(context, arguments, "telora.check/v1"),
        Command::Test(arguments) => test_cli::run(context, &arguments.name),
        Command::Query(arguments) => static_cli::query(context, arguments),
        Command::Lsp => lsp_command(context).map(|()| 0),
    }
}

fn lsp_command(root: PathBuf) -> Result<(), String> {
    telora::lsp::run_stdio(root).map_err(|error| error.to_string())
}

// Display only: filesystem operations keep their canonical verbatim paths.
fn display_host_path(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(local) = text.strip_prefix(r"\\?\") {
            return local.to_owned();
        }
    }
    text.into_owned()
}

fn command_context(context: Option<PathBuf>) -> Result<PathBuf, String> {
    context
        .map_or_else(env::current_dir, Ok)
        .map_err(|error| format!("cannot determine context: {error}"))
}

fn check_command(context: PathBuf, arguments: CheckArgs, schema: &str) -> Result<i32, String> {
    static_cli::check(context, arguments, schema)
}

fn kind_name(kind: ShowKind) -> &'static str {
    match kind {
        ShowKind::Type => "type",
        ShowKind::Let => "let",
        ShowKind::Def => "def",
        ShowKind::Import => "import",
    }
}
fn emit(record: serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(&record).map_err(|e| e.to_string())?
    );
    Ok(())
}
fn emit_stderr(record: serde_json::Value) -> Result<(), String> {
    eprintln!(
        "{}",
        serde_json::to_string(&record).map_err(|error| error.to_string())?
    );
    Ok(())
}
