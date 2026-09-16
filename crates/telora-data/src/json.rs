use crate::source::{Diagnostic, Location, SourceDatabase, SourceId};
use alloc::collections::BTreeMap;
use alloc::{string::String, vec::Vec};
use core::fmt;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ValuePathSegment {
    Index(usize),
    Key(String),
}

pub type ValuePath = Vec<ValuePathSegment>;

#[derive(Clone, Debug)]
pub enum DataScalar {
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Null,
    Bool(bool),
    Temporal { kind: TemporalKind, value: String },
}

#[derive(Clone, Copy, Debug)]
pub enum TemporalKind {
    LocalDate,
    LocalTime,
    LocalDateTime,
    OffsetDateTime,
}

impl TemporalKind {
    pub fn variant(self) -> &'static str {
        match self {
            Self::LocalDate => "LocalDate",
            Self::LocalTime => "LocalTime",
            Self::LocalDateTime => "LocalDateTime",
            Self::OffsetDateTime => "OffsetDateTime",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DataNodeId(usize);
impl DataNodeId {
    pub fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Debug)]
pub struct DataField {
    pub key_location: Location,
    pub value: DataNodeId,
}

#[derive(Clone, Debug)]
pub enum DataPlanNodeKind {
    Scalar(DataScalar),
    Array(Vec<DataNodeId>),
    Object(BTreeMap<String, DataField>),
}

#[derive(Clone, Debug)]
pub struct DataPlanNode {
    pub kind: DataPlanNodeKind,
    pub location: Location,
}

/// A validated, flat arena over source-backed nodes. Edges are node ids, so
/// parsers never construct a second recursive data tree before Heap allocation.
#[derive(Clone, Debug, Default)]
pub struct ValidatedDataPlan {
    pub(crate) postordered: bool,
    nodes: Vec<DataPlanNode>,
    root: Option<DataNodeId>,
    pub(crate) source_index: Option<(crate::source::SourceId, alloc::sync::Arc<crate::source::LineIndex>)>,
}

impl ValidatedDataPlan {
    pub fn compact(&self, loc: Location) -> crate::source::CompactLoc {
        let (source, lines) = self.source_index.as_ref().expect("registered data source");
        assert_eq!(*source, loc.source);
        lines.pack(loc)
    }
    /// Move reachable nodes into child-before-parent order, preserving shared
    /// aliases and all source locations. Payloads are moved, never deep-copied.
    pub fn into_postorder(self) -> Self {
        if self.postordered { return self; }
        fn visit(
            id: DataNodeId,
            pending: &mut [Option<DataPlanNode>],
            mapped: &mut [Option<DataNodeId>],
            output: &mut Vec<DataPlanNode>,
        ) -> DataNodeId {
            if let Some(id) = mapped[id.index()] {
                return id;
            }
            let mut node = pending[id.index()]
                .take()
                .expect("validated data is acyclic");
            match &mut node.kind {
                DataPlanNodeKind::Array(items) => {
                    for child in items {
                        *child = visit(*child, pending, mapped, output);
                    }
                }
                DataPlanNodeKind::Object(fields) => {
                    for field in fields.values_mut() {
                        field.value = visit(field.value, pending, mapped, output);
                    }
                }
                DataPlanNodeKind::Scalar(_) => {}
            }
            let next = DataNodeId(output.len());
            output.push(node);
            mapped[id.index()] = Some(next);
            next
        }
        let Some(root) = self.root else {
            return self;
        };
        let mut mapped = vec![None; self.nodes.len()];
        let mut pending = self.nodes.into_iter().map(Some).collect::<Vec<_>>();
        let mut nodes = Vec::with_capacity(pending.len());
        let root = visit(root, &mut pending, &mut mapped, &mut nodes);
        Self {
            postordered: true,
            nodes,
            root: Some(root),
            source_index: self.source_index,
        }
    }

    pub fn into_nodes(self) -> Vec<DataPlanNode> {
        self.nodes
    }

    pub fn root_node(&self) -> Option<DataNodeId> {
        self.root
    }
    pub fn nodes(&self) -> &[DataPlanNode] {
        &self.nodes
    }
    pub(crate) fn scalar(&mut self, value: DataScalar, location: Location) -> DataNodeId {
        self.push(DataPlanNodeKind::Scalar(value), location)
    }

    pub(crate) fn array(&mut self, values: Vec<DataNodeId>, location: Location) -> DataNodeId {
        self.push(DataPlanNodeKind::Array(values), location)
    }

    pub(crate) fn object(
        &mut self,
        fields: BTreeMap<String, DataField>,
        location: Location,
    ) -> DataNodeId {
        self.push(DataPlanNodeKind::Object(fields), location)
    }

    pub(crate) fn set_root(&mut self, root: DataNodeId) {
        self.root = Some(root);
    }

    pub(crate) fn root(&self) -> DataNodeId {
        self.root.expect("validated data plan has a root")
    }

    pub(crate) fn node(&self, id: DataNodeId) -> &DataPlanNode {
        &self.nodes[id.0]
    }

    pub(crate) fn node_mut(&mut self, id: DataNodeId) -> &mut DataPlanNode {
        &mut self.nodes[id.0]
    }

    fn push(&mut self, kind: DataPlanNodeKind, location: Location) -> DataNodeId {
        let id = DataNodeId(self.nodes.len());
        self.nodes.push(DataPlanNode { kind, location });
        id
    }

    pub(crate) fn enforce_limits(
        &self,
        limits: crate::DataLimits,
        file_size: usize,
    ) -> Result<DataStats, DataLimitError> {
        if file_size > limits.file_size {
            return Err(DataLimitError::new(
                "file_size",
                file_size,
                limits.file_size,
            ));
        }

        fn add(
            value: &mut usize,
            amount: usize,
            name: &'static str,
            limit: usize,
        ) -> Result<(), DataLimitError> {
            *value = value
                .checked_add(amount)
                .ok_or_else(|| DataLimitError::overflow(name, limit))?;
            if *value > limit {
                return Err(DataLimitError::new(name, *value, limit));
            }
            Ok(())
        }

        fn visit(
            plan: &ValidatedDataPlan,
            id: DataNodeId,
            depth: usize,
            limits: crate::DataLimits,
            stats: &mut DataStats,
        ) -> Result<(), DataLimitError> {
            if depth > limits.depth {
                return Err(DataLimitError::new("depth", depth, limits.depth));
            }
            stats.depth = stats.depth.max(depth);
            add(&mut stats.nodes, 1, "nodes", limits.nodes)?;
            match &plan.node(id).kind {
                DataPlanNodeKind::Scalar(DataScalar::String(value)) => {
                    stats.string_len = stats.string_len.max(value.len());
                    if value.len() > limits.string_len {
                        return Err(DataLimitError::new(
                            "string_len",
                            value.len(),
                            limits.string_len,
                        ));
                    }
                    add(
                        &mut stats.payloads_bytes,
                        value.len(),
                        "payloads_bytes",
                        limits.payloads_bytes,
                    )?;
                }
                DataPlanNodeKind::Scalar(DataScalar::Bytes(value)) => {
                    stats.bytes_len = stats.bytes_len.max(value.len());
                    if value.len() > limits.bytes_len {
                        return Err(DataLimitError::new(
                            "bytes_len",
                            value.len(),
                            limits.bytes_len,
                        ));
                    }
                    add(
                        &mut stats.payloads_bytes,
                        value.len(),
                        "payloads_bytes",
                        limits.payloads_bytes,
                    )?;
                }
                DataPlanNodeKind::Scalar(DataScalar::Temporal { value, .. }) => {
                    stats.string_len = stats.string_len.max(value.len());
                    if value.len() > limits.string_len {
                        return Err(DataLimitError::new(
                            "string_len",
                            value.len(),
                            limits.string_len,
                        ));
                    }
                    add(
                        &mut stats.payloads_bytes,
                        value.len(),
                        "payloads_bytes",
                        limits.payloads_bytes,
                    )?;
                }
                DataPlanNodeKind::Scalar(_) => {}
                DataPlanNodeKind::Array(items) => {
                    stats.container_size = stats.container_size.max(items.len());
                    if items.len() > limits.container_size {
                        return Err(DataLimitError::new(
                            "container_size",
                            items.len(),
                            limits.container_size,
                        ));
                    }
                    let child_depth = depth
                        .checked_add(1)
                        .ok_or_else(|| DataLimitError::overflow("depth", limits.depth))?;
                    for item in items {
                        visit(plan, *item, child_depth, limits, stats)?;
                    }
                }
                DataPlanNodeKind::Object(fields) => {
                    stats.container_size = stats.container_size.max(fields.len());
                    if fields.len() > limits.container_size {
                        return Err(DataLimitError::new(
                            "container_size",
                            fields.len(),
                            limits.container_size,
                        ));
                    }
                    let child_depth = depth
                        .checked_add(1)
                        .ok_or_else(|| DataLimitError::overflow("depth", limits.depth))?;
                    for (name, field) in fields {
                        stats.string_len = stats.string_len.max(name.len());
                        if name.len() > limits.string_len {
                            return Err(DataLimitError::new(
                                "string_len",
                                name.len(),
                                limits.string_len,
                            ));
                        }
                        add(
                            &mut stats.payloads_bytes,
                            name.len(),
                            "payloads_bytes",
                            limits.payloads_bytes,
                        )?;
                        visit(plan, field.value, child_depth, limits, stats)?;
                    }
                }
            }
            Ok(())
        }

        let mut stats = DataStats {
            file_size,
            ..DataStats::default()
        };
        visit(self, self.root(), 1, limits, &mut stats)?;
        Ok(stats)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DataStats {
    pub(crate) file_size: usize,
    pub(crate) nodes: usize,
    pub(crate) depth: usize,
    pub(crate) container_size: usize,
    pub(crate) bytes_len: usize,
    pub(crate) string_len: usize,
    pub(crate) payloads_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DataLimitError {
    name: &'static str,
    actual: Option<usize>,
    limit: usize,
}

impl DataLimitError {
    fn new(name: &'static str, actual: usize, limit: usize) -> Self {
        Self {
            name,
            actual: Some(actual),
            limit,
        }
    }

    fn overflow(name: &'static str, limit: usize) -> Self {
        Self {
            name,
            actual: None,
            limit,
        }
    }
}

impl fmt::Display for DataLimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.actual {
            Some(actual) => write!(
                formatter,
                "data source exceeds {name} limit ({actual} > {limit})",
                name = self.name,
                limit = self.limit,
            ),
            None => write!(
                formatter,
                "data source {name} accounting overflowed (limit {limit})",
                name = self.name,
                limit = self.limit,
            ),
        }
    }
}

mod lexer;
mod parse;

#[cfg(test)]
pub(crate) fn validate_json_registered(
    sources: &SourceDatabase,
    source: SourceId,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    parse_with_limits(sources, source, crate::DataLimits::default())
}

pub(crate) fn parse_with_limits(
    sources: &SourceDatabase,
    source: SourceId,
    limits: crate::DataLimits,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let text = sources.get(source).text();
    parse::Parser::new(source, text.chunks(), limits).parse(text.byte_len())
        .map_err(|diagnostic| vec![diagnostic])
}

#[cfg(test)]
mod tests;
