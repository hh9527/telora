//! Pure, source-backed data parsing shared by execution backends. No VM or
//! heap is created by this interface; callers materialize the validated plan.
pub use crate::json::{
    DataField, DataNodeId, DataPlanNode, DataPlanNodeKind, DataScalar, ValidatedDataPlan,
};
use crate::source::{Diagnostic, SourceDatabase, SourceId};

#[derive(Clone, Copy, Debug)]
pub enum Format {
    Json,
    Yaml,
    Toml,
}

pub fn parse_registered(
    sources: &SourceDatabase,
    source: SourceId,
    format: Format,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    match format {
        Format::Json => crate::json::validate_json_registered(sources, source),
        Format::Yaml => crate::yaml::validate_yaml_registered(sources, source),
        Format::Toml => crate::toml::validate_toml_registered(sources, source),
    }
}
