use super::{build::Build, scalar};
use crate::{
    json::{DataField, DataNodeId},
    source::{Diagnostic, Location},
};
use alloc::{collections::BTreeMap, string::String, vec::Vec};

enum Container {
    Array(Vec<DataNodeId>),
    Object(BTreeMap<String, DataField>, Option<(String, Location)>),
}
struct Frame {
    start: usize,
    container: Container,
    separator: bool,
}

pub(super) struct Flow<'a, 'b> {
    build: &'b mut Build,
    text: &'a str,
    offset: usize,
    pos: usize,
    depth: usize,
    frames: Vec<Frame>,
    root: Option<DataNodeId>,
}

impl<'a, 'b> Flow<'a, 'b> {
    pub fn parse(
        build: &'b mut Build,
        text: &'a str,
        offset: usize,
        depth: usize,
    ) -> Result<DataNodeId, Diagnostic> {
        Self {
            build,
            text,
            offset,
            pos: 0,
            depth,
            frames: Vec::new(),
            root: None,
        }
        .run()
    }
    fn loc(&self, start: usize, end: usize) -> Location {
        self.build.loc(self.offset + start..self.offset + end)
    }
    fn error(&self, message: &str) -> Diagnostic {
        Diagnostic::error(message, self.loc(self.pos, self.pos))
    }
    fn ws(&mut self) {
        self.pos += self.text[self.pos..].len() - self.text[self.pos..].trim_start().len();
    }
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }
    fn expect(&mut self, byte: u8) -> Result<(), Diagnostic> {
        self.ws();
        if self.peek() != Some(byte) {
            return Err(self.error("unexpected YAML flow delimiter"));
        }
        self.pos += 1;
        Ok(())
    }
    fn run(mut self) -> Result<DataNodeId, Diagnostic> {
        loop {
            self.ws();
            if self.frames.is_empty()
                && let Some(root) = self.root
            {
                if self.pos != self.text.len() {
                    return Err(self.error("unexpected YAML flow content"));
                }
                return Ok(root);
            }
            if let Some(frame) = self.frames.last() {
                let closer = match frame.container {
                    Container::Array(_) => b']',
                    Container::Object(..) => b'}',
                };
                if self.peek() == Some(closer) {
                    self.pos += 1;
                    let frame = self.frames.pop().unwrap();
                    let loc = self.loc(frame.start, self.pos);
                    let id = match frame.container {
                        Container::Array(items) => self.build.plan.array(items, loc),
                        Container::Object(fields, _) => self.build.plan.object(fields, loc),
                    };
                    self.attach(id);
                    continue;
                }
                if frame.separator {
                    self.expect(b',')?;
                    self.frames.last_mut().unwrap().separator = false;
                    continue;
                }
                let count = match &frame.container {
                    Container::Array(items) => items.len(),
                    Container::Object(fields, _) => fields.len(),
                };
                self.build.slot(count, self.loc(self.pos, self.pos))?;
                if matches!(frame.container, Container::Object(..)) {
                    let start = self.pos;
                    let text = self.scalar_text(&[b':', b',', b'}'])?;
                    let loc = self.loc(start, self.pos);
                    let key = scalar::key(self.build, text.trim(), loc)?;
                    self.expect(b':')?;
                    let Container::Object(fields, pending) =
                        &mut self.frames.last_mut().unwrap().container
                    else {
                        unreachable!()
                    };
                    if let Some(previous) = fields.get(&key) {
                        return Err(
                            Diagnostic::error(format!("duplicate YAML key {key:?}"), loc)
                                .with_secondary("first defined here", previous.key_location),
                        );
                    }
                    *pending = Some((key, loc));
                    self.ws();
                }
            }
            let start = self.pos;
            self.build
                .reserve(self.depth + self.frames.len(), self.loc(start, start))?;
            match self.peek() {
                Some(b'[' | b'{') => {
                    let container = if self.peek() == Some(b'[') {
                        Container::Array(Vec::new())
                    } else {
                        Container::Object(BTreeMap::new(), None)
                    };
                    self.pos += 1;
                    self.frames.push(Frame {
                        start,
                        container,
                        separator: false,
                    });
                }
                _ => {
                    let raw = self.scalar_text(&[b',', b']', b'}'])?;
                    let loc = self.loc(start, self.pos);
                    let value = scalar::value(self.build, raw.trim(), loc)?;
                    let id = self.build.plan.scalar(value, loc);
                    self.attach(id);
                }
            }
        }
    }
    fn attach(&mut self, id: DataNodeId) {
        if let Some(frame) = self.frames.last_mut() {
            match &mut frame.container {
                Container::Array(items) => items.push(id),
                Container::Object(fields, key) => {
                    let (key, key_location) = key.take().unwrap();
                    fields.insert(
                        key,
                        DataField {
                            key_location,
                            value: id,
                        },
                    );
                }
            }
            frame.separator = true;
        } else {
            self.root = Some(id);
        }
    }
    fn scalar_text(&mut self, stops: &[u8]) -> Result<&'a str, Diagnostic> {
        let start = self.pos;
        let mut quote = None;
        while let Some(byte) = self.peek() {
            if let Some(q) = quote {
                self.pos += 1;
                if q == b'"' && byte == b'\\' {
                    if let Some(ch) = self.text[self.pos..].chars().next() {
                        self.pos += ch.len_utf8();
                    }
                } else if byte == q {
                    if q == b'\'' && self.peek() == Some(b'\'') {
                        self.pos += 1;
                    } else {
                        quote = None;
                    }
                }
            } else if matches!(byte, b'\'' | b'"') {
                quote = Some(byte);
                self.pos += 1;
            } else if stops.contains(&byte) {
                break;
            } else {
                self.pos += 1;
            }
        }
        if start == self.pos {
            Err(self.error("expected YAML flow scalar"))
        } else {
            Ok(&self.text[start..self.pos])
        }
    }
}
