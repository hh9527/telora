//! YAML's supported data subset: explicit block/flow frames, no references or
//! merge expansion, and resource admission before decoded payload allocation.
use crate::{
    DataLimits,
    source::SourceText,
    json::{DataField, DataNodeId, DataScalar, ValidatedDataPlan},
    source::{Diagnostic, Location, SourceDatabase, SourceId, TextRange},
};
use alloc::{borrow::Cow, collections::BTreeMap, string::String, vec::Vec};
use core::ops::Range;

mod block_scalar;
mod build;
mod flow;
mod lexer;
mod lines;
mod scalar;
use build::Build;
use lines::Line;

pub(crate) fn parse_with_limits(
    sources: &SourceDatabase,
    source: SourceId,
    limits: DataLimits,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    Parser::new(source, sources.get(source).text(), limits)
        .and_then(Parser::parse)
        .map_err(|error| vec![error])
}

struct Sequence {
    indent: usize,
    depth: usize,
    start: usize,
    items: Vec<DataNodeId>,
    waiting: bool,
}
struct Mapping {
    indent: usize,
    depth: usize,
    start: usize,
    end: usize,
    fields: BTreeMap<String, DataField>,
    first: Option<Range<usize>>,
    waiting: Option<(String, Location)>,
}
enum Task {
    Block { indent: usize, depth: usize },
    Sequence(Sequence),
    Mapping(Mapping),
}

struct Parser<'a> {
    source: &'a SourceText,
    lines: Vec<Line>,
    position: usize,
    build: Build,
    tasks: Vec<Task>,
    result: Option<DataNodeId>,
}

impl<'a> Parser<'a> {
    fn new(id: SourceId, source: &'a SourceText, limits: DataLimits) -> Result<Self, Diagnostic> {
        let build = Build::new(id, limits);
        build.check(
            build.loc(0..source.byte_len()),
            "file_size",
            source.byte_len(),
            limits.file_size,
        )?;
        let lines = lines::index(source.chunks());
        Ok(Self {
            source,
            lines,
            position: 0,
            build,
            tasks: Vec::new(),
            result: None,
        })
    }
    fn text(&self, range: Range<usize>) -> Cow<'a, str> {
        self.source
            .slice(TextRange::from_usize(range).expect("registered YAML range"))
            .expect("YAML character boundaries")
    }
    fn content(&self, index: usize) -> Cow<'a, str> {
        let line = self.lines[index];
        self.text(line.start + line.indent..line.end)
    }
    fn skip(&mut self) {
        while self.position < self.lines.len() {
            // Multiple frames can finish at this same line. Classify trivia
            // once, rather than repeatedly copying/scanning a long next line.
            let trivia = if let Some(trivia) = self.lines[self.position].trivia {
                trivia
            } else {
                let content = self.content(self.position);
                let trivia = content.trim().is_empty() || content.trim_start().starts_with('#');
                self.lines[self.position].trivia = Some(trivia);
                trivia
            };
            if !trivia {
                break;
            }
            self.position += 1;
        }
    }
    fn here(&self) -> Location {
        self.lines.get(self.position).map_or_else(
            || {
                self.build
                    .loc(self.source.byte_len()..self.source.byte_len())
            },
            |line| self.build.loc(line.start + line.indent..line.end),
        )
    }
    fn parse(mut self) -> Result<ValidatedDataPlan, Diagnostic> {
        for line in &self.lines {
            if line.tab_indent {
                return Err(self.build.error(
                    line.start..line.end,
                    "tabs cannot be used for YAML indentation",
                ));
            }
        }
        self.skip();
        if self.position < self.lines.len() && self.content(self.position).trim() == "---" {
            self.position += 1;
            self.skip();
        }
        if self.position == self.lines.len() {
            let loc = self.build.loc(0..0);
            self.build.reserve(1, loc)?;
            self.result = Some(self.build.plan.scalar(DataScalar::Null, loc));
        } else {
            self.tasks.push(Task::Block {
                indent: self.lines[self.position].indent,
                depth: 1,
            });
        }
        while let Some(task) = self.tasks.pop() {
            match task {
                Task::Block { indent, depth } => self.block(indent, depth)?,
                Task::Sequence(sequence) => self.sequence(sequence)?,
                Task::Mapping(mapping) => self.mapping(mapping)?,
            }
        }
        self.skip();
        if self.position < self.lines.len() {
            return Err(Diagnostic::error(
                "YAML module must contain exactly one document with consistent indentation",
                self.here(),
            ));
        }
        self.build
            .plan
            .set_root(self.result.expect("completed YAML root"));
        self.build.plan.postordered = true;
        Ok(self.build.plan)
    }
    fn block(&mut self, indent: usize, depth: usize) -> Result<(), Diagnostic> {
        self.skip();
        let Some(line) = self.lines.get(self.position).copied() else {
            return Err(Diagnostic::error("expected YAML value", self.here()));
        };
        if line.indent != indent {
            return Err(Diagnostic::error(
                "inconsistent YAML indentation",
                self.here(),
            ));
        }
        let content = self.content(self.position);
        if content == "-" || content.starts_with("- ") {
            self.build.reserve(depth, self.here())?;
            self.tasks.push(Task::Sequence(Sequence {
                indent,
                depth,
                start: line.start + indent,
                items: Vec::new(),
                waiting: false,
            }));
        } else if lines::mapping(&content).is_some() {
            self.start_mapping(indent, depth, line.start + indent, None)?;
        } else {
            self.position += 1;
            self.result = Some(self.inline(line.start + indent..line.end, depth)?);
        }
        Ok(())
    }
    fn start_mapping(
        &mut self,
        indent: usize,
        depth: usize,
        start: usize,
        first: Option<Range<usize>>,
    ) -> Result<(), Diagnostic> {
        self.build.reserve(depth, self.build.loc(start..start))?;
        self.tasks.push(Task::Mapping(Mapping {
            indent,
            depth,
            start,
            end: start,
            fields: BTreeMap::new(),
            first,
            waiting: None,
        }));
        Ok(())
    }
    fn sequence(&mut self, mut sequence: Sequence) -> Result<(), Diagnostic> {
        if sequence.waiting {
            sequence
                .items
                .push(self.result.take().expect("sequence child"));
            sequence.waiting = false;
        }
        self.skip();
        let next = self.lines.get(self.position).copied();
        let item = next
            .filter(|l| l.indent == sequence.indent)
            .and_then(|line| {
                let text = self.content(self.position);
                if text == "-" || text.starts_with("- ") {
                    Some(line)
                } else {
                    None
                }
            });
        let Some(line) = item else {
            let end = sequence.items.last().map_or(sequence.start, |id| {
                self.build.plan.node(*id).location.end as usize
            });
            self.result = Some(
                self.build
                    .plan
                    .array(sequence.items, self.build.loc(sequence.start..end)),
            );
            return Ok(());
        };
        self.build.slot(sequence.items.len(), self.here())?;
        let raw = self.text(line.start + line.indent + 1..line.end);
        let rest = lines::uncomment(&raw).trim();
        let start = line.start + line.indent + 1 + (raw.len() - raw.trim_start().len());
        let depth = sequence.depth + 1;
        self.position += 1;
        sequence.waiting = true;
        if rest.is_empty() {
            self.skip();
            if self.position == self.lines.len()
                || self.lines[self.position].indent <= sequence.indent
            {
                return Err(self
                    .build
                    .error(line.start..line.end, "YAML sequence item has no value"));
            }
            let indent = self.lines[self.position].indent;
            self.tasks.push(Task::Sequence(sequence));
            self.tasks.push(Task::Block { indent, depth });
        } else if lines::mapping(rest).is_some() {
            let indent = sequence.indent + 2;
            self.tasks.push(Task::Sequence(sequence));
            self.start_mapping(indent, depth, start, Some(start..start + rest.len()))?;
        } else {
            self.result = Some(self.inline(start..start + rest.len(), depth)?);
            self.tasks.push(Task::Sequence(sequence));
        }
        Ok(())
    }
    fn mapping(&mut self, mut mapping: Mapping) -> Result<(), Diagnostic> {
        if let Some((key, key_location)) = mapping.waiting.take() {
            let value = self.result.take().expect("mapping child");
            mapping.end = self.build.plan.node(value).location.end as usize;
            mapping.fields.insert(
                key,
                DataField {
                    key_location,
                    value,
                },
            );
        }
        self.skip();
        let range = if let Some(first) = mapping.first.take() {
            Some(first)
        } else {
            self.lines
                .get(self.position)
                .filter(|l| l.indent == mapping.indent)
                .map(|l| l.start + l.indent..l.end)
                .filter(|r| lines::mapping(&self.text(r.clone())).is_some())
        };
        let Some(range) = range else {
            self.result = Some(
                self.build
                    .plan
                    .object(mapping.fields, self.build.loc(mapping.start..mapping.end)),
            );
            return Ok(());
        };
        if self
            .lines
            .get(self.position)
            .is_some_and(|l| range.start >= l.start && range.start <= l.end)
        {
            self.position += 1;
        }
        let raw = self.text(range.clone());
        let colon = lines::mapping(&raw).expect("mapping classified");
        let key_text = raw[..colon].trim();
        let key_start = range.start + raw[..colon].len() - raw[..colon].trim_start().len();
        let key_location = self.build.loc(key_start..key_start + key_text.len());
        self.build.slot(mapping.fields.len(), key_location)?;
        let key = scalar::key(&mut self.build, key_text, key_location)?;
        if let Some(previous) = mapping.fields.get(&key) {
            return Err(
                Diagnostic::error(format!("duplicate YAML key {key:?}"), key_location)
                    .with_secondary("first defined here", previous.key_location),
            );
        }
        let tail = &raw[colon + 1..];
        let rest = lines::uncomment(tail).trim();
        let start = range.start + colon + 1 + tail.len() - tail.trim_start().len();
        let depth = mapping.depth + 1;
        mapping.waiting = Some((key, key_location));
        if rest.is_empty() {
            self.skip();
            if self.position < self.lines.len() && self.lines[self.position].indent > mapping.indent
            {
                let indent = self.lines[self.position].indent;
                self.tasks.push(Task::Mapping(mapping));
                self.tasks.push(Task::Block { indent, depth });
                return Ok(());
            }
            self.build.reserve(depth, key_location)?;
            self.result = Some(self.build.plan.scalar(DataScalar::Null, key_location));
        } else if rest.starts_with(['|', '>']) {
            self.result = Some(self.block_scalar(rest, mapping.indent, depth, start)?);
        } else {
            self.result = Some(self.inline(start..start + rest.len(), depth)?);
        }
        self.tasks.push(Task::Mapping(mapping));
        Ok(())
    }
    fn inline(&mut self, range: Range<usize>, depth: usize) -> Result<DataNodeId, Diagnostic> {
        let raw = self.text(range.clone());
        let text = lines::uncomment(&raw).trim();
        let start = range.start + raw.len() - raw.trim_start().len();
        let loc = self.build.loc(start..start + text.len());
        if text.starts_with(['[', '{']) {
            flow::Flow::parse(&mut self.build, text, start, depth)
        } else {
            self.build.reserve(depth, loc)?;
            let value = scalar::value(&mut self.build, text, loc)?;
            Ok(self.build.plan.scalar(value, loc))
        }
    }
}

#[cfg(test)]
mod tests;
