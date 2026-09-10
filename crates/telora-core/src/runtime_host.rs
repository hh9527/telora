//! Host inputs and effects shared by VM entry execution and its callers.
//! These contracts do not depend on module loading or type inference.
use std::collections::BTreeMap;

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
    pub ees_models: Vec<SystemEesModel>,
    pub ees_vars: BTreeMap<String, String>,
    pub text_sources: BTreeMap<String, SystemTextSource>,
    pub vars: Vec<String>,
    pub stdin: SystemStdin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemEesModel {
    pub kind: String,
    pub name: String,
    pub config: serde_json::Value,
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

    fn ees_call(&mut self, call: EesCall) -> RunHostFuture<'_, Result<(), String>>;

    fn next_event(&mut self) -> RunHostFuture<'_, Result<Option<SystemEvent>, String>>;

    fn finish(&mut self) -> RunHostFuture<'_, Result<(), String>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunTermination {
    Exit(i64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunOutcome {
    pub output: String,
    pub termination: RunTermination,
}

#[derive(Clone, Debug, Default)]
pub struct EvalContext {
    pub sources: BTreeMap<String, EvalSource>,
    pub env: BTreeMap<String, String>,
    pub args: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct EvalSource {
    pub source_name: String,
    pub format: SystemDataFormat,
    pub text: String,
}
