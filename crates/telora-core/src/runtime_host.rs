//! Host inputs and effects shared by VM entry execution and its callers.
//! These contracts do not depend on module loading or type inference.
use std::collections::BTreeMap;

pub use telora_data::DataLimits;

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

#[derive(Clone, Debug)]
pub struct EvalSource {
    pub source_name: String,
    pub format: SystemDataFormat,
    pub text: String,
}
