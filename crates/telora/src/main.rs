use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use telora_core::lir::RegisterId;
use telora_core::{
    CallContext, DataLimits, DebugEvent, DebugSink, EesCall, EesReply,
    NativeError, NativeFunction, Quota, RunHost,
    RunHostFuture, RunTermination, SystemCaps, SystemDataSource, SystemEvent, SystemStdin,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
mod ees_arg;
mod ees_cli;
mod eval_cli;
mod source_arg;
mod static_cli;
use telora::static_input;
mod test_cli;
use ees_arg::{NamedEesVar, collect_ees_models, parse_named_ees_var};
use ees_cli::EesArgs;
use eval_cli::{EvalArgs, EvalWithArgs};
use source_arg::{NamedSource, collect_entry_sources, is_stdin_source, parse_named_source};
use telora::package_host;

const EVALUATION_FUEL: usize = 1_000_000;
const STACK_SLOTS: usize = 65_536;
const ALLOCATION_BYTES: u64 = 256 * 1024 * 1024;
const QUERY_SCHEMA: &str = "telora.query/v1";

struct ExecutionConfig {
    session_quota: Quota,
    data_limits: DataLimits,
}

fn execution_config() -> ExecutionConfig {
    ExecutionConfig {
        session_quota: Quota::new(EVALUATION_FUEL, STACK_SLOTS, ALLOCATION_BYTES),
        data_limits: DataLimits::default(),
    }
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
    match run_cli(Cli::parse()) {
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

enum ReaderEvent {
    Event(SystemEvent),
    Error(String),
}

struct ProcessRunHost {
    source_locators: BTreeMap<String, String>,
    ees: Option<telora_ees::Service>,
    ees_actors: BTreeMap<String, String>,
    ees_active: HashSet<String>,
    ees_vars: Vec<NamedEesVar>,
    sender: mpsc::UnboundedSender<ReaderEvent>,
    receiver: mpsc::UnboundedReceiver<ReaderEvent>,
    cancel: watch::Sender<bool>,
    tasks: JoinSet<(String, Result<(), String>)>,
    finished: bool,
}

impl ProcessRunHost {
    fn new(source_locators: BTreeMap<String, String>, ees_vars: Vec<NamedEesVar>) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let (cancel, _) = watch::channel(false);
        Self {
            source_locators,
            ees: None,
            ees_actors: BTreeMap::new(),
            ees_active: HashSet::new(),
            ees_vars,
            sender,
            receiver,
            cancel,
            tasks: JoinSet::new(),
            finished: false,
        }
    }

    fn source_locator<'a>(&'a self, source: &'a SystemDataSource) -> &'a str {
        self.source_locators
            .get(&source.src)
            .map_or(source.src.as_str(), String::as_str)
    }

    fn receive_event(&mut self, event: ReaderEvent) -> Result<Option<SystemEvent>, String> {
        match event {
            ReaderEvent::Error(error) => Err(error),
            ReaderEvent::Event(event) => {
                if let SystemEvent::EesReply(reply) = &event {
                    self.ees_active.remove(&reply.key);
                }
                Ok(Some(event))
            }
        }
    }
}

fn native_string(
    context: &CallContext<'_, '_>,
    register: RegisterId,
    path: &str,
) -> Result<String, NativeError> {
    context
        .value(register)?
        .as_str()
        .map(|value| value.as_str().to_owned())
        .ok_or_else(|| NativeError::new(format!("{path} must be String")))
}

fn native_field(
    context: &mut CallContext<'_, '_>,
    source: RegisterId,
    field: &str,
) -> Result<RegisterId, NativeError> {
    let destination = context.scratch()?;
    context.copy_field(destination, source, field)?;
    Ok(destination)
}

fn native_dict_fields(
    context: &CallContext<'_, '_>,
    register: RegisterId,
    path: &str,
) -> Result<Vec<String>, NativeError> {
    context
        .value(register)?
        .dict_fields()
        .map(|fields| fields.into_iter().map(str::to_owned).collect())
        .ok_or_else(|| NativeError::new(format!("{path} must be Dict")))
}

fn prepare_system_resources(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let caps = context.argument(0)?;
    let _value_owner = context.argument(1)?;
    let prepared_data = context.argument(2)?;
    let prepared_keys = native_dict_fields(context, prepared_data, "prepared data sources")?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();

    let data_requests = native_field(context, caps, "data_srcs")?;
    let mut data_fields = Vec::new();
    for key in native_dict_fields(context, data_requests, "SystemCaps.data_srcs")? {
        if prepared_keys.contains(key.as_str()) {
            let item = context.scratch()?;
            context.copy_field(item, prepared_data, &key)?;
            data_fields.push((key, item));
            continue;
        }
        let request = native_field(context, data_requests, &key)?;
        let src_register = native_field(context, request, "src")?;
        let src = native_string(context, src_register, "DataSrc.src")?;
        let data = context.scratch()?;
        let default = native_field(context, request, "default")?;
        if context.value(default)?.as_atom().as_deref() == Some("None") {
            return Err(NativeError::new(format!(
                "cannot read data source {src:?}: file does not exist"
            )));
        }
        context.copy_tagged_payload(data, default)?;
        let item = context.scratch()?;
        context.make_dict(item, &[("data".into(), data), ("src".into(), src_register)])?;
        data_fields.push((key, item));
    }
    let data = context.scratch()?;
    context.make_dict(data, &data_fields)?;

    let text_requests = native_field(context, caps, "text_srcs")?;
    let mut text_fields = Vec::new();
    for key in native_dict_fields(context, text_requests, "SystemCaps.text_srcs")? {
        let request = native_field(context, text_requests, &key)?;
        let src_register = native_field(context, request, "src")?;
        let src = native_string(context, src_register, "TextSrc.src")?;
        let text = context.scratch()?;
        match fs::read_to_string(&src) {
            Ok(source) => context.set_string(text, source)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let default = native_field(context, request, "default")?;
                if context.value(default)?.as_atom().as_deref() == Some("None") {
                    return Err(NativeError::new(format!(
                        "cannot read text source {src:?}: {error}"
                    )));
                }
                context.copy_tagged_payload(text, default)?;
            }
            Err(error) => {
                return Err(NativeError::new(format!(
                    "cannot read text source {src:?}: {error}"
                )));
            }
        }
        let item = context.scratch()?;
        context.make_dict(item, &[("data".into(), text), ("src".into(), src_register)])?;
        text_fields.push((key, item));
    }
    let texts = context.scratch()?;
    context.make_dict(texts, &text_fields)?;

    let requested_vars = native_field(context, caps, "vars")?;
    let var_count = context
        .value(requested_vars)?
        .sequence_len()
        .ok_or_else(|| NativeError::new("SystemCaps.vars must be Array(String)"))?;
    let mut var_fields = Vec::new();
    for index in 0..var_count {
        let name_register = context.scratch()?;
        context.copy_sequence_item(name_register, requested_vars, index)?;
        let name = native_string(context, name_register, "SystemCaps.vars item")?;
        match env::var(&name) {
            Ok(value) => {
                let value_register = context.scratch()?;
                context.set_string(value_register, value)?;
                var_fields.push((name, value_register));
            }
            Err(env::VarError::NotPresent) => {}
            Err(error) => {
                return Err(NativeError::new(format!(
                    "cannot read variable {name:?}: {error}"
                )));
            }
        }
    }
    let vars = context.scratch()?;
    context.make_dict(vars, &var_fields)?;

    let stdin_mode = native_field(context, caps, "stdin")?;
    let stdin = context.scratch()?;
    match context.value(stdin_mode)?.as_atom().as_deref() {
        Some("Text") => {
            let mut source = String::new();
            Read::read_to_string(&mut io::stdin(), &mut source).map_err(|error| {
                NativeError::new(format!("cannot read standard input: {error}"))
            })?;
            let tag = context.scratch()?;
            let payload = context.scratch()?;
            context.set_atom(tag, "Some")?;
            context.set_string(payload, source)?;
            context.make_tagged(stdin, tag, payload)?;
        }
        Some("Lined" | "Null") => context.set_none(stdin)?,
        _ => return Err(NativeError::new("SystemCaps.stdin is invalid")),
    }

    context.make_dict(
        context.result(),
        &[
            ("data".into(), data),
            ("texts".into(), texts),
            ("vars".into(), vars),
            ("stdin".into(), stdin),
        ],
    )
}

impl RunHost for ProcessRunHost {
    fn resources_provider(&mut self) -> NativeFunction {
        NativeFunction::new(
            "telora.cli.prepare_system_resources",
            3,
            prepare_system_resources,
        )
    }

    fn ees_actors(&self) -> BTreeMap<String, String> {
        self.ees_actors.clone()
    }

    fn configure(&mut self, caps: SystemCaps) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let collected = collect_ees_models(
                &caps.ees_vars,
                &caps.ees_models,
                std::mem::take(&mut self.ees_vars),
            )?;
            if caps.ees != collected.actors {
                return Err(format!(
                    "EES actor declarations do not match model configs: declared {:?}, configured {:?}",
                    caps.ees, collected.actors
                ));
            }
            self.ees_actors = collected.actors;
            self.ees = match collected.manifest {
                Some(manifest) => Some(
                    telora_ees::Service::open(manifest)
                        .await
                        .map_err(|error| format!("cannot initialize application EES: {error:#}"))?,
                ),
                None => None,
            };
            if caps.stdin != SystemStdin::Null
                && caps
                    .data_sources
                    .values()
                    .any(|source| is_stdin_source(self.source_locator(source)))
            {
                return Err(
                    "standard input cannot be both an event stream and a data source".into(),
                );
            }
            if caps.stdin == SystemStdin::Lined {
                let sender = self.sender.clone();
                let mut cancel = self.cancel.subscribe();
                self.tasks.spawn(async move {
                    let mut lines = BufReader::new(tokio::io::stdin()).lines();
                    loop {
                        let line = tokio::select! {
                            biased;
                            changed = cancel.changed() => {
                                if changed.is_ok() && *cancel.borrow() {
                                    return ("<stdin>".into(), Ok(()));
                                }
                                continue;
                            }
                            line = lines.next_line() => line,
                        };
                        match line {
                            Ok(Some(line)) => {
                                let _ = sender
                                    .send(ReaderEvent::Event(SystemEvent::StdinLine(Some(line))));
                            }
                            Ok(None) => {
                                let _ =
                                    sender.send(ReaderEvent::Event(SystemEvent::StdinLine(None)));
                                return ("<stdin>".into(), Ok(()));
                            }
                            Err(error) => {
                                let message = format!("cannot read standard input: {error}");
                                let _ = sender.send(ReaderEvent::Error(message.clone()));
                                return ("<stdin>".into(), Err(message));
                            }
                        }
                    }
                });
            }
            Ok(())
        })
    }

    fn read_data_source(
        &mut self,
        source: &SystemDataSource,
        max_bytes: usize,
    ) -> RunHostFuture<'_, Result<Option<String>, String>> {
        let src = source.src.clone();
        let locator = self.source_locator(source).to_owned();
        Box::pin(async move {
            let max_read = u64::try_from(max_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
            if is_stdin_source(&locator) {
                tokio::io::stdin()
                    .take(max_read)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|error| format!("cannot read data source {src:?}: {error}"))?;
            } else {
                let path = match locator.split_once("://") {
                    Some((scheme, path)) if scheme.starts_with("file+") => path,
                    _ => locator.as_str(),
                };
                let file = match fs::File::open(path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => {
                        return Err(format!("cannot read data source {src:?}: {error}"));
                    }
                };
                file.take(max_read)
                    .read_to_end(&mut bytes)
                    .map_err(|error| format!("cannot read data source {src:?}: {error}"))?;
            }
            if bytes.len() > max_bytes {
                return Err(format!(
                    "data source exceeds file_size limit ({} > {max_bytes})",
                    bytes.len()
                ));
            }
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| format!("cannot read data source {src:?}: {error}"))
        })
    }

    fn ees_call(&mut self, call: EesCall) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if !self.ees_actors.contains_key(&call.actor) {
                return Err(format!("EES actor {:?} is not configured", call.actor));
            }
            if !self.ees_active.insert(call.key.clone()) {
                return Err(format!("EES call key {:?} is already active", call.key));
            }
            let Some(service) = self.ees.clone() else {
                self.ees_active.remove(&call.key);
                return Err("EES service is not configured".into());
            };
            let key = call.key.clone();
            let sender = self.sender.clone();
            self.tasks.spawn(async move {
                let event = service
                    .dispatch(
                        telora_ees::Call {
                            id: key.clone(),
                            actor: call.actor,
                            operation: call.operation,
                            input: call.input,
                        },
                        None,
                    )
                    .await;
                let result = event.into_value();
                let sent = sender
                    .send(ReaderEvent::Event(SystemEvent::EesReply(EesReply {
                        key: key.clone(),
                        result,
                    })))
                    .map_err(|_| "EES reply channel disconnected".to_owned());
                (format!("ees:{key}"), sent)
            });
            Ok(())
        })
    }

    fn next_event(&mut self) -> RunHostFuture<'_, Result<Option<SystemEvent>, String>> {
        Box::pin(async move {
            loop {
                if let Ok(event) = self.receiver.try_recv() {
                    return self.receive_event(event);
                }
                if self.tasks.is_empty() {
                    return Ok(None);
                }
                tokio::select! {
                    event = self.receiver.recv() => {
                        let event = event.ok_or_else(|| {
                            "child event channel disconnected".to_owned()
                        })?;
                        return self.receive_event(event);
                    }
                    joined = self.tasks.join_next(), if !self.tasks.is_empty() => {
                        let Some(joined) = joined else { continue };
                        let (_, result) = joined.map_err(|error| {
                            format!("Host task failed: {error}")
                        })?;
                        result?;
                    }
                }
            }
        })
    }

    fn finish(&mut self) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if self.finished {
                return Ok(());
            }
            self.finished = true;
            let _ = self.cancel.send(true);
            let mut first_error = None;
            while let Some(joined) = self.tasks.join_next().await {
                match joined {
                    Ok((_, Ok(()))) => {}
                    Ok((_, Err(error))) if first_error.is_none() => first_error = Some(error),
                    Err(error) if first_error.is_none() => {
                        first_error = Some(format!("Host task failed: {error}"));
                    }
                    _ => {}
                }
            }
            first_error.map_or(Ok(()), Err)
        })
    }
}

impl Drop for ProcessRunHost {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        self.tasks.abort_all();
    }
}

#[derive(Parser)]
#[command(name = "telora", version, about = "The Telora language toolchain")]
struct Cli {
    /// Find telora-config.json upward from this path (default: current directory).
    #[arg(short = 'C', value_name = "CONTEXT")]
    context: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Evaluate one exported Value without an Entry or effect system.
    Eval(EvalArgs),
    /// Invoke one pure context function and write its Value result.
    EvalWith(EvalWithArgs),
    /// Submit one request to an application reducer service.
    Run(RunArgs),
    /// Process transport requests with one application reducer service.
    Serve(ServeArgs),
    #[command(hide = true)]
    Ees(EesArgs),
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
    #[arg(value_name = "MODULE:EXPORT", value_parser = parse_application_selector)]
    selector: ApplicationSelector,
    #[arg(long)]
    best_effort: bool,
    /// Provide a named Value source: NAME=PATH or NAME=(file|stdin)+(json|yaml|toml)://PATH.
    #[arg(long = "source", value_name = "NAME=SOURCE", value_parser = parse_named_source)]
    sources: Vec<NamedSource>,
    /// Bind a variable declared by the selected ees.Config value: NAME=VALUE.
    #[arg(long = "ees-var", value_name = "NAME=VALUE", value_parser = parse_named_ees_var)]
    ees_vars: Vec<NamedEesVar>,
    #[arg(last = true, value_name = "ARG")]
    args: Vec<String>,
}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    application: ApplicationArgs,
    /// Request/response transport. The first version supports stdio:// JSONL.
    #[arg(long, value_name = "URI")]
    bind: String,
}

#[derive(Clone)]
struct ApplicationSelector {
    module_id: String,
    export: String,
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

fn parse_application_selector(value: &str) -> Result<ApplicationSelector, String> {
    let (module_id, export) = value
        .rsplit_once(':')
        .ok_or_else(|| "expected MODULE:EXPORT".to_owned())?;
    if module_id.is_empty() {
        return Err("application module selector must not be empty".into());
    }
    let mut characters = export.chars();
    if !characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err("application export name must be an identifier".into());
    }
    Ok(ApplicationSelector {
        module_id: module_id.to_owned(),
        export: export.to_owned(),
    })
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
    if let Command::Ees(arguments) = &cli.command {
        return ees_cli::run(arguments, cli.context.is_some());
    }
    let context = command_context(cli.context)?;
    match cli.command {
        Command::Eval(arguments) => eval_cli::run(context, arguments),
        Command::EvalWith(arguments) => eval_cli::run_with(context, arguments),
        Command::Run(arguments) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the run Host: {error}"))?
            .block_on(run_command(context, "run", arguments.application)),
        Command::Serve(arguments) => {
            if arguments.bind != "stdio://" {
                return Err(format!(
                    "unsupported serve binding {:?}; the first version supports stdio://",
                    arguments.bind
                ));
            }
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("cannot start the serve Host: {error}"))?
                .block_on(run_command(context, "serve", arguments.application))
        }
        Command::Ees(_) => unreachable!("EES returns before workspace context discovery"),
        Command::Lock => package_host::lock(&context)
            .and_then(|path| emit(json!(path.to_string_lossy())).map(|()| 0)),
        Command::Check(arguments) => check_command(context, arguments, "telora.check/v1"),
        Command::Test(arguments) => test_cli::run(context, &arguments.name),
        Command::Query(arguments) => static_cli::query(context, arguments),
        Command::Lsp => lsp_command(context).map(|()| 0),
    }
}

fn lsp_command(root: PathBuf) -> Result<(), String> {
    telora::lsp::run_stdio(root).map_err(|error| error.to_string())
}

async fn run_command(
    context: PathBuf,
    entry: &str,
    arguments: ApplicationArgs,
) -> Result<i32, String> {
    let entry_sources = collect_entry_sources(arguments.sources.clone())?;
    if entry == "serve"
        && entry_sources
            .locators
            .values()
            .any(|locator| is_stdin_source(locator))
    {
        return Err("serve --bind stdio:// reserves standard input for JSONL requests".into());
    }
    let module_id = &arguments.selector.module_id;
    let mode = match entry {
        "run" => telora_core::codegen::RunMode::Run,
        "serve" => telora_core::codegen::RunMode::Serve,
        _ => return Err(format!("unknown entry mode {entry:?}")),
    };
    let mut inventory = static_input::Inventory::new(&context, module_id.starts_with("std/"))?;
    let application = inventory.select(module_id)?;
    let mut mir = inventory.solve_run(&application, &arguments.selector.export, mode)?;
    let adapter_conflicts = mir.type_conflicts.iter().filter_map(|conflict| conflict.location)
        .filter(|location| mir.sources.get(location.source).name.as_ref() == "std/_entry/adapter")
        .collect::<std::collections::BTreeSet<_>>();
    for diagnostic in &mut mir.diagnostics {
        if diagnostic.severity == telora_core::source::Severity::Error
            && diagnostic.labels.iter().any(|label| label.primary && adapter_conflicts.contains(&label.location)) {
            diagnostic.message = format!("entry export {:?}: expected {}(State); {}", arguments.selector.export, if entry == "run" { "Run" } else { "Serve" }, diagnostic.message);
        }
    }
    let static_failed = mir.diagnostics.iter().any(|d| d.severity == telora_core::source::Severity::Error);
    if arguments.best_effort {
        for diagnostic in &mir.diagnostics {
            emit_stderr(static_cli::diagnostic(&mir, "telora.run/v1", module_id, diagnostic))?;
        }
        if static_failed {
            emit_stderr(json!({"schema": "telora.run/v1", "module": module_id, "record": "summary", "status": "error"}))?;
            return Ok(1);
        }
    } else if static_failed {
        return Err(mir.diagnostics.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n"));
    }
    let telora_core::mir::ModuleTarget::Bound(root) = mir.roots[0] else { return Err("entry adapter module is unresolved".into()) };
    let symbol = *mir.exports[root.index()].iter().find(|s| mir.symbols[s.index()].name == "configure")
        .ok_or("entry adapter has no configuration export")?;
    let artifact = mir.seal().and_then(|sealed| telora_core::codegen::compile_run(sealed, symbol))
        .map_err(|errors| errors.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n"))?;
    let config = execution_config();
    let linked = telora_core::execution_link::link_entry_with_data(artifact, |link| inventory.read_data(link, config.data_limits.file_size))
        .map_err(|errors| errors.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n"))?;
    let mut host = ProcessRunHost::new(entry_sources.locators, arguments.ees_vars);
    let outcome = telora_core::Vm::new().with_debug_sink(Arc::new(StderrDebugSink))
        .execute_run(
            linked,
            mode,
            &arguments.args,
            &entry_sources.entry,
            &mut host,
            config.session_quota,
            config.data_limits,
            &mut mir.sources,
        )
        .await?;
    io::stdout()
        .write_all(outcome.output.as_bytes())
        .and_then(|()| io::stdout().flush())
        .map_err(|error| format!("cannot write Entry output: {error}"))?;
    match outcome.termination {
        RunTermination::Exit(code) => i32::try_from(code)
            .map_err(|_| format!("Entry exit status {code} is outside the Host range")),
    }
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
