use crate::DataWorld;
use crate::heap::Heap;
use crate::json::{
    DataField, DataNodeId, DataPlanNodeKind, DataScalar, SourcedValue, ValidatedDataPlan,
    materialize_data_plan,
};
use crate::source::{Diagnostic, Location, SourceDatabase, SourceId};
use crate::syntax::toml::lexer::Token;
use crate::syntax::toml::parser::{CstData, Node, NodeRef, Rule};
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct TomlParse {
    pub cst: CstData,
    pub value: Option<SourcedValue>,
    pub diagnostics: Vec<Diagnostic>,
}

pub(crate) fn validate_toml_registered(
    sources: &SourceDatabase,
    source_id: SourceId,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let source = sources.get(source_id);
    let parsed = crate::syntax::toml::parse_document(source_id, source.text());
    if !parsed.diagnostics.is_empty() {
        return Err(parsed.diagnostics);
    }
    TomlLowerer::new(source_id, source.text(), &parsed.syntax)
        .validated_plan()
        .map_err(|diagnostic| vec![diagnostic])
}

pub fn parse_toml_registered(sources: &SourceDatabase, source_id: SourceId) -> TomlParse {
    let source = sources.get(source_id);
    let parsed = crate::syntax::toml::parse_document(source_id, source.text());
    let mut diagnostics = parsed.diagnostics;
    let value = if diagnostics.is_empty() {
        let mut heap = Heap::work();
        match TomlLowerer::new(source_id, source.text(), &parsed.syntax).validated_plan() {
            Ok(plan) => {
                let value = materialize_data_plan(&plan, &mut heap, None);
                Some(SourcedValue {
                    value: DataWorld::new(heap, value.value),
                    provenance: value.provenance,
                })
            }
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                None
            }
        }
    } else {
        None
    };
    TomlParse {
        cst: parsed.syntax,
        value,
        diagnostics,
    }
}

#[cfg(test)]
pub(crate) fn materialize_toml_registered(
    sources: &SourceDatabase,
    source_id: SourceId,
    heap: &mut Heap,
) -> Result<crate::json::MaterializedValue, Vec<Diagnostic>> {
    let source = sources.get(source_id);
    materialize_toml_source(source_id, source.text(), heap)
}

#[cfg(test)]
pub(crate) fn materialize_toml_source(
    source_id: SourceId,
    source: &crate::document::DocumentText,
    heap: &mut Heap,
) -> Result<crate::json::MaterializedValue, Vec<Diagnostic>> {
    let parsed = crate::syntax::toml::parse_document(source_id, source);
    if !parsed.diagnostics.is_empty() {
        return Err(parsed.diagnostics);
    }
    let plan = TomlLowerer::new(source_id, source, &parsed.syntax)
        .validated_plan()
        .map_err(|diagnostic| vec![diagnostic])?;
    Ok(materialize_data_plan(&plan, heap, None))
}

#[derive(Clone)]
struct Entry {
    node: DataNodeId,
    key_location: Location,
}

#[derive(Clone)]
struct TableState {
    explicit: bool,
    sealed: bool,
}

struct TomlPlan {
    data: ValidatedDataPlan,
    tables: BTreeMap<DataNodeId, TableState>,
    table_arrays: std::collections::BTreeSet<DataNodeId>,
}

impl TomlPlan {
    fn new(root_location: Location) -> (Self, DataNodeId) {
        let mut plan = Self {
            data: ValidatedDataPlan::default(),
            tables: BTreeMap::new(),
            table_arrays: std::collections::BTreeSet::new(),
        };
        let root = plan.table(root_location, true);
        (plan, root)
    }

    fn table(&mut self, location: Location, explicit: bool) -> DataNodeId {
        let id = self.data.object(BTreeMap::new(), location);
        self.tables.insert(
            id,
            TableState {
                explicit,
                sealed: false,
            },
        );
        id
    }

    fn array(
        &mut self,
        values: Vec<DataNodeId>,
        location: Location,
        table_array: bool,
    ) -> DataNodeId {
        let id = self.data.array(values, location);
        if table_array {
            self.table_arrays.insert(id);
        }
        id
    }

    fn fields(&self, table: DataNodeId) -> &BTreeMap<String, DataField> {
        let DataPlanNodeKind::Object(fields) = &self.data.node(table).kind else {
            panic!("TOML table id must reference an object")
        };
        fields
    }

    fn fields_mut(&mut self, table: DataNodeId) -> &mut BTreeMap<String, DataField> {
        let DataPlanNodeKind::Object(fields) = &mut self.data.node_mut(table).kind else {
            panic!("TOML table id must reference an object")
        };
        fields
    }
}

#[derive(Clone)]
struct Conflict {
    message: String,
    previous: Location,
}

struct TomlLowerer<'a> {
    source_id: SourceId,
    source: &'a crate::document::DocumentText,
    cst: &'a CstData,
    current: Vec<String>,
}

impl<'a> TomlLowerer<'a> {
    fn new(
        source_id: SourceId,
        source: &'a crate::document::DocumentText,
        cst: &'a CstData,
    ) -> Self {
        Self {
            source_id,
            source,
            cst,
            current: Vec::new(),
        }
    }

    fn validated_plan(mut self) -> Result<ValidatedDataPlan, Diagnostic> {
        let root_location = Location::from_usize(self.source_id, 0..self.source.byte_len())
            .expect("source range fits Location");
        let (mut plan, root) = TomlPlan::new(root_location);
        let mut statements = Vec::new();
        self.collect_statements(NodeRef::ROOT, &mut statements);
        for statement in statements {
            self.statement(&mut plan, root, statement)?;
        }
        plan.data.set_root(root);
        Ok(plan.data)
    }

    fn collect_statements(&self, node: NodeRef, output: &mut Vec<NodeRef>) {
        for child in self.rule_children(node) {
            if matches!(
                self.rule(child),
                Some(Rule::Statement | Rule::KeyValue | Rule::TableTail)
            ) {
                output.push(child);
            } else {
                self.collect_statements(child, output);
            }
        }
    }

    fn statement(
        &mut self,
        plan: &mut TomlPlan,
        root: DataNodeId,
        node: NodeRef,
    ) -> Result<(), Diagnostic> {
        let key_value = if self.rule(node) == Some(Rule::KeyValue) {
            Some(node)
        } else {
            self.rule_children(node)
                .find(|child| self.rule(*child) == Some(Rule::KeyValue))
        };
        if let Some(key_value) = key_value {
            let current = self.current.clone();
            let table = table_at(plan, root, &current, self.location(node), false)
                .map_err(|conflict| self.conflict(node, conflict))?;
            return self.insert_key_value(plan, table, key_value);
        }
        let tail = if self.rule(node) == Some(Rule::TableTail) {
            node
        } else {
            self.rule_children(node)
                .find(|child| self.rule(*child) == Some(Rule::TableTail))
                .ok_or_else(|| self.error(node, "invalid TOML statement"))?
        };
        let key_node = self
            .rule_children(tail)
            .find(|child| self.rule(*child) == Some(Rule::Key))
            .ok_or_else(|| self.error(tail, "table header has no key"))?;
        let key = self.key(key_node)?;
        let array_table = self
            .children(tail)
            .next()
            .is_some_and(|child| matches!(self.cst.get(child), Node::Token(Token::LBracket, _)));
        let location = self.location(node);
        if array_table {
            open_array_table(plan, root, &key, location)
                .map_err(|conflict| self.conflict(node, conflict))?;
        } else {
            open_table(plan, root, &key, location)
                .map_err(|conflict| self.conflict(node, conflict))?;
        }
        self.current = key;
        Ok(())
    }

    fn insert_key_value(
        &self,
        plan: &mut TomlPlan,
        table: DataNodeId,
        node: NodeRef,
    ) -> Result<(), Diagnostic> {
        let key_node = self
            .rule_children(node)
            .find(|child| self.rule(*child) == Some(Rule::Key))
            .ok_or_else(|| self.error(node, "key/value pair has no key"))?;
        let value_node = self
            .children(node)
            .find(|child| self.rule(*child) == Some(Rule::Value) || self.is_value(*child))
            .ok_or_else(|| self.error(node, "key/value pair has no value"))?;
        let key = self.key(key_node)?;
        let value = self.value(plan, value_node)?;
        insert_entry(plan, table, &key, value, self.location(key_node))
            .map_err(|conflict| self.conflict(key_node, conflict))
    }

    fn key(&self, node: NodeRef) -> Result<Vec<String>, Diagnostic> {
        let mut output = Vec::new();
        for part in self
            .rule_children(node)
            .filter(|child| self.rule(*child) == Some(Rule::KeyPart))
        {
            let token = self
                .children(part)
                .find(|child| matches!(self.cst.get(*child), Node::Token(..)))
                .ok_or_else(|| self.error(part, "empty TOML key"))?;
            match self.cst.get(token) {
                Node::Token(Token::String, _) => output.push(self.decode_string(token)?),
                Node::Token(Token::Atom, _) => {
                    for component in self.text(token).split('.') {
                        if component.is_empty()
                            || !component.chars().all(|character| {
                                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                            })
                        {
                            return Err(self.error(token, "invalid bare TOML key"));
                        }
                        output.push(component.to_owned());
                    }
                }
                _ => return Err(self.error(token, "invalid TOML key")),
            }
        }
        if output.is_empty() {
            Err(self.error(node, "empty TOML key"))
        } else {
            Ok(output)
        }
    }

    fn value(&self, plan: &mut TomlPlan, node: NodeRef) -> Result<DataNodeId, Diagnostic> {
        let node = if self.rule(node) == Some(Rule::Value) {
            self.children(node)
                .find(|child| self.is_value(*child))
                .ok_or_else(|| self.error(node, "empty TOML value"))?
        } else {
            node
        };
        match self.cst.get(node) {
            Node::Token(Token::String, _) => Ok(plan.data.scalar(
                DataScalar::String(self.decode_string(node)?),
                self.location(node),
            )),
            Node::Token(Token::Atom, _) => self.atom(plan, node),
            Node::Rule(Rule::Array, _) => self.array(plan, node),
            Node::Rule(Rule::InlineTable, _) => self.inline_table(plan, node),
            _ => Err(self.error(node, "expected a TOML value")),
        }
    }

    fn atom(&self, plan: &mut TomlPlan, node: NodeRef) -> Result<DataNodeId, Diagnostic> {
        let text = self.text(node);
        let location = self.location(node);
        let value = match text.as_ref() {
            "true" => DataScalar::Atom("True".into()),
            "false" => DataScalar::Atom("False".into()),
            _ => {
                if let Some(temporal) = parse_temporal(&text) {
                    let (tag, canonical) = temporal.map_err(|message| self.error(node, message))?;
                    DataScalar::TaggedString {
                        tag: tag.into(),
                        value: canonical,
                    }
                } else {
                    parse_number(&text).map_err(|message| self.error(node, message))?
                }
            }
        };
        Ok(plan.data.scalar(value, location))
    }

    fn array(&self, plan: &mut TomlPlan, node: NodeRef) -> Result<DataNodeId, Diagnostic> {
        let mut values = Vec::new();
        self.collect_array_values(plan, node, &mut values)?;
        Ok(plan.array(values, self.location(node), false))
    }

    fn collect_array_values(
        &self,
        plan: &mut TomlPlan,
        node: NodeRef,
        output: &mut Vec<DataNodeId>,
    ) -> Result<(), Diagnostic> {
        for child in self.children(node) {
            match self.rule(child) {
                Some(Rule::Value | Rule::Array | Rule::InlineTable) => {
                    output.push(self.value(plan, child)?)
                }
                Some(Rule::ArrayTail) => self.collect_array_values(plan, child, output)?,
                _ if matches!(
                    self.cst.get(child),
                    Node::Token(Token::String | Token::Atom, _)
                ) =>
                {
                    output.push(self.value(plan, child)?)
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn inline_table(&self, plan: &mut TomlPlan, node: NodeRef) -> Result<DataNodeId, Diagnostic> {
        let table = plan.table(self.location(node), true);
        for key_value in self
            .rule_children(node)
            .filter(|child| self.rule(*child) == Some(Rule::KeyValue))
        {
            self.insert_key_value(plan, table, key_value)?;
        }
        seal_table(plan, table);
        Ok(table)
    }

    fn decode_string(&self, node: NodeRef) -> Result<String, Diagnostic> {
        let text = self.text(node);
        let literal = text.starts_with('\'');
        let multiline = text.starts_with("\"\"\"") || text.starts_with("'''");
        let quote_width = if multiline { 3 } else { 1 };
        let mut body = text[quote_width..text.len() - quote_width].to_owned();
        if multiline {
            if let Some(rest) = body.strip_prefix("\r\n") {
                body = rest.to_owned();
            } else if let Some(rest) = body.strip_prefix('\n') {
                body = rest.to_owned();
            }
        }
        if literal {
            validate_string_controls(&body, multiline)
                .map_err(|message| self.error(node, message))?;
            return Ok(normalize_multiline_newlines(body, multiline));
        }
        decode_basic_string(&body, multiline)
            .map(|value| normalize_multiline_newlines(value, multiline))
            .map_err(|message| self.error(node, message))
    }

    fn is_value(&self, node: NodeRef) -> bool {
        matches!(
            self.cst.get(node),
            Node::Token(Token::String | Token::Atom, _)
        ) || matches!(self.rule(node), Some(Rule::Array | Rule::InlineTable))
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
                    .expect("CST span fits source"),
            )
            .expect("CST span is valid UTF-8")
    }

    fn location(&self, node: NodeRef) -> Location {
        Location::from_usize(self.source_id, self.cst.span(node)).expect("CST span fits source")
    }

    fn error(&self, node: NodeRef, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.location(node))
    }

    fn conflict(&self, node: NodeRef, conflict: Conflict) -> Diagnostic {
        Diagnostic::error(conflict.message, self.location(node))
            .with_secondary("first defined here", conflict.previous)
    }
}

fn table_at(
    plan: &mut TomlPlan,
    table: DataNodeId,
    path: &[String],
    location: Location,
    dotted: bool,
) -> Result<DataNodeId, Conflict> {
    if path.is_empty() {
        return Ok(table);
    }
    if plan.tables[&table].sealed {
        return Err(Conflict {
            message: "cannot extend an inline TOML table".into(),
            previous: plan.data.node(table).location,
        });
    }
    let entry = match plan.fields(table).get(&path[0]).cloned() {
        Some(entry) => Entry {
            node: entry.value,
            key_location: entry.key_location,
        },
        None => {
            let child = plan.table(location, dotted);
            plan.fields_mut(table).insert(
                path[0].clone(),
                DataField {
                    value: child,
                    key_location: location,
                },
            );
            Entry {
                node: child,
                key_location: location,
            }
        }
    };
    let next = match &plan.data.node(entry.node).kind {
        DataPlanNodeKind::Object(_) => entry.node,
        DataPlanNodeKind::Array(values) if plan.table_arrays.contains(&entry.node) => {
            let Some(next) = values.last().copied() else {
                return Err(Conflict {
                    message: "array of tables has no current element".into(),
                    previous: entry.key_location,
                });
            };
            if !matches!(plan.data.node(next).kind, DataPlanNodeKind::Object(_)) {
                return Err(Conflict {
                    message: "array of tables has no current element".into(),
                    previous: entry.key_location,
                });
            }
            next
        }
        _ => {
            return Err(Conflict {
                message: format!("TOML key {:?} is not a table", path[0]),
                previous: entry.key_location,
            });
        }
    };
    table_at(plan, next, &path[1..], location, dotted)
}

fn insert_entry(
    plan: &mut TomlPlan,
    table: DataNodeId,
    path: &[String],
    value: DataNodeId,
    location: Location,
) -> Result<(), Conflict> {
    let (parents, name) = path.split_at(path.len() - 1);
    let target = table_at(plan, table, parents, location, true)?;
    if plan.tables[&target].sealed {
        return Err(Conflict {
            message: "cannot extend an inline TOML table".into(),
            previous: plan.data.node(target).location,
        });
    }
    if let Some(previous) = plan.fields(target).get(&name[0]) {
        return Err(Conflict {
            message: format!("duplicate TOML key {:?}", name[0]),
            previous: previous.key_location,
        });
    }
    plan.fields_mut(target).insert(
        name[0].clone(),
        DataField {
            value,
            key_location: location,
        },
    );
    Ok(())
}

fn open_table(
    plan: &mut TomlPlan,
    root: DataNodeId,
    path: &[String],
    location: Location,
) -> Result<(), Conflict> {
    let (parents, name) = path.split_at(path.len() - 1);
    let parent = table_at(plan, root, parents, location, false)?;
    match plan.fields(parent).get(&name[0]).cloned() {
        None => {
            let table = plan.table(location, true);
            plan.fields_mut(parent).insert(
                name[0].clone(),
                DataField {
                    value: table,
                    key_location: location,
                },
            );
            Ok(())
        }
        Some(entry) => match plan.tables.get_mut(&entry.value) {
            Some(table) if !table.explicit && !table.sealed => {
                table.explicit = true;
                Ok(())
            }
            _ => Err(Conflict {
                message: format!("TOML table {:?} is already defined", name[0]),
                previous: entry.key_location,
            }),
        },
    }
}

fn open_array_table(
    plan: &mut TomlPlan,
    root: DataNodeId,
    path: &[String],
    location: Location,
) -> Result<(), Conflict> {
    let (parents, name) = path.split_at(path.len() - 1);
    let parent = table_at(plan, root, parents, location, false)?;
    let table = plan.table(location, true);
    match plan.fields(parent).get(&name[0]).cloned() {
        None => {
            let array = plan.array(vec![table], location, true);
            plan.fields_mut(parent).insert(
                name[0].clone(),
                DataField {
                    value: array,
                    key_location: location,
                },
            );
            Ok(())
        }
        Some(entry) => match &mut plan.data.node_mut(entry.value).kind {
            DataPlanNodeKind::Array(values) if plan.table_arrays.contains(&entry.value) => {
                values.push(table);
                Ok(())
            }
            _ => Err(Conflict {
                message: format!("TOML key {:?} is not an array of tables", name[0]),
                previous: entry.key_location,
            }),
        },
    }
}

fn seal_table(plan: &mut TomlPlan, table: DataNodeId) {
    plan.tables
        .get_mut(&table)
        .expect("TOML table state exists")
        .sealed = true;
    let children = plan
        .fields(table)
        .values()
        .map(|field| field.value)
        .filter(|child| plan.tables.contains_key(child))
        .collect::<Vec<_>>();
    for child in children {
        seal_table(plan, child);
    }
}

fn parse_number(text: &str) -> Result<DataScalar, &'static str> {
    validate_numeric_underscores(text)?;
    let normalized = text.replace('_', "");
    match normalized.as_str() {
        "inf" | "+inf" | "-inf" | "nan" | "+nan" | "-nan" => {
            return Err("TOML Float must be finite");
        }
        _ => {}
    }
    let unsigned_prefix = normalized.trim_start_matches(['+', '-']);
    let radix_prefixed = unsigned_prefix.starts_with("0x")
        || unsigned_prefix.starts_with("0o")
        || unsigned_prefix.starts_with("0b");
    if radix_prefixed && text.starts_with(['+', '-']) {
        return Err("TOML radix integers cannot have a sign");
    }
    if !radix_prefixed && normalized.contains(['.', 'e', 'E']) {
        if invalid_leading_zero(&normalized) {
            return Err("invalid leading zero in TOML Float");
        }
        validate_float_syntax(&normalized)?;
        let value = normalized.parse::<f64>().map_err(|_| "invalid TOML Float");
        return value.and_then(|value| {
            value
                .is_finite()
                .then_some(DataScalar::Float(value))
                .ok_or("TOML Float must be finite")
        });
    }
    let (negative, unsigned) = normalized
        .strip_prefix('-')
        .map_or((false, normalized.as_str()), |value| (true, value));
    let unsigned = unsigned.strip_prefix('+').unwrap_or(unsigned);
    let (radix, digits) = if let Some(digits) = unsigned.strip_prefix("0x") {
        (16, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0o") {
        (8, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0b") {
        (2, digits)
    } else {
        if unsigned.len() > 1 && unsigned.starts_with('0') {
            return Err("invalid leading zero in TOML integer");
        }
        (10, unsigned)
    };
    if digits.is_empty() {
        return Err("invalid TOML integer");
    }
    let magnitude = i128::from_str_radix(digits, radix).map_err(|_| "invalid TOML integer")?;
    let signed = if negative { -magnitude } else { magnitude };
    i64::try_from(signed)
        .map(DataScalar::Int)
        .map_err(|_| "TOML integer is outside the i64 range")
}

fn validate_float_syntax(value: &str) -> Result<(), &'static str> {
    let unsigned = value.trim_start_matches(['+', '-']);
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, None), |(mantissa, exponent)| {
            (mantissa, Some(exponent))
        });
    if unsigned.matches(['e', 'E']).count() > 1 {
        return Err("invalid TOML Float");
    }
    if let Some((whole, fraction)) = mantissa.split_once('.') {
        if whole.is_empty()
            || fraction.is_empty()
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("invalid TOML Float");
        }
    } else if mantissa.is_empty() || !mantissa.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid TOML Float");
    }
    if let Some(exponent) = exponent {
        let digits = exponent.trim_start_matches(['+', '-']);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("invalid TOML Float exponent");
        }
    }
    Ok(())
}

fn validate_numeric_underscores(value: &str) -> Result<(), &'static str> {
    let unsigned = value.trim_start_matches(['+', '-']);
    let radix = if unsigned.starts_with("0x") {
        16
    } else if unsigned.starts_with("0o") {
        8
    } else if unsigned.starts_with("0b") {
        2
    } else {
        10
    };
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().copied().enumerate() {
        if byte != b'_' {
            continue;
        }
        let valid = |byte: u8| char::from(byte).is_digit(radix);
        if index == 0
            || index + 1 == bytes.len()
            || !valid(bytes[index - 1])
            || !valid(bytes[index + 1])
        {
            return Err("TOML numeric underscores must occur between digits");
        }
    }
    Ok(())
}

fn invalid_leading_zero(value: &str) -> bool {
    let value = value.trim_start_matches(['+', '-']);
    value.len() > 1
        && value.starts_with('0')
        && value.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
}

fn parse_temporal(text: &str) -> Option<Result<(&'static str, String), &'static str>> {
    if text.len() >= 10
        && text.as_bytes().get(4) == Some(&b'-')
        && text.as_bytes().get(7) == Some(&b'-')
    {
        return Some(parse_date_time(text));
    }
    if text.len() >= 8
        && text.as_bytes().get(2) == Some(&b':')
        && text.as_bytes().get(5) == Some(&b':')
    {
        return Some(parse_time(text).map(|time| ("LocalTime", time)));
    }
    None
}

fn parse_date_time(text: &str) -> Result<(&'static str, String), &'static str> {
    let date = &text[..10];
    validate_date(date)?;
    if text.len() == 10 {
        return Ok(("LocalDate", date.to_owned()));
    }
    let separator = text.as_bytes()[10];
    if !matches!(separator, b'T' | b't' | b' ') {
        return Err("invalid TOML date-time separator");
    }
    let remainder = &text[11..];
    let (time, offset) = split_offset(remainder)?;
    let time = parse_time(time)?;
    if let Some(offset) = offset {
        let offset = canonical_offset(offset)?;
        Ok(("OffsetDateTime", format!("{date}T{time}{offset}")))
    } else {
        Ok(("LocalDateTime", format!("{date}T{time}")))
    }
}

fn split_offset(time: &str) -> Result<(&str, Option<&str>), &'static str> {
    if let Some(value) = time.strip_suffix(['Z', 'z']) {
        return Ok((value, Some("Z")));
    }
    if time.len() > 8
        && let Some(index) = time[8..].find(['+', '-']).map(|index| index + 8)
    {
        return Ok((&time[..index], Some(&time[index..])));
    }
    Ok((time, None))
}

fn validate_date(date: &str) -> Result<(), &'static str> {
    if date.len() != 10 {
        return Err("invalid TOML date");
    }
    let year = decimal(&date[0..4])?;
    let month = decimal(&date[5..7])?;
    let day = decimal(&date[8..10])?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err("invalid TOML month"),
    };
    if day == 0 || day > days {
        return Err("invalid TOML day");
    }
    Ok(())
}

fn parse_time(time: &str) -> Result<String, &'static str> {
    if time.len() < 8
        || time.as_bytes().get(2) != Some(&b':')
        || time.as_bytes().get(5) != Some(&b':')
    {
        return Err("invalid TOML time");
    }
    let hour = decimal(&time[0..2])?;
    let minute = decimal(&time[3..5])?;
    let second = decimal(&time[6..8])?;
    if hour > 23 || minute > 59 || second > 59 {
        return Err("TOML time component is outside its valid range");
    }
    if time.len() > 8 {
        let fraction = time.strip_prefix(&time[..8]).expect("prefix exists");
        if !fraction.starts_with('.')
            || fraction.len() == 1
            || !fraction[1..].bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("invalid TOML fractional second");
        }
    }
    Ok(time.to_owned())
}

fn canonical_offset(offset: &str) -> Result<String, &'static str> {
    if matches!(offset, "Z" | "z") {
        return Ok("Z".into());
    }
    if offset.len() != 6 || offset.as_bytes().get(3) != Some(&b':') {
        return Err("invalid TOML date-time offset");
    }
    let hour = decimal(&offset[1..3])?;
    let minute = decimal(&offset[4..6])?;
    if hour > 23 || minute > 59 || !matches!(offset.as_bytes()[0], b'+' | b'-') {
        return Err("TOML date-time offset is outside its valid range");
    }
    if hour == 0 && minute == 0 {
        Ok("Z".into())
    } else {
        Ok(offset.to_owned())
    }
}

fn decimal(value: &str) -> Result<u32, &'static str> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid decimal digits in TOML temporal value");
    }
    value.parse().map_err(|_| "invalid TOML temporal value")
}

fn validate_string_controls(value: &str, multiline: bool) -> Result<(), &'static str> {
    if value.chars().any(|character| {
        character.is_control()
            && character != '\t'
            && !(multiline && matches!(character, '\n' | '\r'))
    }) {
        Err("TOML String contains a forbidden control character")
    } else {
        Ok(())
    }
}

fn decode_basic_string(value: &str, multiline: bool) -> Result<String, &'static str> {
    validate_string_controls(value, multiline)?;
    let mut output = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        if multiline && matches!(characters.peek(), Some('\n' | '\r')) {
            if characters.next() == Some('\r') && characters.peek() == Some(&'\n') {
                characters.next();
            }
            while characters
                .peek()
                .is_some_and(|character| character.is_whitespace())
            {
                characters.next();
            }
            continue;
        }
        let escaped = characters.next().ok_or("unterminated TOML escape")?;
        match escaped {
            'b' => output.push('\u{0008}'),
            't' => output.push('\t'),
            'n' => output.push('\n'),
            'f' => output.push('\u{000c}'),
            'r' => output.push('\r'),
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            'u' | 'U' => {
                let digits = if escaped == 'u' { 4 } else { 8 };
                let mut value = 0u32;
                for _ in 0..digits {
                    value = value
                        .checked_mul(16)
                        .and_then(|value| {
                            characters
                                .next()
                                .and_then(|character| character.to_digit(16))
                                .and_then(|digit| value.checked_add(digit))
                        })
                        .ok_or("invalid TOML Unicode escape")?;
                }
                output.push(char::from_u32(value).ok_or("invalid TOML Unicode scalar")?);
            }
            _ => return Err("unknown TOML escape"),
        }
    }
    Ok(output)
}

fn normalize_multiline_newlines(value: String, multiline: bool) -> String {
    if multiline && value.contains('\r') {
        value.replace("\r\n", "\n")
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> TomlParse {
        let mut sources = SourceDatabase::default();
        let id = sources.add("test.toml", source);
        parse_toml_registered(&sources, id)
    }

    #[test]
    fn direct_materialization_does_not_touch_the_target_on_validation_failure() {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("invalid.toml", "value = []\nvalue = 1\n");
        let mut heap = Heap::main();
        let before = heap.allocation_count();
        assert!(materialize_toml_registered(&sources, source_id, &mut heap).is_err());
        assert_eq!(heap.allocation_count(), before);
    }

    #[test]
    fn lowers_tables_arrays_inline_values_and_temporal_tags() {
        let parsed = parse(
            r#"title = "Telora"
when = 1979-05-27 07:32:00+00:00
local = 1979-05-27T07:32:00
dates = [1979-05-27, 07:32:00.1200]
point = { x = 1, y = 2 }
[owner]
name = 'Ada'
[[products]]
name = "one"
[[products]]
name = "two"
"#,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(
            parsed.value.unwrap().value.to_string(),
            "{dates: ['LocalDate(\"1979-05-27\"), 'LocalTime(\"07:32:00.1200\")], local: 'LocalDateTime(\"1979-05-27T07:32:00\"), owner: {name: \"Ada\"}, point: {x: 1, y: 2}, products: [{name: \"one\"}, {name: \"two\"}], title: \"Telora\", when: 'OffsetDateTime(\"1979-05-27T07:32:00Z\")}"
        );
    }

    #[test]
    fn rejects_invalid_dates_and_duplicate_keys() {
        let date = parse("when = 2025-02-29\n");
        assert!(date.value.is_none());
        assert!(date.diagnostics[0].message.contains("day"));

        let duplicate = parse("a = 1\na = 2\n");
        assert!(duplicate.value.is_none());
        assert_eq!(duplicate.diagnostics[0].labels.len(), 2);
    }

    #[test]
    fn decodes_toml_strings_numbers_and_rejects_table_conflicts() {
        let parsed = parse(
            "escaped = \"line\\n\\u5F62\"\nfolded = \"\"\"\nfirst\\\n  second\"\"\"\nhex = 0xDEAD_BEEF\nfloat = 1_000.50\n",
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(
            parsed.value.unwrap().value.to_string(),
            "{escaped: \"line\\n形\", float: 1000.5, folded: \"firstsecond\", hex: 3735928559}"
        );

        for source in [
            "value = 1__0\n",
            "value = 01\n",
            "a = {b = 1}\na.c = 2\n",
            "a = 1\n[a]\nb = 2\n",
            "[a]\nb = 1\n[a]\nc = 2\n",
            "a = []\n[[a]]\nb = 1\n",
            "a.b = 1\n[a]\nc = 2\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.value.is_none(), "accepted invalid TOML: {source}");
            assert!(!parsed.diagnostics.is_empty(), "{source}");
        }

        let implicit_header = parse("[a.b]\nvalue = 1\n[a]\nname = \"ok\"\n");
        assert!(
            implicit_header.diagnostics.is_empty(),
            "{:?}",
            implicit_header.diagnostics
        );
    }

    #[test]
    fn covers_toml_1_0_string_and_numeric_boundaries() {
        let parsed = parse(
            "four = \"\"\"one\"\"\"\"\nfive = '''two'''''\r\nlines = \"\"\"a\r\nb\"\"\"\r\nempty = \"\"\nquoted.key = 1\n\"quoted.key\" = 2\n",
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(
            parsed.value.unwrap().value.to_string(),
            "{empty: \"\", five: \"two''\", four: \"one\\\"\", lines: \"a\\nb\", quoted: {key: 1}, quoted.key: 2}"
        );

        for source in [
            "value = +0x1\n",
            "value = -0o7\n",
            "value = 1.\n",
            "value = 1.e2\n",
            "value = 1e\n",
            "value = 1e+\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.value.is_none(), "accepted invalid TOML: {source}");
        }
    }

    #[test]
    fn rejects_non_finite_float_values() {
        for source in [
            "value = inf\n",
            "value = -inf\n",
            "value = nan\n",
            "value = 1.0e9999\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.value.is_none(), "accepted {source}");
            assert!(parsed.diagnostics[0].message.contains("must be finite"));
        }

        let overflow = parse("value = 9223372036854775808\n");
        assert!(overflow.value.is_none());
        assert!(
            overflow.diagnostics[0]
                .message
                .contains("outside the i64 range")
        );
    }
}
