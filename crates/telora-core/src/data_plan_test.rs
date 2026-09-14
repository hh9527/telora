//! Test-only readable projection of the pure parser arena.
use crate::data_plan::{DataNodeId, DataPlanNodeKind, DataScalar, ValidatedDataPlan};

pub(crate) fn render(plan: &ValidatedDataPlan) -> String {
    fn node(plan: &ValidatedDataPlan, id: DataNodeId) -> String {
        match &plan.nodes()[id.index()].kind {
            DataPlanNodeKind::Scalar(value) => match value {
                DataScalar::Int(n) => n.to_string(),
                DataScalar::Float(n) => n.to_string(),
                DataScalar::String(s) => serde_json::to_string(s).unwrap(),
                DataScalar::Bytes(bytes) => format!(
                    "b\"{}\"",
                    bytes
                        .iter()
                        .map(|b| format!("\\x{b:02x}"))
                        .collect::<String>()
                ),
                DataScalar::Null => "'None".into(),
                DataScalar::Bool(b) => if *b { "'True" } else { "'False" }.into(),
                DataScalar::Temporal { kind, value } => format!(
                    "'{}({})",
                    kind.variant(),
                    serde_json::to_string(value).unwrap()
                ),
            },
            DataPlanNodeKind::Array(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(|id| node(plan, *id))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            DataPlanNodeKind::Object(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(key, field)| format!("{key}: {}", node(plan, field.value)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    node(plan, plan.root_node().unwrap())
}
