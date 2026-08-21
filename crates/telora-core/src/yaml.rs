use crate::DataWorld;
use crate::heap::Heap;
use crate::json::{
    DataField, DataNodeId, DataPlanNodeKind, DataScalar, SourcedValue, ValidatedDataPlan,
    materialize_data_plan,
};
use crate::source::{Diagnostic, Location, SourceDatabase, SourceId};
use crate::syntax::yaml::lexer::Token;
use crate::syntax::yaml::parser::{CstData, Node, NodeRef};
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct YamlParse {
    pub cst: crate::syntax::yaml::CstData,
    pub value: Option<SourcedValue>,
    pub diagnostics: Vec<Diagnostic>,
}

pub(crate) fn validate_yaml_registered(
    sources: &SourceDatabase,
    source_id: SourceId,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    let source = sources.get(source_id);
    let parsed = crate::syntax::yaml::parse_document(source_id, source.text());
    if !parsed.diagnostics.is_empty() {
        return Err(parsed.diagnostics);
    }
    YamlLowerer::new(source_id, source.text(), &parsed.syntax)
        .validated_plan()
        .map_err(|diagnostic| vec![diagnostic])
}

pub fn parse_yaml_registered(sources: &SourceDatabase, source_id: SourceId) -> YamlParse {
    let source = sources.get(source_id);
    let parsed = crate::syntax::yaml::parse_document(source_id, source.text());
    let mut diagnostics = parsed.diagnostics;
    let value = if diagnostics.is_empty() {
        let mut heap = Heap::work();
        match YamlLowerer::new(source_id, source.text(), &parsed.syntax).validated_plan() {
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
    YamlParse {
        cst: parsed.syntax,
        value,
        diagnostics,
    }
}

#[cfg(test)]
pub(crate) fn materialize_yaml_registered(
    sources: &SourceDatabase,
    source_id: SourceId,
    heap: &mut Heap,
) -> Result<crate::json::MaterializedValue, Vec<Diagnostic>> {
    let source = sources.get(source_id);
    materialize_yaml_source(source_id, source.text(), heap)
}

#[cfg(test)]
pub(crate) fn materialize_yaml_source(
    source_id: SourceId,
    source: &crate::document::DocumentText,
    heap: &mut Heap,
) -> Result<crate::json::MaterializedValue, Vec<Diagnostic>> {
    let parsed = crate::syntax::yaml::parse_document(source_id, source);
    if !parsed.diagnostics.is_empty() {
        return Err(parsed.diagnostics);
    }
    let plan = YamlLowerer::new(source_id, source, &parsed.syntax)
        .validated_plan()
        .map_err(|diagnostic| vec![diagnostic])?;
    Ok(materialize_data_plan(&plan, heap, None))
}

#[derive(Clone)]
struct Line {
    start: usize,
    end: usize,
    indent: usize,
    content_start: usize,
}

struct YamlLowerer<'a> {
    source_id: SourceId,
    source: &'a crate::document::DocumentText,
    source_len: usize,
    lines: Vec<Line>,
    position: usize,
    plan: ValidatedDataPlan,
    anchors: BTreeMap<String, DataNodeId>,
}

fn push_line(lines: &mut Vec<Line>, start: usize, text: &str) {
    let indent = text.bytes().take_while(|byte| *byte == b' ').count();
    lines.push(Line {
        start,
        end: start + text.len(),
        indent,
        content_start: start + indent,
    });
}

impl<'a> YamlLowerer<'a> {
    fn new(source_id: SourceId, source: &'a crate::document::DocumentText, cst: &CstData) -> Self {
        let source_len = source.byte_len();
        let mut lines = Vec::new();
        for node in cst.children(NodeRef::ROOT) {
            if !matches!(cst.get(node), Node::Token(Token::Line, _)) {
                continue;
            }
            let span = cst.span(node);
            let text = source
                .slice(
                    crate::source::TextRange::from_usize(span.clone())
                        .expect("YAML CST span fits registered source"),
                )
                .expect("YAML CST line is valid UTF-8");
            let raw = text.trim_end_matches('\n').trim_end_matches('\r');
            push_line(&mut lines, span.start, raw);
        }
        if source_len == 0 {
            lines.push(Line {
                start: 0,
                end: 0,
                indent: 0,
                content_start: 0,
            });
        }
        Self {
            source_id,
            source,
            source_len,
            lines,
            position: 0,
            plan: ValidatedDataPlan::default(),
            anchors: BTreeMap::new(),
        }
    }

    fn validated_plan(mut self) -> Result<ValidatedDataPlan, Diagnostic> {
        if let Some((index, _)) = self
            .lines
            .iter()
            .enumerate()
            .find(|(index, _)| self.line_content(*index).starts_with('\t'))
        {
            return Err(self.line_error(index, "tabs cannot be used for YAML indentation"));
        }
        self.skip_trivia();
        if self.current_content().as_deref() == Some("---") {
            self.position += 1;
            self.skip_trivia();
        }
        let node = if self.position == self.lines.len() {
            self.plan
                .scalar(DataScalar::Atom("None".into()), self.location(0, 0))
        } else {
            let indent = self.lines[self.position].indent;
            self.parse_block(indent)?
        };
        self.skip_trivia();
        if self.position < self.lines.len() {
            return Err(self.line_error(
                self.position,
                "YAML module must contain exactly one document",
            ));
        }
        self.plan.set_root(node);
        Ok(self.plan)
    }

    fn parse_block(&mut self, indent: usize) -> Result<DataNodeId, Diagnostic> {
        self.skip_trivia();
        let line = self
            .lines
            .get(self.position)
            .ok_or_else(|| self.eof_error("expected YAML value"))?;
        if line.indent != indent {
            return Err(self.line_error(self.position, "inconsistent YAML indentation"));
        }
        let line_start = line.start;
        let content = self.line_content(self.position).into_owned();
        if content == "-" || content.starts_with("- ") {
            self.parse_sequence(indent)
        } else if split_mapping(&content).is_some() {
            self.parse_mapping(indent, None)
        } else {
            let index = self.position;
            self.position += 1;
            self.parse_inline_value(index, content, line_start + indent)
        }
    }

    fn parse_sequence(&mut self, indent: usize) -> Result<DataNodeId, Diagnostic> {
        let start = self.lines[self.position].start + indent;
        let mut values = Vec::new();
        while self.position < self.lines.len() {
            self.skip_trivia();
            if self.position >= self.lines.len() || self.lines[self.position].indent != indent {
                break;
            }
            let index = self.position;
            let content = self.line_content(index).into_owned();
            let Some(rest) = content.strip_prefix('-') else {
                break;
            };
            if !rest.is_empty() && !rest.starts_with(' ') {
                break;
            }
            self.position += 1;
            let rest = rest.trim_start().to_owned();
            let value = if rest.is_empty() {
                self.skip_trivia();
                if self.position >= self.lines.len() || self.lines[self.position].indent <= indent {
                    return Err(self.line_error(index, "YAML sequence item has no value"));
                }
                self.parse_block(self.lines[self.position].indent)?
            } else if split_mapping(&rest).is_some() {
                self.parse_mapping(indent + 2, Some((index, rest)))?
            } else {
                self.parse_inline_value(index, rest, self.lines[index].start + indent + 2)?
            };
            values.push(value);
        }
        let end = values.last().map_or(start, |node| self.node_end(*node));
        Ok(self.plan.array(values, self.location(start, end)))
    }

    fn parse_mapping(
        &mut self,
        indent: usize,
        first: Option<(usize, String)>,
    ) -> Result<DataNodeId, Diagnostic> {
        let start = first.as_ref().map_or_else(
            || self.lines[self.position].start + indent,
            |(index, _)| self.lines[*index].start + self.lines[*index].indent + 2,
        );
        let mut entries = Vec::new();
        let mut merged = Vec::new();
        let mut seen = BTreeMap::<String, Location>::new();
        let mut pending = first;
        loop {
            self.skip_trivia();
            let (index, content) = if let Some(first) = pending.take() {
                first
            } else {
                if self.position >= self.lines.len() || self.lines[self.position].indent != indent {
                    break;
                }
                let index = self.position;
                let content = self.line_content(index).into_owned();
                if split_mapping(&content).is_none() {
                    break;
                }
                self.position += 1;
                (index, content)
            };
            let (key_text, rest) = split_mapping(&content)
                .ok_or_else(|| self.line_error(index, "expected YAML mapping entry"))?;
            let key_offset = self.lines[index].start + self.lines[index].indent;
            let key = parse_key(key_text).map_err(|message| {
                Diagnostic::error(
                    message,
                    self.location(key_offset, key_offset + key_text.len()),
                )
            })?;
            let key_location = self.location(key_offset, key_offset + key_text.len());
            if key != "<<"
                && let Some(previous) = seen.insert(key.clone(), key_location)
            {
                return Err(
                    Diagnostic::error(format!("duplicate YAML key {key:?}"), key_location)
                        .with_secondary("first defined here", previous),
                );
            }
            let rest = rest.trim_start();
            let value = if rest.is_empty() {
                self.skip_trivia();
                if self.position >= self.lines.len() || self.lines[self.position].indent <= indent {
                    self.plan
                        .scalar(DataScalar::Atom("None".into()), key_location)
                } else {
                    self.parse_block(self.lines[self.position].indent)?
                }
            } else if rest.starts_with('|') || rest.starts_with('>') {
                self.parse_block_scalar(index, rest, indent)?
            } else {
                let value_offset = self.lines[index].end.saturating_sub(rest.len());
                self.parse_inline_value(index, rest.to_owned(), value_offset)?
            };
            if key == "<<" {
                collect_merge_entries(&self.plan, value, &mut merged)
                    .map_err(|message| Diagnostic::error(message, key_location))?;
            } else {
                entries.push((key, key_location, value));
            }
        }
        let explicit = entries
            .iter()
            .map(|(key, _, _)| key.as_str())
            .collect::<std::collections::HashSet<_>>();
        let mut effective = Vec::new();
        let mut merged_seen = BTreeMap::new();
        for (key, location, value) in merged {
            if explicit.contains(key.as_str()) {
                continue;
            }
            if let Some(previous) = merged_seen.insert(key.clone(), location) {
                return Err(Diagnostic::error(
                    format!("duplicate effective YAML merge key {key:?}"),
                    location,
                )
                .with_secondary("first merged here", previous));
            }
            effective.push((key, location, value));
        }
        effective.extend(entries);
        let entries = effective;
        let end = entries
            .last()
            .map_or(start, |(_, _, value)| self.node_end(*value));
        let fields = entries
            .into_iter()
            .map(|(name, key_location, value)| {
                (
                    name,
                    DataField {
                        key_location,
                        value,
                    },
                )
            })
            .collect();
        Ok(self.plan.object(fields, self.location(start, end)))
    }

    fn parse_inline_value(
        &mut self,
        line: usize,
        text: String,
        offset: usize,
    ) -> Result<DataNodeId, Diagnostic> {
        let text = strip_comment(&text).trim().to_owned();
        if let Some(encoded) = text.strip_prefix("!!binary") {
            let encoded = encoded.trim();
            if encoded.is_empty() {
                return Err(self.line_error(line, "YAML !!binary requires base64 data"));
            }
            let location = self.location(offset, offset + text.len());
            let bytes =
                decode_base64(encoded).map_err(|message| Diagnostic::error(message, location))?;
            return Ok(self.plan.scalar(DataScalar::Bytes(bytes), location));
        }
        if text.starts_with('!') {
            return Err(self.line_error(line, "custom YAML tags are not supported"));
        }
        if let Some(alias) = text.strip_prefix('*') {
            if !valid_anchor_name(alias) {
                return Err(self.line_error(line, "invalid YAML alias"));
            }
            let anchored = self.anchors.get(alias).copied().ok_or_else(|| {
                self.line_error(line, format!("unknown or cyclic YAML alias {alias:?}"))
            })?;
            return Ok(self
                .plan
                .clone_root_at(anchored, self.location(offset, offset + text.len())));
        }
        if let Some(anchor) = text.strip_prefix('&') {
            let split = anchor.find(char::is_whitespace).unwrap_or(anchor.len());
            let (name, rest) = anchor.split_at(split);
            if !valid_anchor_name(name) || rest.trim().is_empty() {
                return Err(self.line_error(line, "YAML anchor must name a value"));
            }
            let node = self.parse_inline_value(line, rest.trim().to_owned(), offset + split + 1)?;
            if self.anchors.insert(name.to_owned(), node).is_some() {
                return Err(self.line_error(line, format!("duplicate YAML anchor {name:?}")));
            }
            return Ok(node);
        }
        let location = self.location(offset, offset + text.len());
        if text.starts_with('[') || text.starts_with('{') {
            let node =
                FlowParser::new(self.source_id, offset, &text, &mut self.plan, &self.anchors)
                    .parse()?;
            return Ok(node);
        }
        parse_scalar(&mut self.plan, &text, location)
            .map_err(|message| Diagnostic::error(message, location))
    }

    fn parse_block_scalar(
        &mut self,
        line: usize,
        header: &str,
        parent_indent: usize,
    ) -> Result<DataNodeId, Diagnostic> {
        let style = header.as_bytes()[0];
        let indicators = header[1..].trim();
        if indicators.len() > 2
            || !indicators
                .chars()
                .all(|ch| ch == '+' || ch == '-' || ('1'..='9').contains(&ch))
        {
            return Err(self.line_error(line, "invalid YAML block scalar header"));
        }
        let chomping = indicators.chars().find(|ch| matches!(ch, '+' | '-'));
        let explicit = indicators
            .chars()
            .find_map(|ch| ch.to_digit(10))
            .map(|n| parent_indent + n as usize);
        let start_pos = self.position;
        let inferred = (start_pos..self.lines.len())
            .filter(|index| {
                !self.line_content(*index).trim().is_empty()
                    && self.lines[*index].indent > parent_indent
            })
            .map(|index| self.lines[index].indent)
            .next();
        let content_indent = explicit.or(inferred).unwrap_or(parent_indent + 1);
        let mut pieces = Vec::new();
        let mut end = self.lines[line].end;
        while self.position < self.lines.len() {
            let candidate = &self.lines[self.position];
            let candidate_indent = candidate.indent;
            let candidate_end = candidate.end;
            let content = self.line_content(self.position).into_owned();
            if !content.trim().is_empty() && candidate_indent < content_indent {
                break;
            }
            if candidate_indent <= parent_indent && content.trim().is_empty() {
                pieces.push((String::new(), false));
            } else {
                let extra = candidate_indent.saturating_sub(content_indent);
                pieces.push((format!("{}{}", " ".repeat(extra), content), extra > 0));
            }
            end = candidate_end;
            self.position += 1;
        }
        let mut value = if style == b'|' {
            pieces
                .iter()
                .map(|(line, _)| line.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            fold_lines(&pieces)
        };
        match chomping {
            Some('-') => {
                while value.ends_with('\n') {
                    value.pop();
                }
            }
            Some('+') => {
                value.push('\n');
            }
            _ => {
                while value.ends_with('\n') {
                    value.pop();
                }
                if !pieces.is_empty() {
                    value.push('\n');
                }
            }
        }
        Ok(self.plan.scalar(
            DataScalar::String(value),
            self.location(self.lines[line].start, end),
        ))
    }

    fn skip_trivia(&mut self) {
        while self.position < self.lines.len() {
            let content = self.line_content(self.position);
            let content = content.trim();
            if content.is_empty() || content.starts_with('#') {
                self.position += 1;
            } else {
                break;
            }
        }
    }

    fn current_content(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.lines
            .get(self.position)
            .map(|_| self.line_content(self.position))
            .map(|content| match content {
                std::borrow::Cow::Borrowed(content) => std::borrow::Cow::Borrowed(content.trim()),
                std::borrow::Cow::Owned(content) => {
                    std::borrow::Cow::Owned(content.trim().to_owned())
                }
            })
    }

    fn line_content(&self, line: usize) -> std::borrow::Cow<'_, str> {
        let line = &self.lines[line];
        self.source
            .slice(
                crate::source::TextRange::from_usize(line.content_start..line.end)
                    .expect("YAML line span fits registered source"),
            )
            .expect("YAML line span is valid UTF-8")
    }
    fn location(&self, start: usize, end: usize) -> Location {
        Location::from_usize(self.source_id, start..end).expect("YAML range fits Location")
    }
    fn line_error(&self, line: usize, message: impl Into<String>) -> Diagnostic {
        let line = &self.lines[line];
        Diagnostic::error(message, self.location(line.start, line.end))
    }
    fn eof_error(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.location(self.source_len, self.source_len))
    }

    fn node_end(&self, node: DataNodeId) -> usize {
        self.plan.node(node).location.end as usize
    }
}

fn split_mapping(text: &str) -> Option<(&str, &str)> {
    let mut quote = None;
    let mut depth = 0usize;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote == Some('"') && ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '[' | '{' => depth += 1,
            ']' | '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0
                && text[index + 1..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace) =>
            {
                return Some((&text[..index], &text[index + 1..]));
            }
            _ => {}
        }
    }
    None
}

fn parse_key(text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty YAML mapping key");
    }
    if text.starts_with('[') || text.starts_with('{') || text.starts_with('?') {
        return Err("YAML mapping keys must be Strings");
    }
    if let Some(value) = decode_quoted(text) {
        return Ok(value);
    }
    if text.starts_with(['\'', '"']) || core_non_string(text) {
        return Err("YAML mapping keys must be Strings");
    }
    Ok(text.to_owned())
}

fn parse_scalar(
    plan: &mut ValidatedDataPlan,
    text: &str,
    location: Location,
) -> Result<DataNodeId, &'static str> {
    if let Some(value) = decode_quoted(text) {
        return Ok(plan.scalar(DataScalar::String(value), location));
    }
    if text.starts_with(['\'', '"']) {
        return Err("invalid quoted YAML String");
    }
    let value = match text {
        "" | "~" | "null" | "Null" | "NULL" => DataScalar::Atom("None".into()),
        "true" | "True" | "TRUE" => DataScalar::Atom("True".into()),
        "false" | "False" | "FALSE" => DataScalar::Atom("False".into()),
        ".inf" | ".Inf" | ".INF" | "-.inf" | "-.Inf" | "-.INF" | ".nan" | ".NaN" | ".NAN" => {
            return Err("YAML Float must be finite");
        }
        _ if looks_integer(text) => DataScalar::Int(parse_yaml_int(text)?),
        _ if looks_float(text) => {
            let value = text
                .replace('_', "")
                .parse::<f64>()
                .map_err(|_| "invalid YAML Float")?;
            if !value.is_finite() {
                return Err("YAML Float must be finite");
            }
            DataScalar::Float(value)
        }
        _ => DataScalar::String(text.into()),
    };
    Ok(plan.scalar(value, location))
}

fn collect_merge_entries(
    plan: &ValidatedDataPlan,
    value: DataNodeId,
    output: &mut Vec<(String, Location, DataNodeId)>,
) -> Result<(), &'static str> {
    match &plan.node(value).kind {
        DataPlanNodeKind::Object(entries) => {
            output.extend(
                entries
                    .iter()
                    .map(|(name, field)| (name.clone(), field.key_location, field.value)),
            );
            Ok(())
        }
        DataPlanNodeKind::Array(values) => {
            for value in values {
                let DataPlanNodeKind::Object(entries) = &plan.node(*value).kind else {
                    return Err("YAML merge sequence items must be mappings");
                };
                output.extend(
                    entries
                        .iter()
                        .map(|(name, field)| (name.clone(), field.key_location, field.value)),
                );
            }
            Ok(())
        }
        _ => Err("YAML merge value must be a mapping or sequence of mappings"),
    }
}

fn decode_base64(text: &str) -> Result<Vec<u8>, &'static str> {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let compact = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if compact.is_empty() || compact.len() % 4 != 0 {
        return Err("YAML !!binary contains invalid base64 data");
    }
    let mut output = Vec::with_capacity(compact.len() / 4 * 3);
    for (index, chunk) in compact.chunks_exact(4).enumerate() {
        let last = index + 1 == compact.len() / 4;
        let padding = usize::from(chunk[3] == b'=') + usize::from(chunk[2] == b'=');
        if padding > 0 && !last || padding == 1 && chunk[2] == b'=' || padding > 2 {
            return Err("YAML !!binary contains invalid base64 padding");
        }
        let a = digit(chunk[0]).ok_or("YAML !!binary contains invalid base64 data")?;
        let b = digit(chunk[1]).ok_or("YAML !!binary contains invalid base64 data")?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            digit(chunk[2]).ok_or("YAML !!binary contains invalid base64 data")?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            digit(chunk[3]).ok_or("YAML !!binary contains invalid base64 data")?
        };
        if padding == 2 && b & 0x0f != 0 || padding == 1 && c & 0x03 != 0 {
            return Err("YAML !!binary contains non-canonical base64 data");
        }
        output.push((a << 2) | (b >> 4));
        if padding < 2 {
            output.push((b << 4) | (c >> 2));
        }
        if padding == 0 {
            output.push((c << 6) | d);
        }
    }
    Ok(output)
}

fn looks_integer(text: &str) -> bool {
    let unsigned = text.trim_start_matches(['+', '-']);
    !unsigned.is_empty()
        && (unsigned.bytes().all(|b| b.is_ascii_digit() || b == b'_')
            || unsigned.starts_with("0x")
            || unsigned.starts_with("0o"))
}
fn parse_yaml_int(text: &str) -> Result<i64, &'static str> {
    let normalized = text.replace('_', "");
    let (negative, unsigned) = normalized
        .strip_prefix('-')
        .map_or((false, normalized.as_str()), |v| (true, v));
    let unsigned = unsigned.strip_prefix('+').unwrap_or(unsigned);
    let (radix, digits) = unsigned.strip_prefix("0x").map_or_else(
        || {
            unsigned
                .strip_prefix("0o")
                .map_or((10, unsigned), |v| (8, v))
        },
        |v| (16, v),
    );
    let magnitude = i128::from_str_radix(digits, radix).map_err(|_| "invalid YAML integer")?;
    i64::try_from(if negative { -magnitude } else { magnitude })
        .map_err(|_| "YAML integer is outside the i64 range")
}
fn looks_float(text: &str) -> bool {
    text.contains(['.', 'e', 'E']) && text.chars().any(|ch| ch.is_ascii_digit())
}

fn decode_quoted(text: &str) -> Option<String> {
    if text.len() < 2 {
        return None;
    }
    if text.starts_with('\'') && text.ends_with('\'') {
        return Some(text[1..text.len() - 1].replace("''", "'"));
    }
    if !(text.starts_with('"') && text.ends_with('"')) {
        return None;
    }
    let mut output = String::new();
    let mut chars = text[1..text.len() - 1].chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next()? {
            '0' => output.push('\0'),
            'a' => output.push('\u{7}'),
            'b' => output.push('\u{8}'),
            't' | '\t' => output.push('\t'),
            'n' => output.push('\n'),
            'v' => output.push('\u{b}'),
            'f' => output.push('\u{c}'),
            'r' => output.push('\r'),
            'e' => output.push('\u{1b}'),
            '"' => output.push('"'),
            '/' => output.push('/'),
            '\\' => output.push('\\'),
            escaped @ ('x' | 'u' | 'U') => {
                let digits = match escaped {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let mut value = 0u32;
                for _ in 0..digits {
                    value = value.checked_mul(16)? + chars.next()?.to_digit(16)?;
                }
                output.push(char::from_u32(value)?);
            }
            _ => return None,
        }
    }
    Some(output)
}

fn strip_comment(text: &str) -> &str {
    let mut quote = None;
    for (index, ch) in text.char_indices() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch == '#' && (index == 0 || text[..index].ends_with(char::is_whitespace)) {
            return &text[..index];
        }
    }
    text
}
fn valid_anchor_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}
fn fold_lines(lines: &[(String, bool)]) -> String {
    let mut output = String::new();
    for (index, (line, more_indented)) in lines.iter().enumerate() {
        output.push_str(line);
        if index + 1 < lines.len() {
            output.push(
                if line.is_empty()
                    || lines[index + 1].0.is_empty()
                    || *more_indented
                    || lines[index + 1].1
                {
                    '\n'
                } else {
                    ' '
                },
            );
        }
    }
    output
}
fn core_non_string(text: &str) -> bool {
    matches!(
        text,
        "~" | "null"
            | "Null"
            | "NULL"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | ".inf"
            | ".Inf"
            | ".INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | ".nan"
            | ".NaN"
            | ".NAN"
    ) || looks_integer(text)
        || text.parse::<f64>().is_ok()
}

struct FlowParser<'a> {
    source_id: SourceId,
    offset: usize,
    text: &'a str,
    pos: usize,
    plan: &'a mut ValidatedDataPlan,
    anchors: &'a BTreeMap<String, DataNodeId>,
}
impl<'a> FlowParser<'a> {
    fn new(
        source_id: SourceId,
        offset: usize,
        text: &'a str,
        plan: &'a mut ValidatedDataPlan,
        anchors: &'a BTreeMap<String, DataNodeId>,
    ) -> Self {
        Self {
            source_id,
            offset,
            text,
            pos: 0,
            plan,
            anchors,
        }
    }
    fn parse(mut self) -> Result<DataNodeId, Diagnostic> {
        let value = self.value()?;
        self.ws();
        if self.pos != self.text.len() {
            return Err(self.error("unexpected YAML flow content"));
        }
        Ok(value)
    }
    fn value(&mut self) -> Result<DataNodeId, Diagnostic> {
        self.ws();
        let start = self.pos;
        match self.peek() {
            Some('[') => {
                self.bump();
                let mut values = Vec::new();
                loop {
                    self.ws();
                    if self.take(']') {
                        break;
                    }
                    values.push(self.value()?);
                    self.ws();
                    if self.take(']') {
                        break;
                    }
                    self.expect(',')?;
                }
                Ok(self.plan.array(values, self.loc(start, self.pos)))
            }
            Some('{') => {
                self.bump();
                let mut entries = Vec::new();
                let mut merged = Vec::new();
                let mut seen = BTreeMap::new();
                loop {
                    self.ws();
                    if self.take('}') {
                        break;
                    }
                    let key_start = self.pos;
                    let key_text = self.scalar_text(&[':'])?;
                    let key = parse_key(key_text.trim()).map_err(|m| self.error(m))?;
                    let key_loc = self.loc(key_start, self.pos);
                    if key != "<<"
                        && let Some(previous) = seen.insert(key.clone(), key_loc)
                    {
                        return Err(Diagnostic::error(
                            format!("duplicate YAML key {key:?}"),
                            key_loc,
                        )
                        .with_secondary("first defined here", previous));
                    }
                    self.expect(':')?;
                    let value = self.value()?;
                    if key == "<<" {
                        collect_merge_entries(self.plan, value, &mut merged)
                            .map_err(|message| self.error(message))?;
                    } else {
                        entries.push((key, key_loc, value));
                    }
                    self.ws();
                    if self.take('}') {
                        break;
                    }
                    self.expect(',')?;
                }
                let explicit = entries
                    .iter()
                    .map(|(key, _, _)| key.as_str())
                    .collect::<std::collections::HashSet<_>>();
                let mut effective = Vec::new();
                let mut merged_seen = BTreeMap::new();
                for (key, location, value) in merged {
                    if explicit.contains(key.as_str()) {
                        continue;
                    }
                    if let Some(previous) = merged_seen.insert(key.clone(), location) {
                        return Err(Diagnostic::error(
                            format!("duplicate effective YAML merge key {key:?}"),
                            location,
                        )
                        .with_secondary("first merged here", previous));
                    }
                    effective.push((key, location, value));
                }
                effective.extend(entries);
                let fields = effective
                    .into_iter()
                    .map(|(name, key_location, value)| {
                        (
                            name,
                            DataField {
                                key_location,
                                value,
                            },
                        )
                    })
                    .collect();
                Ok(self.plan.object(fields, self.loc(start, self.pos)))
            }
            _ => {
                let raw = self.scalar_text(&[',', ']', '}'])?.trim();
                let loc = self.loc(start, self.pos);
                if let Some(encoded) = raw.strip_prefix("!!binary") {
                    let bytes =
                        decode_base64(encoded.trim()).map_err(|message| self.error(message))?;
                    return Ok(self.plan.scalar(DataScalar::Bytes(bytes), loc));
                }
                if raw.starts_with('!') {
                    return Err(self.error("custom YAML tags are not supported"));
                }
                if let Some(name) = raw.strip_prefix('*') {
                    let anchored = self
                        .anchors
                        .get(name)
                        .copied()
                        .ok_or_else(|| self.error(format!("unknown YAML alias {name:?}")));
                    let anchored = anchored?;
                    return Ok(self.plan.clone_root_at(anchored, loc));
                }
                parse_scalar(self.plan, raw, loc).map_err(|m| self.error(m))
            }
        }
    }
    fn scalar_text(&mut self, stops: &[char]) -> Result<&'a str, Diagnostic> {
        let start = self.pos;
        let mut quote = None;
        while let Some(ch) = self.peek() {
            if let Some(q) = quote {
                self.bump();
                if ch == q {
                    quote = None;
                } else if q == '"' && ch == '\\' {
                    self.bump();
                }
            } else if matches!(ch, '\'' | '"') {
                quote = Some(ch);
                self.bump();
            } else if stops.contains(&ch) {
                break;
            } else {
                self.bump();
            }
        }
        if self.pos == start {
            Err(self.error("expected YAML flow scalar"))
        } else {
            Ok(&self.text[start..self.pos])
        }
    }
    fn ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }
    fn bump(&mut self) {
        if let Some(ch) = self.peek() {
            self.pos += ch.len_utf8();
        }
    }
    fn take(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, expected: char) -> Result<(), Diagnostic> {
        self.ws();
        if self.take(expected) {
            Ok(())
        } else {
            Err(self.error(format!("expected {expected:?} in YAML flow value")))
        }
    }
    fn loc(&self, start: usize, end: usize) -> Location {
        Location::from_usize(self.source_id, self.offset + start..self.offset + end)
            .expect("flow range fits")
    }
    fn error(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.loc(self.pos, self.pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(source: &str) -> YamlParse {
        let mut sources = SourceDatabase::default();
        let id = sources.add("test.yaml", source);
        parse_yaml_registered(&sources, id)
    }
    #[test]
    fn direct_materialization_does_not_touch_the_target_on_validation_failure() {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("invalid.yaml", "ok: []\nok: 1\n");
        let mut heap = Heap::main();
        let before = heap.allocation_count();
        assert!(materialize_yaml_registered(&sources, source_id, &mut heap).is_err());
        assert_eq!(heap.allocation_count(), before);
    }
    #[test]
    fn lowers_core_schema_collections_aliases_and_block_scalars() {
        let parsed = parse(
            "name: Telora\nenabled: true\nlegacy: yes\nwhen: 2026-08-04\nbase: &pair [1, 2]\ncopy: *pair\nitems:\n  - one\n  - {name: two, ok: false}\nliteral: |-\n  a\n  b\nfolded: >\n  hello\n  world\n",
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(
            parsed.value.unwrap().value.to_string(),
            "{base: [1, 2], copy: [1, 2], enabled: 'True, folded: \"hello world\\n\", items: [\"one\", {name: \"two\", ok: 'False}], legacy: \"yes\", literal: \"a\\nb\", name: \"Telora\", when: \"2026-08-04\"}"
        );
    }
    #[test]
    fn rejects_ambiguous_yaml_features() {
        for source in [
            "a: 1\na: 2\n",
            "value: !thing x\n",
            "base: &x [1]\nvalue: {<<: *x}\n",
            "---\na: 1\n---\nb: 2\n",
            "value: *later\nlater: &later 1\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.value.is_none(), "accepted {source}");
        }
    }

    #[test]
    fn expands_mapping_merges_and_decodes_the_standard_binary_tag() {
        let parsed = parse(
            "defaults: &defaults {a: 1, b: 2}\nitem:\n  <<: *defaults\n  b: 3\n  bytes: !!binary SGk=\nflow: {<<: *defaults, b: 4}\n",
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let value = parsed.value.unwrap().value.to_string();
        assert!(
            value.contains("item: {a: 1, b: 3, bytes: b\"\\x48\\x69\"}"),
            "{value}"
        );
        assert!(value.contains("flow: {a: 1, b: 4}"), "{value}");

        for source in [
            "value: !!binary SGk\n",
            "value: !!binary SG==\n",
            "value: {tagged: !custom x}\n",
            "value: {1: text}\n",
            "base: &base {a: 1}\nvalue: {<<: [*base, 1]}\n",
            "a: &a {x: 1}\nb: &b {x: 2}\nvalue: {<<: [*a, *b]}\n",
            "root: &root {self: *root}\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.value.is_none(), "accepted invalid YAML: {source}");
            assert!(!parsed.diagnostics.is_empty(), "{source}");
        }

        let mut sources = SourceDatabase::default();
        let source = "base: &base [0, 1]\nitems: [*base, *base]\n";
        let source_id = sources.add("aliases.yaml", source);
        let plan = validate_yaml_registered(&sources, source_id).unwrap();
        let stats = plan
            .enforce_limits(crate::DataLimits::default(), source.len())
            .unwrap();
        // Root Object + base Array and its two children + items Array, two
        // alias Arrays, and both pairs of aliased children.
        assert_eq!(stats.nodes, 11);
        assert!(
            plan.enforce_limits(
                crate::DataLimits {
                    nodes: 10,
                    ..crate::DataLimits::default()
                },
                source.len(),
            )
            .unwrap_err()
            .to_string()
            .contains("nodes")
        );
    }

    #[test]
    fn rejects_non_finite_float_values() {
        for source in [
            "value: .inf\n",
            "value: -.inf\n",
            "value: .nan\n",
            "value: 1.0e9999\n",
        ] {
            let parsed = parse(source);
            assert!(parsed.value.is_none(), "accepted {source}");
            assert!(parsed.diagnostics[0].message.contains("must be finite"));
        }
    }
}
