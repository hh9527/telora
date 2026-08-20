use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio as ProcessStdio};
use std::sync::Arc;
use telora_core::lir::RegisterId;
use telora_core::{
    CallContext, ChildExit, ChildOptions, ChildOutputMode, ChildSpawnResult, ChildStdinMode,
    ChildText, DebugEvent, DebugSink, DefinitionKind, Engine, EngineConfig, FactState, Location,
    NativeError, NativeFunction, PositionEncoding, Quota, RunHost, RunHostFuture, RunTermination,
    SpawnStdioChild, SystemCaps, SystemDataSource, SystemEvent, SystemStdin, TextPosition,
    WorkspaceSnapshot,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

const EVALUATION_FUEL: usize = 1_000_000;
const STACK_SLOTS: usize = 65_536;
const ALLOCATION_BYTES: u64 = 256 * 1024 * 1024;
const QUERY_SCHEMA: &str = "telora.query/v1";

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

    fn configure(&mut self, caps: SystemCaps) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async move {
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
    ) -> RunHostFuture<'_, Result<Option<String>, String>> {
        let src = source.src.clone();
        Box::pin(async move {
            match fs::read_to_string(&src) {
                Ok(source) => Ok(Some(source)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(format!("cannot read data source {src:?}: {error}")),
            }
        })
    }

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
    RunWith(RunWithArgs),
    Check(CheckArgs),
    /// Query module and semantic facts as JSONL.
    #[command(visible_alias = "q")]
    Query(QueryArgs),
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
    best_effort: bool,
    #[arg(last = true, value_name = "ENTRY_ARG")]
    entry_args: Vec<String>,
}

#[derive(Args)]
struct RunWithArgs {
    /// Canonical Entry module selector, such as std/entry/default or @src/serve.entry.telora.
    #[arg(value_name = "ENTRY_MODULE")]
    entry: String,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Args)]
struct CheckArgs {
    module_id: String,
    #[arg(short = 'C', value_name = "CONTEXT")]
    context: Option<PathBuf>,
}

#[derive(Args)]
#[command(
    after_help = "Examples:\n  telora query modules\n  telora q modules -p std/\n  telora query exports @src/lib.telora\n  telora query at @src/lib.telora -k type,def -p Query\n  telora query at @src/lib.telora:13:0"
)]
struct QueryArgs {
    /// Find telora-deps.json upward from this path (default: current directory).
    #[arg(short = 'C', value_name = "CONTEXT", global = true)]
    context: Option<PathBuf>,
    #[command(subcommand)]
    command: QueryCommand,
}

#[derive(Subcommand)]
enum QueryCommand {
    /// List this crate's public/private/native modules and external public modules.
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
    /// Canonical module ID, such as @src/lib.telora or std/string.
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
    match cli.command {
        Command::Run(arguments) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the run Host: {error}"))?
            .block_on(run_command("std/entry/default", arguments)),
        Command::RunWith(arguments) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the run Host: {error}"))?
            .block_on(run_command(&arguments.entry, arguments.run)),
        Command::Check(arguments) => check_command(arguments),
        Command::Query(arguments) => query_command(arguments),
        Command::Lsp => lsp_command().map(|()| 0),
    }
}

fn lsp_command() -> Result<(), String> {
    let root = env::current_dir()
        .map_err(|error| format!("cannot determine current directory: {error}"))?;
    telora::lsp::run_stdio(root, engine_config()).map_err(|error| error.to_string())
}

async fn run_command(entry: &str, arguments: RunArgs) -> Result<i32, String> {
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
        let failed = workspace
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.severity == telora_core::source::Severity::Error);
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
        .run_pending_with_host(pending, entry, &arguments.entry_args, &mut host)
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
    let failed = has_error_diagnostic;
    emit(json!({
        "schema": "telora.check/v1",
        "module": arguments.module_id,
        "record": "summary",
        "status": if failed { "error" } else { "ok" },
        "dependencies": workspace.modules().len().saturating_sub(1),
    }))?;
    Ok(i32::from(failed))
}

enum ModuleQuery {
    Exports {
        pattern: Option<String>,
    },
    Definitions {
        pattern: Option<String>,
        kinds: Option<KindSet>,
    },
    Position(QueryPosition),
}

fn query_command(arguments: QueryArgs) -> Result<i32, String> {
    let context = command_context(arguments.context)?;
    if let QueryCommand::Modules(arguments) = &arguments.command {
        let modules = match engine().module_catalog(context) {
            Ok(modules) => modules,
            Err(error) => {
                emit(json!({
                    "schema": QUERY_SCHEMA,
                    "record": "diagnostic",
                    "authority": "recovery",
                    "severity": "error",
                    "message": error.to_string(),
                }))?;
                return Ok(1);
            }
        };
        for module in modules.into_iter().filter(|module| {
            arguments
                .pattern
                .as_deref()
                .is_none_or(|pattern| module.id.to_string().contains(pattern))
        }) {
            emit(json!({
                "schema": QUERY_SCHEMA,
                "record": "module",
                "module": module.id.to_string(),
                "origin": module.origin.name(),
                "visibility": module.visibility.name(),
                "format": module.format.name(),
            }))?;
        }
        return Ok(0);
    }
    let (module_id, query) = match arguments.command {
        QueryCommand::Exports(arguments) => (
            arguments.module_id,
            ModuleQuery::Exports {
                pattern: arguments.pattern,
            },
        ),
        QueryCommand::At(arguments) => {
            let ModuleSelector {
                module_id,
                position,
            } = arguments.selector;
            let query = if let Some(position) = position {
                if arguments.pattern.is_some() || arguments.kinds.is_some() {
                    return Err(
                        "-p/--pattern and -k/--kind require a module-only query target".into(),
                    );
                }
                ModuleQuery::Position(position)
            } else {
                ModuleQuery::Definitions {
                    pattern: arguments.pattern,
                    kinds: arguments.kinds,
                }
            };
            (module_id, query)
        }
        QueryCommand::Modules(_) => unreachable!("handled above"),
    };
    let workspace = if module_id.starts_with("std/") {
        engine().recover_builtin_workspace(&module_id)
    } else {
        engine().recover_workspace_id(context, &module_id)
    };
    let workspace = match workspace {
        Ok(workspace) => workspace,
        Err(error) => {
            emit(json!({
                "schema": QUERY_SCHEMA,
                "module": module_id,
                "record": "diagnostic",
                "authority": "recovery",
                "severity": "error",
                "message": error.to_string(),
            }))?;
            return Ok(1);
        }
    };
    let root = workspace
        .modules()
        .iter()
        .find(|module| module.name == module_id)
        .ok_or_else(|| {
            format!(
                "selected module {:?} is absent from the workspace",
                module_id
            )
        })?;
    match query {
        ModuleQuery::Exports { pattern } => {
            query_exports(&workspace, root.id, &module_id, pattern.as_deref())
        }
        ModuleQuery::Definitions { pattern, kinds } => query_definitions(
            &workspace,
            root.id,
            &module_id,
            pattern.as_deref(),
            kinds.as_ref().map(|set| set.0.as_slice()),
        ),
        ModuleQuery::Position(position) => {
            query_position(&workspace, root.id, &module_id, position)
        }
    }?;
    Ok(0)
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
    let start = source
        .text()
        .position(location.start, PositionEncoding::Utf8)
        .expect("semantic locations are valid UTF-8 source boundaries");
    let end = source
        .text()
        .position(location.end, PositionEncoding::Utf8)
        .expect("semantic locations are valid UTF-8 source boundaries");
    json!({"line":start.line + 1,"column":start.character,"end_line":end.line + 1,"end_column":end.character})
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

fn query_definitions(
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
                json!({"schema":QUERY_SCHEMA,"module":module_name,"record":"definition","authority":authority(&d.ty.state),"name":d.name,"kind":kind_name(kind),"target":target,"location":location_json(workspace,d.location)}),
            )?;
            continue;
        }
        let ty = d
            .scheme
            .clone()
            .or_else(|| d.ty.value.and_then(|id| workspace.types().display(id)));
        emit(
            json!({"schema":QUERY_SCHEMA,"module":module_name,"record":"definition","authority":authority(&d.ty.state),"name":d.name,"kind":kind_name(kind),"type":ty,"location":location_json(workspace,d.location)}),
        )?;
    }
    Ok(())
}
fn query_exports(
    workspace: &WorkspaceSnapshot,
    module: telora_core::WorkspaceModuleId,
    module_name: &str,
    pattern: Option<&str>,
) -> Result<(), String> {
    let authority = "authoritative";
    let mut exports = workspace.exports_of(module);
    exports.retain(|e| pattern.is_none_or(|p| e.name.contains(p)));
    exports.sort_by(|a, b| a.name.cmp(&b.name));
    for export in exports {
        let ty = export
            .scheme
            .or_else(|| workspace.types().display(export.ty));
        emit(
            json!({"schema":QUERY_SCHEMA,"module":module_name,"record":"export","authority":authority,"name":export.name,"type":ty}),
        )?;
    }
    Ok(())
}
fn query_position(
    workspace: &WorkspaceSnapshot,
    module: telora_core::WorkspaceModuleId,
    module_name: &str,
    at: QueryPosition,
) -> Result<(), String> {
    let source_id = workspace
        .module(module)
        .and_then(|m| m.source)
        .ok_or_else(|| "selected module has no source".to_owned())?;
    let source = workspace.sources().get(source_id);
    let line = u32::try_from(at.line - 1)
        .map_err(|_| format!("line {} is outside {module_name}", at.line))?;
    let (start, end) = if let Some(column) = at.column {
        let column = u32::try_from(column)
            .map_err(|_| format!("position {}:{} is outside {module_name}", at.line, column))?;
        let offset = source
            .text()
            .offset(TextPosition::new(line, column), PositionEncoding::Utf8)
            .map_err(|_| format!("position {}:{} is outside {module_name}", at.line, column))?;
        (offset, offset)
    } else {
        source
            .text()
            .line_content_offsets(line)
            .map_err(|_| format!("line {} is outside {module_name}", at.line))?
    };
    let intersects = |loc: Location| {
        loc.source == source_id
            && if at.column.is_some() {
                loc.start <= start && start < loc.end
            } else {
                loc.start < end && start < loc.end
            }
    };
    for d in workspace
        .definitions()
        .iter()
        .filter(|d| d.module == module && intersects(d.location))
    {
        if let Some(kind) = kind_of(d.kind) {
            emit(
                json!({"schema":QUERY_SCHEMA,"module":module_name,"record":"definition","authority":authority(&d.ty.state),"name":d.name,"kind":kind_name(kind),"type":d.scheme.clone().or_else(||d.ty.value.and_then(|id|workspace.types().display(id))),"location":location_json(workspace,d.location)}),
            )?;
        }
    }
    for r in workspace
        .references()
        .iter()
        .filter(|r| r.module == module && intersects(r.location))
    {
        emit(
            json!({"schema":QUERY_SCHEMA,"module":module_name,"record":"reference","authority":if r.definition.is_some()||r.external{"authoritative"}else{"recovery"},"name":r.name,"resolved":r.definition.is_some(),"external":r.external,"location":location_json(workspace,r.location)}),
        )?;
    }
    for e in workspace
        .expressions()
        .iter()
        .filter(|e| e.module == module && intersects(e.location))
    {
        emit(
            json!({"schema":QUERY_SCHEMA,"module":module_name,"record":"expression","authority":"debug","state":format!("{:?}",e.ty.state),"type":e.ty.value.and_then(|id|workspace.types().display(id)),"location":location_json(workspace,e.location)}),
        )?;
    }
    Ok(())
}
