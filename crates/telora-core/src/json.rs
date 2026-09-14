use crate::source::{Diagnostic, Location, SourceDatabase, SourceId};
use crate::syntax::json::lexer::Token;
use crate::syntax::json::parser::{CstData, Node, NodeRef, Rule};
use std::collections::BTreeMap;
use std::fmt;

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
pub enum TemporalKind { LocalDate, LocalTime, LocalDateTime, OffsetDateTime }

impl TemporalKind {
    pub fn variant(self) -> &'static str {
        match self {
            Self::LocalDate => "LocalDate", Self::LocalTime => "LocalTime",
            Self::LocalDateTime => "LocalDateTime", Self::OffsetDateTime => "OffsetDateTime",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DataNodeId(usize);
impl DataNodeId { pub fn index(self) -> usize { self.0 } }

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
    nodes: Vec<DataPlanNode>,
    root: Option<DataNodeId>,
}

impl ValidatedDataPlan {
    pub fn root_node(&self) -> Option<DataNodeId> { self.root }
    pub fn nodes(&self) -> &[DataPlanNode] { &self.nodes }
    /// Runtime string parsing attributes values to the input expression, not
    /// to temporary parser SourceIds which are outside the session database.
    pub(crate) fn set_location(&mut self, location: Location) {
        for node in &mut self.nodes {
            node.location = location;
            if let DataPlanNodeKind::Object(fields) = &mut node.kind {
                for field in fields.values_mut() { field.key_location = location; }
            }
        }
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

    pub(crate) fn clone_root_at(&mut self, id: DataNodeId, location: Location) -> DataNodeId {
        let kind = self.node(id).kind.clone();
        self.push(kind, location)
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

pub(crate) fn validate_json_registered(
    sources: &SourceDatabase,
    source_id: SourceId,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let source = sources.get(source_id);
    let parsed = crate::syntax::json::parse_document(source_id, source.text());
    if !parsed.diagnostics.is_empty() {
        return Err(parsed.diagnostics);
    }
    JsonLowerer::new(source_id, source.text(), &parsed.syntax)
        .validated_plan()
        .map_err(|diagnostic| vec![diagnostic])
}

struct JsonLowerer<'a> {
    source_id: SourceId,
    source: &'a crate::document::DocumentText,
    cst: &'a CstData,
}

impl<'a> JsonLowerer<'a> {
    fn new(
        source_id: SourceId,
        source: &'a crate::document::DocumentText,
        cst: &'a CstData,
    ) -> Self {
        Self {
            source_id,
            source,
            cst,
        }
    }

    fn validated_plan(&self) -> Result<ValidatedDataPlan, Diagnostic> {
        let value = self
            .children(NodeRef::ROOT)
            .find(|node| self.is_value(*node))
            .ok_or_else(|| self.error(NodeRef::ROOT, "expected a JSON value"))?;
        let mut plan = ValidatedDataPlan::default();
        let root = self.plan_value(value, &mut plan)?;
        plan.set_root(root);
        Ok(plan)
    }

    fn plan_value(
        &self,
        node: NodeRef,
        plan: &mut ValidatedDataPlan,
    ) -> Result<DataNodeId, Diagnostic> {
        let location = self.location(node);
        match self.cst.get(node) {
            Node::Token(Token::Null, _) => {
                Ok(plan.scalar(DataScalar::Null, location))
            }
            Node::Token(Token::True, _) => {
                Ok(plan.scalar(DataScalar::Bool(true), location))
            }
            Node::Token(Token::False, _) => {
                Ok(plan.scalar(DataScalar::Bool(false), location))
            }
            Node::Token(Token::Number, _) => {
                let value = self.number(node)?;
                Ok(plan.scalar(value, location))
            }
            Node::Rule(Rule::StringLiteral, _) => {
                Ok(plan.scalar(DataScalar::String(self.decode_string(node)?), location))
            }
            Node::Rule(Rule::Literal | Rule::Value, _) => {
                let child = self
                    .children(node)
                    .find(|child| self.is_value(*child))
                    .ok_or_else(|| self.error(node, "empty JSON value"))?;
                self.plan_value(child, plan)
            }
            Node::Rule(Rule::Array, _) => {
                let mut values = Vec::new();
                for child in self.children(node).filter(|child| self.is_value(*child)) {
                    values.push(self.plan_value(child, plan)?);
                }
                Ok(plan.array(values, location))
            }
            Node::Rule(Rule::Object, _) => {
                let mut fields: BTreeMap<String, DataField> = BTreeMap::new();
                for member in self
                    .rule_children(node)
                    .filter(|child| self.rule(*child) == Some(Rule::Member))
                {
                    let key_node = self
                        .rule_children(member)
                        .find(|child| self.rule(*child) == Some(Rule::StringLiteral))
                        .ok_or_else(|| self.error(member, "JSON object key must be a string"))?;
                    let key = self.decode_string(key_node)?;
                    let key_location = self.location(key_node);
                    if let Some(previous) = fields.get(&key) {
                        return Err(Diagnostic::error(
                            format!("duplicate JSON object key {key:?}"),
                            key_location,
                        )
                        .with_secondary("first defined here", previous.key_location));
                    }
                    let value = self
                        .children(member)
                        .find(|child| *child != key_node && self.is_value(*child))
                        .ok_or_else(|| self.error(member, "JSON member has no value"))?;
                    let value = self.plan_value(value, plan)?;
                    fields.insert(
                        key,
                        DataField {
                            key_location,
                            value,
                        },
                    );
                }
                Ok(plan.object(fields, location))
            }
            _ => Err(self.error(node, "expected a JSON value")),
        }
    }

    fn number(&self, node: NodeRef) -> Result<DataScalar, Diagnostic> {
        let text = self.text(node);
        if text.contains(['.', 'e', 'E']) {
            let value = text
                .parse::<f64>()
                .map_err(|_| self.error(node, "invalid Float value"))?;
            if !value.is_finite() {
                return Err(self.error(node, "JSON Float must be finite"));
            }
            Ok(DataScalar::Float(value))
        } else {
            text.parse::<i64>()
                .map(DataScalar::Int)
                .map_err(|_| self.error(node, "JSON integer is outside the i64 range"))
        }
    }

    fn decode_string(&self, node: NodeRef) -> Result<String, Diagnostic> {
        let text = self.text(node);
        let mut decoder = StringDecoder {
            bytes: text.as_bytes(),
            offset: 1,
        };
        let mut output = String::new();
        while decoder.offset < decoder.bytes.len() - 1 {
            let byte = decoder.bytes[decoder.offset];
            if byte != b'\\' {
                let character = text[decoder.offset..].chars().next().expect("valid UTF-8");
                output.push(character);
                decoder.offset += character.len_utf8();
                continue;
            }
            decoder.offset += 1;
            let escaped = *decoder
                .bytes
                .get(decoder.offset)
                .ok_or_else(|| self.error(node, "unterminated JSON escape"))?;
            decoder.offset += 1;
            match escaped {
                b'"' => output.push('"'),
                b'\\' => output.push('\\'),
                b'/' => output.push('/'),
                b'b' => output.push('\u{0008}'),
                b'f' => output.push('\u{000c}'),
                b'n' => output.push('\n'),
                b'r' => output.push('\r'),
                b't' => output.push('\t'),
                b'u' => output.push(
                    decoder
                        .unicode_escape()
                        .map_err(|message| self.error(node, message))?,
                ),
                other => {
                    return Err(
                        self.error(node, format!("invalid JSON escape \\{}", char::from(other)))
                    );
                }
            }
        }
        Ok(output)
    }

    fn is_value(&self, node: NodeRef) -> bool {
        matches!(
            self.cst.get(node),
            Node::Token(Token::Number | Token::True | Token::False | Token::Null, _)
        ) || matches!(
            self.rule(node),
            Some(Rule::Value | Rule::Literal | Rule::Array | Rule::Object | Rule::StringLiteral)
        )
    }
    fn children(&self, node: NodeRef) -> impl Iterator<Item = NodeRef> + '_ {
        self.cst.children(node)
    }
    fn rule_children(&self, node: NodeRef) -> impl Iterator<Item = NodeRef> + '_ {
        self.children(node)
            .filter(|child| matches!(self.cst.get(*child), Node::Rule(..)))
    }
    fn rule(&self, node: NodeRef) -> Option<Rule> {
        match self.cst.get(node) {
            Node::Rule(rule, _) => Some(rule),
            Node::Token(..) => None,
        }
    }
    fn text(&self, node: NodeRef) -> std::borrow::Cow<'_, str> {
        self.source
            .slice(
                crate::source::TextRange::from_usize(self.cst.span(node))
                    .expect("CST span fits registered source"),
            )
            .expect("CST span is a valid source slice")
    }
    fn location(&self, node: NodeRef) -> Location {
        Location::from_usize(self.source_id, self.cst.span(node))
            .expect("CST span fits registered source")
    }
    fn error(&self, node: NodeRef, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.location(node))
    }
}

struct StringDecoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl StringDecoder<'_> {
    fn unicode_escape(&mut self) -> Result<char, &'static str> {
        let first = self.hex_quad()?;
        let codepoint = if (0xd800..=0xdbff).contains(&first) {
            if self.bytes.get(self.offset..self.offset + 2) != Some(b"\\u") {
                return Err("high surrogate requires a low surrogate");
            }
            self.offset += 2;
            let second = self.hex_quad()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err("invalid low surrogate");
            }
            0x10000 + (((first - 0xd800) as u32) << 10) + (second - 0xdc00) as u32
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Err("unexpected low surrogate");
        } else {
            first as u32
        };
        char::from_u32(codepoint).ok_or("invalid Unicode scalar value")
    }

    fn hex_quad(&mut self) -> Result<u16, &'static str> {
        let mut value = 0u16;
        for _ in 0..4 {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or("Unicode escape requires four hex digits")?;
            self.offset += 1;
            let digit = char::from(byte)
                .to_digit(16)
                .ok_or("Unicode escape requires four hex digits")?;
            value = value * 16 + digit as u16;
        }
        Ok(value)
    }
}

#[cfg(test)]
#[path = "json/tests/mod.rs"]
mod tests;
