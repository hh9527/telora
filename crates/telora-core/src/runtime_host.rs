//! Host inputs and effects shared by VM entry execution and its callers.
//! These contracts do not depend on module loading or type inference.

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

#[derive(Clone, Debug)]
pub struct EvalSource {
    pub source_name: String,
    pub format: SystemDataFormat,
    pub text: String,
}
