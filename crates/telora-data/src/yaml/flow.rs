use super::structure::Text;
use super::{build::Build, scalar};
use crate::{
    json::{DataField, DataNodeId},
    source::{Diagnostic, Location},
};
use alloc::vec::Vec;

enum Container {
    Array(Vec<DataNodeId>),
    Object(Vec<(Text, DataField)>, Option<(Text, Location)>),
}
struct Frame {
    start: usize,
    container: Container,
    separator: bool,
    recovered: usize,
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
                self.build
                    .slot(count + frame.recovered, self.loc(self.pos, self.pos))?;
                if self.peek() == Some(b',') {
                    let loc = self.loc(self.pos, self.pos + 1);
                    self.build.reserve(self.depth + self.frames.len(), loc)?;
                    self.build
                        .plan
                        .diagnostics
                        .push(Diagnostic::error("unexpected YAML flow comma", loc));
                    self.frames.last_mut().unwrap().recovered += 1;
                    self.pos += 1;
                    continue;
                }
                if matches!(frame.container, Container::Object(..)) {
                    let start = self.pos;
                    let text = self.scalar_text(&[b':', b',', b'}'])?;
                    let loc = self.loc(start, self.pos - (text.len() - text.trim_end().len()));
                    let key = scalar::key(self.build, text.trim(), loc)?;
                    self.expect(b':')?;
                    let Container::Object(_, pending) =
                        &mut self.frames.last_mut().unwrap().container
                    else {
                        unreachable!()
                    };
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
                        Container::Object(Vec::new(), None)
                    };
                    self.pos += 1;
                    self.frames.push(Frame {
                        start,
                        container,
                        separator: false,
                        recovered: 0,
                    });
                }
                _ => {
                    let raw = self.scalar_text(&[b',', b']', b'}'])?;
                    let loc = self.loc(start, self.pos - (raw.len() - raw.trim_end().len()));
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
                    fields.push((
                        key,
                        DataField {
                            key_location,
                            value: id,
                        },
                    ));
                }
            }
            frame.separator = true;
        } else {
            self.root = Some(id);
        }
    }
    fn scalar_text(&mut self, stops: &[u8]) -> Result<&'a str, Diagnostic> {
        let start = self.pos;
        self.pos += super::lexer::scalar_end(&self.text[start..], stops);
        if start == self.pos {
            Err(self.error("expected YAML flow scalar"))
        } else {
            Ok(&self.text[start..self.pos])
        }
    }
}
