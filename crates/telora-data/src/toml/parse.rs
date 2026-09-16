use super::structure::{Assignment, Header, Key, Kind, Plan, Scalar as DataScalar, Table, Text};
use super::{admission::Admission, input::Input};
use crate::{
    DataLimits,
    source::{Diagnostic, Location, SourceId},
};
use alloc::vec::Vec;

pub(super) fn parse(source: SourceId, text: &str, limits: DataLimits) -> Result<Plan, Diagnostic> {
    let build = Admission::new(limits);
    build.check(
        Location::from_usize(source, 0..0).unwrap(),
        "file_size",
        text.len(),
        limits.file_size,
    )?;
    Parser {
        input: Input::new(source, text),
        build,
        plan: Plan {
            tables: vec![Table {
                header: None,
                items: Vec::new(),
            }],
            ..Plan::default()
        },
        missing: 0,
        statements: 0,
    }
    .document()
}
struct Parser<'a> {
    input: Input<'a>,
    build: Admission,
    plan: Plan,
    missing: usize,
    statements: usize,
}
enum Task {
    Value(usize),
    Array {
        id: usize,
        start: usize,
        depth: usize,
        after: bool,
        slots: usize,
    },
    Inline {
        id: usize,
        start: usize,
        depth: usize,
        after: bool,
        allow_end: bool,
        slots: usize,
    },
    Push(usize),
    Field {
        id: usize,
        path: Vec<usize>,
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
    fn document(mut self) -> Result<Plan, Diagnostic> {
        loop {
            self.input.space(true);
            if self.input.peek().is_none() {
                return Ok(self.plan);
            }
            let start = self.input.offset;
            self.build.check(
                self.input.loc(start),
                "nodes",
                self.statements + 1,
                self.build.limits.nodes,
            )?;
            self.statements += 1;
            if self.input.eat(b'[') {
                let array = self.input.eat(b'[');
                let path = self.key()?;
                self.expect(b']')?;
                if array {
                    self.expect(b']')?;
                }
                self.plan.tables.push(Table {
                    header: Some(Header {
                        path,
                        array,
                        location: self.input.loc(start),
                    }),
                    items: Vec::new(),
                });
            } else {
                let path = self.assignment()?;
                let value = self.value(2)?;
                self.plan
                    .tables
                    .last_mut()
                    .unwrap()
                    .items
                    .push(Assignment { path, value });
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
    }
    fn key(&mut self) -> Result<Vec<usize>, Diagnostic> {
        let mut path = Vec::new();
        loop {
            self.input.space(false);
            let start = self.input.offset;
            let text = match self.input.peek() {
                Some(b'"' | b'\'') => self
                    .input
                    .string(false, |_, len, loc| self.build.string_size(len, loc, false))?,
                _ => {
                    while self
                        .input
                        .peek()
                        .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                    {
                        let n = self.input.key_run();
                        self.build.string_size(
                            self.input.offset - start + n,
                            self.input.loc(start),
                            false,
                        )?;
                        self.input.take(n);
                    }
                    if self.input.offset == start {
                        return Err(self.input.error(start, "expected TOML key"));
                    }
                    Text {
                        range: start..self.input.offset,
                        decoded_len: self.input.offset - start,
                        escaped: false,
                    }
                }
            };
            self.build.check(
                self.input.loc(start),
                "depth",
                path.len() + 2,
                self.build.limits.depth,
            )?;
            path.push(self.plan.keys.len());
            self.plan.keys.push(Key {
                text,
                location: self.input.loc(start),
            });
            self.input.space(false);
            if !self.input.eat(b'.') {
                return Ok(path);
            }
        }
    }
    fn assignment(&mut self) -> Result<Vec<usize>, Diagnostic> {
        let path = self.key()?;
        self.expect(b'=')?;
        self.input.space(false);
        Ok(path)
    }
    fn atom_part(&mut self, start: usize, n: usize) -> Result<(), Diagnostic> {
        self.input.take(n);
        let text = self.input.slice(start);
        if super::scalar::is_temporal(text) {
            self.build
                .string_size(text.len().saturating_sub(5), self.input.loc(start), true)?;
        }
        Ok(())
    }
    fn atom(&mut self) -> Result<DataScalar, Diagnostic> {
        let start = self.input.offset;
        loop {
            let n = self.input.atom_run();
            if n == 0 {
                break;
            }
            self.atom_part(start, n)?;
        }
        let text = self.input.slice(start);
        if text.len() == 10
            && text.as_bytes()[4] == b'-'
            && text.as_bytes()[7] == b'-'
            && self.input.peek() == Some(b' ')
            && self.input.nth(1).is_some_and(|b| b.is_ascii_digit())
        {
            self.input.take(1);
            loop {
                let n = self.input.atom_run();
                if n == 0 {
                    break;
                }
                self.atom_part(start, n)?;
            }
        }
        let text = self.input.slice(start);
        let loc = self.input.loc(start);
        match text {
            "true" => Ok(DataScalar::Bool(true)),
            "false" => Ok(DataScalar::Bool(false)),
            "" => Err(self.input.error(start, "expected TOML value")),
            _ => {
                if super::scalar::is_temporal(text) {
                    let len = text.len()
                        - if text.ends_with("+00:00") || text.ends_with("-00:00") {
                            5
                        } else {
                            0
                        };
                    self.build.string_size(len, loc, true)?;
                    self.build.payload(len, loc)?;
                }
                Ok(DataScalar::Atom(loc.range()))
            }
        }
    }
    fn value(&mut self, depth: usize) -> Result<usize, Diagnostic> {
        let mut tasks = vec![Task::Value(depth)];
        let mut result = None;
        while let Some(task) = tasks.pop() {
            match task {
                Task::Value(depth) => {
                    let start = self.input.offset;
                    let loc = self.input.loc(start);
                    self.build
                        .check(loc, "depth", depth, self.build.limits.depth)?;
                    self.build.check(
                        loc,
                        "nodes",
                        self.plan.nodes.len() + self.missing + 1,
                        self.build.limits.nodes,
                    )?;
                    match self.input.peek() {
                        Some(b'[') => {
                            self.input.take(1);
                            let id = self.plan.push(Kind::Array(Vec::new()), loc);
                            tasks.push(Task::Array {
                                id,
                                start,
                                depth,
                                after: false,
                                slots: 0,
                            });
                        }
                        Some(b'{') => {
                            self.input.take(1);
                            let id = self.plan.push(Kind::Inline(Vec::new()), loc);
                            tasks.push(Task::Inline {
                                id,
                                start,
                                depth,
                                after: false,
                                allow_end: true,
                                slots: 0,
                            });
                        }
                        _ => {
                            let scalar = if matches!(self.input.peek(), Some(b'"' | b'\'')) {
                                let text = self.input.string(true, |_, len, loc| {
                                    self.build.string_size(len, loc, true)
                                })?;
                                self.build
                                    .payload(text.decoded_len, self.input.loc(start))?;
                                DataScalar::String(text)
                            } else {
                                self.atom()?
                            };
                            result =
                                Some(self.plan.push(Kind::Scalar(scalar), self.input.loc(start)));
                        }
                    }
                }
                Task::Array {
                    id,
                    start,
                    depth,
                    after,
                    slots,
                } => {
                    self.input.space(true);
                    if self.input.eat(b']') {
                        self.plan.nodes[id].location = self.input.loc(start);
                        result = Some(id);
                        continue;
                    }
                    if after {
                        self.expect(b',')?;
                        tasks.push(Task::Array {
                            id,
                            start,
                            depth,
                            after: false,
                            slots,
                        });
                    } else {
                        self.slot(slots)?;
                        if self.extra_comma(depth + 1)? {
                            tasks.push(Task::Array {
                                id,
                                start,
                                depth,
                                after: false,
                                slots: slots + 1,
                            });
                            continue;
                        }
                        tasks.push(Task::Array {
                            id,
                            start,
                            depth,
                            after: true,
                            slots: slots + 1,
                        });
                        tasks.push(Task::Push(id));
                        tasks.push(Task::Value(depth + 1));
                    }
                }
                Task::Inline {
                    id,
                    start,
                    depth,
                    after,
                    allow_end,
                    slots,
                } => {
                    self.input.space(false);
                    if self.input.eat(b'}') {
                        if !allow_end {
                            self.plan.diagnostics.push(self.input.error(
                                self.input.offset - 1,
                                "trailing comma in TOML inline table",
                            ));
                        }
                        self.plan.nodes[id].location = self.input.loc(start);
                        result = Some(id);
                        continue;
                    }
                    if after {
                        self.expect(b',')?;
                        tasks.push(Task::Inline {
                            id,
                            start,
                            depth,
                            after: false,
                            allow_end: false,
                            slots,
                        });
                    } else {
                        self.slot(slots)?;
                        if self.extra_comma(depth + 1)? {
                            tasks.push(Task::Inline {
                                id,
                                start,
                                depth,
                                after: false,
                                allow_end: true,
                                slots: slots + 1,
                            });
                            continue;
                        }
                        let path = self.assignment()?;
                        tasks.push(Task::Inline {
                            id,
                            start,
                            depth,
                            after: true,
                            allow_end: true,
                            slots: slots + 1,
                        });
                        tasks.push(Task::Field { id, path });
                        tasks.push(Task::Value(depth + 1));
                    }
                }
                Task::Push(id) => {
                    let Kind::Array(items) = &mut self.plan.nodes[id].kind else {
                        unreachable!()
                    };
                    items.push(result.take().unwrap());
                }
                Task::Field { id, path } => {
                    let Kind::Inline(fields) = &mut self.plan.nodes[id].kind else {
                        unreachable!()
                    };
                    fields.push(Assignment {
                        path,
                        value: result.take().unwrap(),
                    });
                }
            }
        }
        Ok(result.expect("completed syntax value"))
    }
    fn slot(&self, count: usize) -> Result<(), Diagnostic> {
        self.build.check(
            self.input.loc(self.input.offset),
            "container_size",
            count + 1,
            self.build.limits.container_size,
        )
    }
    fn extra_comma(&mut self, depth: usize) -> Result<bool, Diagnostic> {
        let start = self.input.offset;
        if self.input.peek() != Some(b',') {
            return Ok(false);
        }
        self.build.check(
            self.input.loc(start),
            "nodes",
            self.plan.nodes.len() + self.missing + 1,
            self.build.limits.nodes,
        )?;
        self.build.check(
            self.input.loc(start),
            "depth",
            depth,
            self.build.limits.depth,
        )?;
        self.missing += 1;
        self.input.take(1);
        self.plan
            .diagnostics
            .push(self.input.error(start, "unexpected TOML comma"));
        Ok(true)
    }
}
