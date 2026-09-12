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

/// Apply the shared logical data limits before any execution backend allocates
/// objects. Alias expansion is counted by the validated plan's traversal.
pub fn enforce_limits(
    plan: &ValidatedDataPlan,
    limits: crate::DataLimits,
    file_size: usize,
) -> Result<(), String> {
    plan.enforce_limits(limits, file_size)
        .map(|_| ())
        .map_err(|error| error.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_data_limits_count_expanded_aliases_and_decoded_payloads() {
        let mut sources = SourceDatabase::default();
        let text = "base: &base [1, 2]\ncopy: *base\n";
        let source = sources.add("limits.yaml", text);
        let plan = parse_registered(&sources, source, Format::Yaml).unwrap();
        let mut limits = crate::DataLimits::default();
        limits.nodes = 6;
        assert!(enforce_limits(&plan, limits, text.len()).unwrap_err().contains("nodes"));
        limits.nodes = 7;
        enforce_limits(&plan, limits, text.len()).unwrap();
        limits.depth = 2;
        assert!(enforce_limits(&plan, limits, text.len()).unwrap_err().contains("depth"));
        let text = r#"["\u4e2d"]"#;
        let source = sources.add("limits.json", text);
        let plan = parse_registered(&sources, source, Format::Json).unwrap();
        limits = crate::DataLimits::default();
        limits.string_len = 2;
        assert!(enforce_limits(&plan, limits, text.len()).unwrap_err().contains("string_len"));
        limits.string_len = 3;
        enforce_limits(&plan, limits, text.len()).unwrap();
        limits.payloads_bytes = 2;
        assert!(enforce_limits(&plan, limits, text.len()).unwrap_err().contains("payloads_bytes"));
    }
}
