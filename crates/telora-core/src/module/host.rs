#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    pub module_quota: Quota,
    pub session_quota: Quota,
    pub data_limits: DataLimits,
}

/// Admission limits applied independently to each static or Entry data source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataLimits {
    /// Maximum raw source bytes read before parsing.
    pub file_size: usize,
    /// Maximum logical Value occurrences after alias and merge expansion.
    pub nodes: usize,
    /// Maximum logical graph depth, with the root at depth one.
    pub depth: usize,
    /// Maximum element or field count of any one Array or Object.
    pub container_size: usize,
    /// Maximum decoded byte length of any one Bytes value.
    pub bytes_len: usize,
    /// Maximum decoded UTF-8 byte length of any String, object key, or temporal value.
    pub string_len: usize,
    /// Maximum total decoded bytes in Strings, object keys, temporal values, and Bytes.
    pub payloads_bytes: usize,
}

impl Default for DataLimits {
    fn default() -> Self {
        Self {
            file_size: 256 * 1024 * 1024,
            nodes: 1_000_000,
            depth: 256,
            container_size: 1_000_000,
            bytes_len: 64 * 1024 * 1024,
            string_len: 64 * 1024 * 1024,
            payloads_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildStdinMode {
    Piped,
    Inherit,
    Null,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildOutputMode {
    PipedLine,
    PipedToEnd,
    Inherit,
    Null,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildOptions {
    pub bin: String,
    pub cwd: Option<String>,
    pub envs: BTreeMap<String, Option<String>>,
    pub clear_env: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChildStdio {
    pub stdin: ChildStdinMode,
    pub stdout: ChildOutputMode,
    pub stderr: ChildOutputMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnStdioChild {
    pub key: String,
    pub opts: ChildOptions,
    pub stdio: ChildStdio,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildText {
    pub key: String,
    pub data: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildSpawnResult {
    pub key: String,
    pub result: Result<i64, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChildExit {
    Code(i64),
    Signal(Option<i64>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemDataFormat {
    Json,
    Yaml,
    Toml,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemDataSource {
    pub src: String,
    pub format: SystemDataFormat,
    pub has_default: bool,
}

pub type EntryDataSources = BTreeMap<String, SystemDataSource>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemTextSource {
    pub src: String,
    pub default: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemStdin {
    Text,
    Lined,
    Null,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemCaps {
    pub data_sources: BTreeMap<String, SystemDataSource>,
    pub ees: BTreeMap<String, String>,
    pub spawn_child: bool,
    pub text_sources: BTreeMap<String, SystemTextSource>,
    pub vars: Vec<String>,
    pub stdin: SystemStdin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EesCall {
    pub key: String,
    pub actor: String,
    pub operation: String,
    pub input: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EesReply {
    pub key: String,
    pub result: Result<serde_json::Value, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemEvent {
    EesReply(EesReply),
    StdinLine(Option<String>),
    ChildStdout(ChildText),
    ChildStderr(ChildText),
    ChildSpawnResult(ChildSpawnResult),
    ChildExited { key: String, exited: ChildExit },
}

pub type RunHostFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;

pub trait RunHost {
    fn resources_provider(&mut self) -> crate::NativeFunction;

    fn ees_actors(&self) -> BTreeMap<String, String>;

    fn configure(&mut self, caps: SystemCaps) -> RunHostFuture<'_, Result<(), String>>;

    /// Reads a configured data source without decoding it into a Telora value.
    /// The runtime registers and materializes the returned source directly in
    /// the Entry WorkWorld.
    fn read_data_source(
        &mut self,
        source: &SystemDataSource,
        max_bytes: usize,
    ) -> RunHostFuture<'_, Result<Option<String>, String>>;

    fn spawn_stdio_child(
        &mut self,
        child: SpawnStdioChild,
    ) -> RunHostFuture<'_, Result<(), String>>;

    fn post_stdin(&mut self, text: ChildText) -> RunHostFuture<'_, Result<(), String>>;

    fn ees_call(&mut self, call: EesCall) -> RunHostFuture<'_, Result<(), String>>;

    fn next_event(&mut self) -> RunHostFuture<'_, Result<Option<SystemEvent>, String>>;

    fn finish(&mut self) -> RunHostFuture<'_, Result<(), String>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunTermination {
    Exit(i64),
    Exec(ChildOptions),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunOutcome {
    pub output: String,
    pub termination: RunTermination,
}

struct NoProcessRunHost;

fn empty_system_resources(
    context: &mut crate::CallContext<'_, '_>,
) -> Result<(), crate::NativeError> {
    let data = context.scratch()?;
    let texts = context.scratch()?;
    let vars = context.scratch()?;
    let stdin = context.scratch()?;
    context.make_dict(data, &[])?;
    context.make_dict(texts, &[])?;
    context.make_dict(vars, &[])?;
    context.set_none(stdin)?;
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

impl RunHost for NoProcessRunHost {
    fn resources_provider(&mut self) -> crate::NativeFunction {
        crate::NativeFunction::new(
            "host.prepare_system_resources.empty",
            3,
            empty_system_resources,
        )
    }

    fn ees_actors(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn configure(&mut self, caps: SystemCaps) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if !caps.data_sources.is_empty()
                || !caps.ees.is_empty()
                || !caps.text_sources.is_empty()
                || !caps.vars.is_empty()
                || caps.stdin != SystemStdin::Null
                || caps.spawn_child
            {
                return Err("this Host does not provide initialization capabilities".into());
            }
            Ok(())
        })
    }

    fn read_data_source(
        &mut self,
        _source: &SystemDataSource,
        _max_bytes: usize,
    ) -> RunHostFuture<'_, Result<Option<String>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn spawn_stdio_child(
        &mut self,
        _child: SpawnStdioChild,
    ) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async { Err("this Host does not provide stdio child processes".into()) })
    }

    fn post_stdin(&mut self, _text: ChildText) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async { Err("this Host does not provide stdio child processes".into()) })
    }

    fn ees_call(&mut self, _call: EesCall) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async { Err("this Host does not provide EES actors".into()) })
    }

    fn next_event(&mut self) -> RunHostFuture<'_, Result<Option<SystemEvent>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn finish(&mut self) -> RunHostFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}
