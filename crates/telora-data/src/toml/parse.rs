use super::{build::Build, input::Input, scalar};
use crate::{
    DataLimits,
    json::{DataNodeId, DataPlanNodeKind, DataScalar, ValidatedDataPlan},
    source::{Diagnostic, Location, SourceDatabase, SourceId},
};
use alloc::{string::String, vec::Vec};

pub(super) fn parse(
    sources: &SourceDatabase,
    source: SourceId,
    limits: DataLimits,
) -> Result<ValidatedDataPlan, Diagnostic> {
    let text = sources.get(source).text();
    parse_chunks(source, text.chunks(), text.byte_len(), limits)
}

pub(super) fn parse_chunks<'a>(
    source: SourceId,
    chunks: impl Iterator<Item = &'a str>,
    size: usize,
    limits: DataLimits,
) -> Result<ValidatedDataPlan, Diagnostic> {
    let build = Build::new(limits);
    build.check(
        Location::from_usize(source, 0..0).expect("source start"),
        "file_size",
        size,
        limits.file_size,
    )?;
    Parser {
        input: Input::new(source, chunks),
        build,
    }
    .document(size)
}

struct Parser<'a> {
    input: Input<'a>,
    build: Build,
}

enum Task {
    Value(usize),
    Array {
        id: DataNodeId,
        start: usize,
        after: bool,
    },
    Inline {
        id: DataNodeId,
        start: usize,
        after: bool,
        allow_end: bool,
    },
    Push(DataNodeId),
    Field {
        id: DataNodeId,
        key: String,
        loc: Location,
    },
}

impl Parser<'_> {
    fn expect(&mut self, byte: u8) -> Result<(), Diagnostic> {
        if self.input.eat(byte) {
            Ok(())
        } else {
            Err(self.input.error(
                self.input.offset,
                format!("expected '{}' in TOML", byte as char),
            ))
        }
    }
    fn document(mut self, size: usize) -> Result<ValidatedDataPlan, Diagnostic> {
        let start = self.input.loc(0);
        self.build
            .check(start, "file_size", size, self.build.limits.file_size)?;
        let root = self.build.table(1, start, true)?;
        let mut current = root;
        loop {
            self.input.space(true);
            if self.input.peek().is_none() {
                break;
            }
            let start = self.input.offset;
            if self.input.eat(b'[') {
                let array = self.input.eat(b'[');
                let key = self.key()?;
                self.expect(b']')?;
                if array {
                    self.expect(b']')?;
                }
                current = self.build.header(root, key, array, self.input.loc(start))?;
            } else {
                let (table, key, loc) = self.assignment(current)?;
                let value = self.value(self.build.depth(table) + 1)?;
                self.build.insert(table, key, loc, value);
            }
            self.input.space(false);
            if self.input.peek() == Some(b'#') {
                self.input.comment();
            }
            if self.input.peek().is_some() && !self.input.newline() {
                return Err(self
                    .input
                    .error(self.input.offset, "expected end of TOML statement"));
            }
        }
        self.build.plan.node_mut(root).location = self.input.loc(0);
        self.build.plan.set_root(root);
        Ok(self.build.plan.into_postorder())
    }
    fn key(&mut self) -> Result<Vec<(String, Location)>, Diagnostic> {
        let mut path = Vec::new();
        loop {
            self.input.space(false);
            let start = self.input.offset;
            let key = match self.input.peek() {
                Some(b'"' | b'\'') => self.input.string(&self.build, false)?,
                _ => {
                    let mut key = String::new();
                    while self
                        .input
                        .peek()
                        .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                    {
                        let n = self.input.key_run();
                        self.build
                            .string_size(key.len() + n, self.input.loc(start), false)?;
                        key.push_str(self.input.take(n));
                    }
                    if key.is_empty() {
                        return Err(self.input.error(start, "expected TOML key"));
                    }
                    key
                }
            };
            self.build.check(
                self.input.loc(start),
                "depth",
                path.len() + 2,
                self.build.limits.depth,
            )?;
            path.push((key, self.input.loc(start)));
            self.input.space(false);
            if !self.input.eat(b'.') {
                return Ok(path);
            }
        }
    }
    fn assignment(&mut self, id: DataNodeId) -> Result<(DataNodeId, String, Location), Diagnostic> {
        let mut path = self.key()?;
        self.expect(b'=')?;
        self.input.space(false);
        let (key, loc) = path.pop().expect("nonempty key");
        let table = self.build.path(id, path, true)?;
        self.build.field_slot(table, &key, loc)?;
        Ok((table, key, loc))
    }
    fn atom_part(&mut self, text: &mut String, n: usize, start: usize) -> Result<(), Diagnostic> {
        let part = &self.input.rest()[..n];
        let mut prefix = [0u8; 10];
        for (to, from) in prefix.iter_mut().zip(text.bytes().chain(part.bytes())) {
            *to = from;
        }
        if (prefix[4] == b'-' && prefix[7] == b'-') || (prefix[2] == b':' && prefix[5] == b':') {
            // UTC offset canonicalization can remove five bytes. Keep at most
            // that bounded uncertainty until the complete suffix is available.
            self.build.string_size(
                (text.len() + n).saturating_sub(5),
                self.input.loc(start),
                true,
            )?;
        }
        text.push_str(self.input.take(n));
        Ok(())
    }
    fn atom(&mut self) -> Result<DataScalar, Diagnostic> {
        let start = self.input.offset;
        let mut text = String::new();
        loop {
            let n = self.input.atom_run();
            if n == 0 {
                break;
            }
            self.atom_part(&mut text, n, start)?;
        }
        if text.len() == 10
            && text.as_bytes()[4] == b'-'
            && text.as_bytes()[7] == b'-'
            && self.input.peek() == Some(b' ')
            && self.input.nth(1).is_some_and(|b| b.is_ascii_digit())
        {
            text.push_str(self.input.take(1));
            loop {
                let n = self.input.atom_run();
                if n == 0 {
                    break;
                }
                self.atom_part(&mut text, n, start)?;
            }
        }
        let loc = self.input.loc(start);
        match text.as_str() {
            "true" => Ok(DataScalar::Bool(true)),
            "false" => Ok(DataScalar::Bool(false)),
            "" => Err(self.input.error(start, "expected TOML value")),
            _ => {
                let is_temporal =
                    (text.len() >= 10 && text.as_bytes()[4] == b'-' && text.as_bytes()[7] == b'-')
                        || (text.len() >= 8
                            && text.as_bytes()[2] == b':'
                            && text.as_bytes()[5] == b':');
                if is_temporal {
                    let len = text.len()
                        - if text.ends_with("+00:00") || text.ends_with("-00:00") {
                            5
                        } else {
                            0
                        };
                    self.build.string_size(len, loc, true)?;
                }
                if let Some(result) = scalar::parse_temporal(&text) {
                    let (kind, value) = result.map_err(|m| Diagnostic::error(m, loc))?;
                    self.build.string_size(value.len(), loc, true)?;
                    self.build.payload(value.len(), loc)?;
                    Ok(DataScalar::Temporal { kind, value })
                } else {
                    scalar::parse_number(&text).map_err(|m| Diagnostic::error(m, loc))
                }
            }
        }
    }
    fn value(&mut self, depth: usize) -> Result<DataNodeId, Diagnostic> {
        let mut tasks = vec![Task::Value(depth)];
        let mut result = None;
        while let Some(task) = tasks.pop() {
            match task {
                Task::Value(depth) => {
                    let start = self.input.offset;
                    let loc = self.input.loc(start);
                    self.build.reserve(depth, loc)?;
                    match self.input.peek() {
                        Some(b'[') => {
                            self.input.take(1);
                            let id =
                                self.build
                                    .node(DataPlanNodeKind::Array(Vec::new()), depth, loc)?;
                            tasks.push(Task::Array {
                                id,
                                start,
                                after: false,
                            });
                        }
                        Some(b'{') => {
                            self.input.take(1);
                            let id = self.build.table(depth, loc, true)?;
                            tasks.push(Task::Inline {
                                id,
                                start,
                                after: false,
                                allow_end: true,
                            });
                        }
                        _ => {
                            let scalar = if matches!(self.input.peek(), Some(b'"' | b'\'')) {
                                let text = self.input.string(&self.build, true)?;
                                self.build.payload(text.len(), self.input.loc(start))?;
                                DataScalar::String(text)
                            } else {
                                self.atom()?
                            };
                            result = Some(self.build.node(
                                DataPlanNodeKind::Scalar(scalar),
                                depth,
                                self.input.loc(start),
                            )?);
                        }
                    }
                }
                Task::Array { id, start, after } => {
                    self.input.space(true);
                    if self.input.eat(b']') {
                        self.build.plan.node_mut(id).location = self.input.loc(start);
                        result = Some(id);
                        continue;
                    }
                    if after {
                        self.expect(b',')?;
                        tasks.push(Task::Array {
                            id,
                            start,
                            after: false,
                        });
                    } else {
                        self.build.array_slot(id, self.input.loc(start))?;
                        tasks.push(Task::Array {
                            id,
                            start,
                            after: true,
                        });
                        tasks.push(Task::Push(id));
                        tasks.push(Task::Value(self.build.depth(id) + 1));
                    }
                }
                Task::Inline {
                    id,
                    start,
                    after,
                    allow_end,
                } => {
                    self.input.space(false);
                    if allow_end && self.input.eat(b'}') {
                        self.build.plan.node_mut(id).location = self.input.loc(start);
                        self.build.meta[id.index()].sealed = true;
                        result = Some(id);
                        continue;
                    }
                    if after {
                        self.expect(b',')?;
                        tasks.push(Task::Inline {
                            id,
                            start,
                            after: false,
                            allow_end: false,
                        });
                    } else {
                        let (table, key, loc) = self.assignment(id)?;
                        tasks.push(Task::Inline {
                            id,
                            start,
                            after: true,
                            allow_end: true,
                        });
                        tasks.push(Task::Field {
                            id: table,
                            key,
                            loc,
                        });
                        tasks.push(Task::Value(self.build.depth(table) + 1));
                    }
                }
                Task::Push(id) => self.build.push(id, result.take().expect("completed child")),
                Task::Field { id, key, loc } => {
                    self.build
                        .insert(id, key, loc, result.take().expect("completed child"))
                }
            }
        }
        Ok(result.expect("completed value"))
    }
}
