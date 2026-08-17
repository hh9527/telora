use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio as ProcessStdio};
use std::sync::Arc;
use telora_core::{
    ChildExit, ChildOptions, ChildOutputMode, ChildSpawnResult, ChildStdinMode, ChildText,
    DebugEvent, DebugSink, DefinitionKind, Engine, EngineConfig, FactState, Location, Quota,
    RunHost, RunHostFuture, RunTermination, SpawnStdioChild, SystemEvent, Value,
    WorkspaceModuleState, WorkspaceSnapshot, parse_json,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

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
    match run_cli(Cli::parse()) {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

enum ReaderEvent {
    Event(SystemEvent),
    Error(String),
}

struct ProcessRunHost {
    children: HashMap<String, Option<mpsc::UnboundedSender<Option<String>>>>,
    sender: mpsc::UnboundedSender<ReaderEvent>,
    receiver: mpsc::UnboundedReceiver<ReaderEvent>,
    cancel: watch::Sender<bool>,
    tasks: JoinSet<(String, Result<(), String>)>,
    finished: bool,
}

impl ProcessRunHost {
    fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let (cancel, _) = watch::channel(false);
        Self {
            children: HashMap::new(),
            sender,
            receiver,
            cancel,
            tasks: JoinSet::new(),
            finished: false,
        }
    }

    fn command(options: &ChildOptions) -> ProcessCommand {
        let mut command = ProcessCommand::new(&options.bin);
        if let Some(cwd) = &options.cwd {
            command.current_dir(cwd);
        }
        if options.clear_env {
            command.env_clear();
        }
        for (name, value) in &options.envs {
            match value {
                Some(value) => {
                    command.env(name, value);
                }
                None => {
                    command.env_remove(name);
                }
            }
        }
        command
    }

    fn output_stdio(mode: ChildOutputMode) -> ProcessStdio {
        match mode {
            ChildOutputMode::PipedLine | ChildOutputMode::PipedToEnd => ProcessStdio::piped(),
            ChildOutputMode::Inherit => ProcessStdio::inherit(),
            ChildOutputMode::Null => ProcessStdio::null(),
        }
    }

    async fn read_stream(
        key: String,
        reader: impl AsyncRead + Unpin,
        mode: ChildOutputMode,
        stderr: bool,
        sender: mpsc::UnboundedSender<ReaderEvent>,
    ) -> Result<(), String> {
        let make_event = |data| {
            let text = ChildText {
                key: key.clone(),
                data,
            };
            if stderr {
                SystemEvent::ChildStderr(text)
            } else {
                SystemEvent::ChildStdout(text)
            }
        };
        match mode {
            ChildOutputMode::PipedLine => {
                let mut lines = BufReader::new(reader).lines();
                while let Some(line) = lines.next_line().await.map_err(|error| {
                    format!("cannot read child {key:?} stream as UTF-8 text: {error}")
                })? {
                    if sender
                        .send(ReaderEvent::Event(make_event(Some(line))))
                        .is_err()
                    {
                        return Ok(());
                    }
                }
            }
            ChildOutputMode::PipedToEnd => {
                let mut reader = BufReader::new(reader);
                let mut text = String::new();
                reader.read_to_string(&mut text).await.map_err(|error| {
                    format!("cannot read child {key:?} stream as UTF-8 text: {error}")
                })?;
                if !text.is_empty()
                    && sender
                        .send(ReaderEvent::Event(make_event(Some(text))))
                        .is_err()
                {
                    return Ok(());
                }
            }
            ChildOutputMode::Inherit | ChildOutputMode::Null => unreachable!(),
        }
        let _ = sender.send(ReaderEvent::Event(make_event(None)));
        Ok(())
    }

    async fn supervise_child(
        request: SpawnStdioChild,
        mut stdin_messages: mpsc::UnboundedReceiver<Option<String>>,
        mut cancel: watch::Receiver<bool>,
        sender: mpsc::UnboundedSender<ReaderEvent>,
    ) -> Result<(), String> {
        let key = request.key.clone();
        let mut command = TokioCommand::from(Self::command(&request.opts));
        command.kill_on_drop(true);
        command.stdin(match request.stdio.stdin {
            ChildStdinMode::Piped => ProcessStdio::piped(),
            ChildStdinMode::Inherit => ProcessStdio::inherit(),
            ChildStdinMode::Null => ProcessStdio::null(),
        });
        command.stdout(Self::output_stdio(request.stdio.stdout));
        command.stderr(Self::output_stdio(request.stdio.stderr));
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = sender.send(ReaderEvent::Event(SystemEvent::ChildSpawnResult(
                    ChildSpawnResult {
                        key,
                        result: Err(format!("cannot spawn {:?}: {error}", request.opts.bin)),
                    },
                )));
                return Ok(());
            }
        };
        let pid = i64::from(child.id().expect("a spawned child has a process id"));
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if *cancel.borrow() {
            let _ = child.start_kill();
            child
                .wait()
                .await
                .map_err(|error| format!("cannot wait for child {key:?}: {error}"))?;
            return Ok(());
        }

        let _ = sender.send(ReaderEvent::Event(SystemEvent::ChildSpawnResult(
            ChildSpawnResult {
                key: key.clone(),
                result: Ok(pid),
            },
        )));

        let mut stream_tasks = JoinSet::new();
        if let Some(stdout) = stdout {
            let stream_sender = sender.clone();
            let stream_key = key.clone();
            stream_tasks.spawn(async move {
                let result = Self::read_stream(
                    stream_key,
                    stdout,
                    request.stdio.stdout,
                    false,
                    stream_sender.clone(),
                )
                .await;
                if let Err(error) = &result {
                    let _ = stream_sender.send(ReaderEvent::Error(error.clone()));
                }
                result
            });
        }
        if let Some(stderr) = stderr {
            let stream_sender = sender.clone();
            let stream_key = key.clone();
            stream_tasks.spawn(async move {
                let result = Self::read_stream(
                    stream_key,
                    stderr,
                    request.stdio.stderr,
                    true,
                    stream_sender.clone(),
                )
                .await;
                if let Err(error) = &result {
                    let _ = stream_sender.send(ReaderEvent::Error(error.clone()));
                }
                result
            });
        }

        let mut stdin_tasks = JoinSet::new();
        if let Some(mut stdin) = stdin {
            let error_sender = sender.clone();
            let stdin_key = key.clone();
            stdin_tasks.spawn(async move {
                while let Some(data) = stdin_messages.recv().await {
                    let Some(data) = data else {
                        return Ok(());
                    };
                    let result = stdin.write_all(data.as_bytes()).await;
                    let result = match result {
                        Ok(()) => stdin.flush().await,
                        Err(error) => Err(error),
                    };
                    if let Err(error) = result {
                        let message = format!("cannot write child {stdin_key:?} stdin: {error}");
                        let _ = error_sender.send(ReaderEvent::Error(message.clone()));
                        return Err(message);
                    }
                }
                Ok(())
            });
        }

        let status = tokio::select! {
            biased;
            changed = cancel.changed() => {
                if changed.is_ok() && *cancel.borrow() {
                    let _ = child.start_kill();
                    child.wait().await.map_err(|error| {
                        format!("cannot wait for child {key:?}: {error}")
                    })?;
                }
                None
            },
            result = child.wait() => Some(result.map_err(|error| {
                format!("cannot wait for child {key:?}: {error}")
            })?),
        };

        stdin_tasks.abort_all();
        while stdin_tasks.join_next().await.is_some() {}

        let mut cancelled = status.is_none() || *cancel.borrow();
        if cancelled {
            stream_tasks.abort_all();
        }
        while !stream_tasks.is_empty() {
            tokio::select! {
                biased;
                changed = cancel.changed(), if !cancelled => {
                    if changed.is_ok() && *cancel.borrow() {
                        cancelled = true;
                        stream_tasks.abort_all();
                    }
                }
                result = stream_tasks.join_next() => {
                    let Some(result) = result else { continue };
                    if !cancelled {
                        result.map_err(|error| {
                            format!("child {key:?} stream task failed: {error}")
                        })??;
                    }
                }
            }
        }

        if let Some(status) = status {
            let exited = match status.code() {
                Some(code) => ChildExit::Code(i64::from(code)),
                None => ChildExit::Signal(exit_signal(&status)),
            };
            let _ = sender.send(ReaderEvent::Event(SystemEvent::ChildExited { key, exited }));
        }
        Ok(())
    }

    fn receive_event(&mut self, event: ReaderEvent) -> Result<Option<SystemEvent>, String> {
        match event {
            ReaderEvent::Error(error) => Err(error),
            ReaderEvent::Event(event) => {
                match &event {
                    SystemEvent::ChildSpawnResult(ChildSpawnResult {
                        key,
                        result: Err(_),
                    })
                    | SystemEvent::ChildExited { key, .. } => {
                        self.children.remove(key);
                    }
                    _ => {}
                }
                Ok(Some(event))
            }
        }
    }
}

#[cfg(unix)]
fn exit_signal(status: &std::process::ExitStatus) -> Option<i64> {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map(i64::from)
}

#[cfg(not(unix))]
fn exit_signal(_status: &std::process::ExitStatus) -> Option<i64> {
    None
}

impl RunHost for ProcessRunHost {
    fn spawn_stdio_child(
        &mut self,
        request: SpawnStdioChild,
    ) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if request.key.is_empty() {
                let _ = self
                    .sender
                    .send(ReaderEvent::Event(SystemEvent::ChildSpawnResult(
                        ChildSpawnResult {
                            key: request.key,
                            result: Err("child key must not be empty".into()),
                        },
                    )));
                return Ok(());
            }
            if self.children.contains_key(&request.key) {
                let _ = self
                    .sender
                    .send(ReaderEvent::Event(SystemEvent::ChildSpawnResult(
                        ChildSpawnResult {
                            key: request.key.clone(),
                            result: Err(format!("child key {:?} is already active", request.key)),
                        },
                    )));
                return Ok(());
            }
            let key = request.key.clone();
            let (stdin_sender, stdin_receiver) = mpsc::unbounded_channel();
            let stdin_sender =
                (request.stdio.stdin == ChildStdinMode::Piped).then_some(stdin_sender);
            self.children.insert(key.clone(), stdin_sender);
            let cancel = self.cancel.subscribe();
            let sender = self.sender.clone();
            self.tasks.spawn(async move {
                let result = Self::supervise_child(request, stdin_receiver, cancel, sender).await;
                (key, result)
            });
            Ok(())
        })
    }

    fn post_stdin(&mut self, text: ChildText) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let stdin = self
                .children
                .get_mut(&text.key)
                .ok_or_else(|| format!("unknown active child {:?}", text.key))?
                .as_ref()
                .ok_or_else(|| format!("child {:?} has no open piped stdin", text.key))?
                .clone();
            let close = text.data.is_none();
            stdin
                .send(text.data)
                .map_err(|_| format!("child {:?} has no open piped stdin", text.key))
                .map(|()| {
                    if close {
                        self.children
                            .get_mut(&text.key)
                            .expect("child was resolved above")
                            .take();
                    }
                })
        })
    }

    fn next_event(&mut self) -> RunHostFuture<'_, Result<Option<SystemEvent>, String>> {
        Box::pin(async move {
            loop {
                if let Ok(event) = self.receiver.try_recv() {
                    return self.receive_event(event);
                }
                if self.children.is_empty() && self.tasks.is_empty() {
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
                        let (key, result) = joined.map_err(|error| {
                            format!("child supervisor task failed: {error}")
                        })?;
                        if let Err(error) = result {
                            self.children.remove(&key);
                            return Err(error);
                        }
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
            self.children.clear();
            let mut first_error = None;
            while let Some(joined) = self.tasks.join_next().await {
                match joined {
                    Ok((_, Ok(()))) => {}
                    Ok((_, Err(error))) if first_error.is_none() => first_error = Some(error),
                    Err(error) if first_error.is_none() => {
                        first_error = Some(format!("child supervisor task failed: {error}"));
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
        self.children.clear();
        self.tasks.abort_all();
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
    Check(CheckArgs),
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
    #[arg(long, value_name = "FILE")]
    entry: Option<PathBuf>,
    #[arg(long)]
    best_effort: bool,
}

#[derive(Args)]
struct CheckArgs {
    module_id: String,
    #[arg(short = 'C', value_name = "CONTEXT")]
    context: Option<PathBuf>,
}

#[derive(Args)]
struct ShowArgs {
    module_id: String,
    #[arg(short = 'C', value_name = "CONTEXT")]
    context: Option<PathBuf>,
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

fn run_cli(cli: Cli) -> Result<i32, String> {
    match cli.command {
        Command::Run(arguments) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the run Host: {error}"))?
            .block_on(run_command(arguments)),
        Command::Check(arguments) => check_command(arguments),
        Command::Show(arguments) => show_command(arguments).map(|()| 0),
        Command::Lsp => lsp_command().map(|()| 0),
    }
}

fn lsp_command() -> Result<(), String> {
    let root = env::current_dir()
        .map_err(|error| format!("cannot determine current directory: {error}"))?;
    telora::lsp::run_stdio(root, engine_config()).map_err(|error| error.to_string())
}

async fn run_command(arguments: RunArgs) -> Result<i32, String> {
    let input = arguments.input.as_deref().map(read_input).transpose()?;
    let context = command_context(arguments.context.clone())?;
    let module_id = arguments
        .binary
        .as_ref()
        .map(|binary| format!("@bin/{binary}.telora"));
    if arguments.best_effort {
        let recovery_engine = Engine::new(engine_config());
        let workspace = if let Some(path) = arguments.standalone.as_deref() {
            recovery_engine.recover_standalone(path)
        } else {
            recovery_engine
                .recover_workspace_id(&context, module_id.as_deref().expect("required by clap"))
        }
        .map_err(|error| error.to_string())?;
        let selected = module_id.as_deref().unwrap_or("@standalone");
        for diagnostic in workspace.diagnostics() {
            emit_stderr(diagnostic_record(
                "telora.run/v1",
                selected,
                &workspace,
                diagnostic,
            ))?;
        }
        let incomplete = workspace
            .modules()
            .iter()
            .filter(|module| module.state != WorkspaceModuleState::Known)
            .map(|module| module.name.as_str())
            .collect::<Vec<_>>();
        let failed = workspace
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.severity == telora_core::source::Severity::Error)
            || !incomplete.is_empty();
        if !incomplete.is_empty() {
            emit_stderr(json!({
                "schema": "telora.run/v1",
                "module": selected,
                "record": "diagnostic",
                "severity": "error",
                "message": format!("Main finalization is incomplete: {}", incomplete.join(", ")),
                "labels": [],
                "notes": [],
            }))?;
        }
        if failed {
            emit_stderr(json!({
                "schema": "telora.run/v1",
                "module": selected,
                "record": "summary",
                "status": "error",
            }))?;
            return Ok(1);
        }
    }
    let engine = engine();
    let pending = if let Some(path) = arguments.standalone {
        engine.prepare_standalone(path)
    } else {
        engine.prepare_module_id(context, module_id.as_deref().expect("required by clap"))
    }
    .map_err(|error| error.to_string())?;
    let mut host = ProcessRunHost::new();
    let outcome = engine
        .run_pending_with_host(pending, input, arguments.entry.as_deref(), &mut host)
        .await
        .map_err(|error| error.to_string())?;
    io::stdout()
        .write_all(outcome.output.as_bytes())
        .and_then(|()| io::stdout().flush())
        .map_err(|error| format!("cannot write Entry output: {error}"))?;
    match outcome.termination {
        RunTermination::Exit(code) => i32::try_from(code)
            .map_err(|_| format!("Entry exit status {code} is outside the Host range")),
        RunTermination::Exec(options) => exec_process(options),
    }
}

#[cfg(unix)]
fn exec_process(options: ChildOptions) -> Result<i32, String> {
    use std::os::unix::process::CommandExt;
    let error = ProcessRunHost::command(&options).exec();
    Err(format!("cannot exec {:?}: {error}", options.bin))
}

#[cfg(not(unix))]
fn exec_process(options: ChildOptions) -> Result<i32, String> {
    let status = ProcessRunHost::command(&options)
        .status()
        .map_err(|error| format!("cannot execute {:?}: {error}", options.bin))?;
    Ok(status.code().unwrap_or(1))
}

fn command_context(context: Option<PathBuf>) -> Result<PathBuf, String> {
    context
        .map_or_else(env::current_dir, Ok)
        .map_err(|error| format!("cannot determine context: {error}"))
}

fn check_command(arguments: CheckArgs) -> Result<i32, String> {
    let context = command_context(arguments.context)?;
    let workspace = engine()
        .recover_workspace_id(context, &arguments.module_id)
        .map_err(|error| error.to_string())?;
    let has_error_diagnostic = workspace
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.severity == telora_core::source::Severity::Error);
    for diagnostic in workspace.diagnostics() {
        let severity = match diagnostic.severity {
            telora_core::source::Severity::Error => "error",
            telora_core::source::Severity::Warning => "warning",
            telora_core::source::Severity::Info => "info",
        };
        let labels = diagnostic
            .labels
            .iter()
            .map(|label| {
                let source = workspace.sources().get(label.location.source);
                json!({
                    "source": source.name.as_ref(),
                    "location": location_json(&workspace, label.location),
                    "message": label.message,
                    "primary": label.primary,
                })
            })
            .collect::<Vec<_>>();
        emit(json!({
            "schema": "telora.check/v1",
            "module": arguments.module_id,
            "record": "diagnostic",
            "severity": severity,
            "message": diagnostic.message,
            "labels": labels,
            "notes": diagnostic.notes,
        }))?;
    }
    let incomplete = workspace
        .modules()
        .iter()
        .filter(|module| module.state != WorkspaceModuleState::Known)
        .map(|module| module.name.as_str())
        .collect::<Vec<_>>();
    if !incomplete.is_empty() {
        emit(json!({
            "schema": "telora.check/v1",
            "module": arguments.module_id,
            "record": "diagnostic",
            "severity": "error",
            "message": format!("module finalization is incomplete: {}", incomplete.join(", ")),
            "labels": [],
            "notes": [],
        }))?;
    }
    // Recovery marks a module Known only after the ordinary strict
    // analyze/compile/evaluate/publish path succeeds. Other states retain the
    // richer best-effort diagnostics but cannot make check succeed.
    let failed = has_error_diagnostic || !incomplete.is_empty();
    emit(json!({
        "schema": "telora.check/v1",
        "module": arguments.module_id,
        "record": "summary",
        "status": if failed { "error" } else { "ok" },
        "dependencies": workspace.modules().len().saturating_sub(1),
    }))?;
    Ok(i32::from(failed))
}

fn show_command(arguments: ShowArgs) -> Result<(), String> {
    let context = command_context(arguments.context)?;
    let workspace = if arguments.module_id.starts_with("std/") {
        engine()
            .recover_builtin_workspace(&arguments.module_id)
            .map_err(|error| error.to_string())?
    } else {
        engine()
            .recover_workspace_id(context, &arguments.module_id)
            .map_err(|error| error.to_string())?
    };
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
        DefinitionKind::DefinitionSlot | DefinitionKind::Native => Some(ShowKind::Def),
        DefinitionKind::NativeType => Some(ShowKind::Type),
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
fn diagnostic_record(
    schema: &str,
    module: &str,
    workspace: &WorkspaceSnapshot,
    diagnostic: &telora_core::source::Diagnostic,
) -> serde_json::Value {
    let severity = match diagnostic.severity {
        telora_core::source::Severity::Error => "error",
        telora_core::source::Severity::Warning => "warning",
        telora_core::source::Severity::Info => "info",
    };
    let labels = diagnostic
        .labels
        .iter()
        .map(|label| {
            let source = workspace.sources().get(label.location.source);
            json!({
                "source": source.name.as_ref(),
                "location": location_json(workspace, label.location),
                "message": label.message,
                "primary": label.primary,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema": schema,
        "module": module,
        "record": "diagnostic",
        "severity": severity,
        "message": diagnostic.message,
        "labels": labels,
        "notes": diagnostic.notes,
    })
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
        if kind == ShowKind::Import && d.import_namespace {
            let target = d
                .import_target
                .and_then(|target| workspace.module(target))
                .map(|module| module.name.as_str());
            emit(
                json!({"schema":"telora.show/v1","module":module_name,"record":"definition","authority":authority(&d.ty.state),"name":d.name,"kind":kind_name(kind),"target":target,"location":location_json(workspace,d.location)}),
            )?;
            continue;
        }
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
