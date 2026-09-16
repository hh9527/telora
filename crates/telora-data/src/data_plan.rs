//! Pure, source-backed data parsing shared by execution backends. No VM or
//! heap is created by this interface; callers materialize the validated plan.
pub use crate::json::{
    DataField, DataNodeId, DataPlanNode, DataPlanNodeKind, DataScalar, TemporalKind,
    ValidatedDataPlan,
};
use crate::source::{Diagnostic, SourceDatabase, SourceId};
use alloc::{string::String, string::ToString, vec::Vec};

#[derive(Clone, Copy, Debug)]
pub enum Format {
    Json,
    Yaml,
    Toml,
}

/// Inspect a plan's data limits before any execution backend allocates objects.
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
    parse_registered_with_limits(sources, source, format, crate::DataLimits::default())
}

/// All formats admit resources during construction, before allocating payloads.
pub fn parse_registered_with_limits(
    sources: &SourceDatabase,
    source: SourceId,
    format: Format,
    limits: crate::DataLimits,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let mut plan = match format {
        Format::Json => crate::json::parse_with_limits(sources, source, limits),
        Format::Yaml => crate::yaml::parse_with_limits(sources, source, limits),
        Format::Toml => crate::toml::parse_with_limits(sources, source, limits),
    }?;
    plan.source_index = Some((source, sources.get(source).line_index().clone()));
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postorder_moves_payloads_and_closes_child_ids() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("values.toml", "base = ['hello']\ncopy = ['hello']\n");
        // Build parent before children so this exercises the actual reorder,
        // rather than a parser's already-postordered fast path.
        let mut plan = ValidatedDataPlan::default();
        let loc = crate::source::Location::from_usize(source, 0..32).unwrap();
        let root = plan.object(Default::default(), loc);
        let value = plan.scalar(DataScalar::String("hello".into()), loc);
        let base = plan.array(vec![value], loc);
        let copy = plan.array(vec![value], loc);
        let DataPlanNodeKind::Object(fields) = &mut plan.node_mut(root).kind else { panic!("root") };
        fields.insert("base".into(), DataField { key_location: loc, value: base });
        fields.insert("copy".into(), DataField { key_location: loc, value: copy });
        plan.set_root(root);
        let (pointer, location) = plan
            .nodes()
            .iter()
            .find_map(|node| match &node.kind {
                DataPlanNodeKind::Scalar(DataScalar::String(value)) => {
                    Some((value.as_ptr(), node.location))
                }
                _ => None,
            })
            .unwrap();
        let plan = plan.into_postorder();
        for (index, node) in plan.nodes().iter().enumerate() {
            match &node.kind {
                DataPlanNodeKind::Array(items) => {
                    assert!(items.iter().all(|id| id.index() < index))
                }
                DataPlanNodeKind::Object(fields) => {
                    assert!(fields.values().all(|field| field.value.index() < index))
                }
                DataPlanNodeKind::Scalar(DataScalar::String(value)) => {
                    if node.location == location { assert_eq!(value.as_ptr(), pointer); }
                }
                _ => {}
            }
        }
        let DataPlanNodeKind::Object(fields) =
            &plan.nodes()[plan.root_node().unwrap().index()].kind
        else {
            panic!("root");
        };
        let DataPlanNodeKind::Array(base) = &plan.nodes()[fields["base"].value.index()].kind else {
            panic!("base");
        };
        let DataPlanNodeKind::Array(copy) = &plan.nodes()[fields["copy"].value.index()].kind else {
            panic!("copy");
        };
        assert_eq!(base.len(), copy.len());
    }

    #[test]
    fn backend_data_limits_count_nodes_and_decoded_payloads() {
        let mut sources = SourceDatabase::default();
        let text = "base: [1, 2]\ncopy: [1, 2]\n";
        let source = sources.add("limits.yaml", text);
        let plan = parse_registered(&sources, source, Format::Yaml).unwrap();
        let mut limits = crate::DataLimits::default();
        limits.nodes = 6;
        assert!(
            enforce_limits(&plan, limits, text.len())
                .unwrap_err()
                .contains("nodes")
        );
        limits.nodes = 7;
        enforce_limits(&plan, limits, text.len()).unwrap();
        limits.depth = 2;
        assert!(
            enforce_limits(&plan, limits, text.len())
                .unwrap_err()
                .contains("depth")
        );
        let text = r#"["\u4e2d"]"#;
        let source = sources.add("limits.json", text);
        let plan = parse_registered(&sources, source, Format::Json).unwrap();
        limits = crate::DataLimits::default();
        limits.string_len = 2;
        assert!(
            enforce_limits(&plan, limits, text.len())
                .unwrap_err()
                .contains("string_len")
        );
        limits.string_len = 3;
        enforce_limits(&plan, limits, text.len()).unwrap();
        limits.payloads_bytes = 2;
        assert!(
            enforce_limits(&plan, limits, text.len())
                .unwrap_err()
                .contains("payloads_bytes")
        );
    }
}
