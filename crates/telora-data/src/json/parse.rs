//! A bounded, nonrecursive JSON machine. A completed container is attached to
//! its parent by ID; all output nodes are already in child-before-parent order.
use super::lexer::{Kind, Scanner, Token};
use super::structure::{RawKind, RawPlan, StringSpan};
use super::{DataField, DataNodeId};
use crate::{
    DataLimits,
    source::{Diagnostic, Location, SourceId},
};
use alloc::{string::String, vec::Vec};
use core::ops::Range;

#[derive(Clone, Copy)]
enum Expect {
    Value,
    ValueOrEnd,
    Key,
    KeyOrEnd,
    Colon,
    Separator,
}

enum Container {
    Array(Vec<DataNodeId>),
    Object {
        fields: Vec<(StringSpan, DataField)>,
        key: Option<(StringSpan, Location)>,
    },
}

struct Frame {
    start: usize,
    expect: Expect,
    container: Container,
    separator: Option<Range<usize>>,
    skipped: usize,
}

pub(super) struct Parser<'a, I> {
    source: SourceId,
    scanner: Scanner<'a, I>,
    frames: Vec<Frame>,
    plan: RawPlan,
    root: Option<DataNodeId>,
    limits: DataLimits,
    nodes: usize,
    payload: usize,
}

impl<'a, I: Iterator<Item = &'a str>> Parser<'a, I> {
    pub fn new(source: SourceId, chunks: I, limits: DataLimits) -> Self {
        Self {
            source,
            scanner: Scanner::new(chunks),
            frames: Vec::new(),
            plan: RawPlan::default(),
            root: None,
            limits,
            nodes: 0,
            payload: 0,
        }
    }

    fn location(&self, span: Range<usize>) -> Location {
        Location::from_usize(self.source, span).expect("registered source span")
    }
    fn error(&self, span: Range<usize>, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.location(span))
    }
    fn next(&mut self) -> Result<Token<'a>, Diagnostic> {
        self.scanner
            .next()
            .map_err(|e| self.error(e.span, e.message))
    }
    fn limit(
        &self,
        span: Range<usize>,
        name: &str,
        value: usize,
        limit: usize,
    ) -> Result<(), Diagnostic> {
        if value > limit {
            Err(self.error(
                span,
                format!("data source exceeds {name} limit ({value} > {limit})"),
            ))
        } else {
            Ok(())
        }
    }
    fn container_slot(&self, span: Range<usize>) -> Result<(), Diagnostic> {
        if let Some(frame) = self.frames.last() {
            let len = match &frame.container {
                Container::Array(items) => items.len(),
                Container::Object { fields, .. } => fields.len(),
            };
            self.limit(
                span,
                "container_size",
                len + frame.skipped + 1,
                self.limits.container_size,
            )?;
        }
        Ok(())
    }
    fn reserve_value(&mut self, span: Range<usize>) -> Result<(), Diagnostic> {
        self.container_slot(span.clone())?;
        self.limit(
            span.clone(),
            "depth",
            self.frames.len() + 1,
            self.limits.depth,
        )?;
        self.limit(span, "nodes", self.nodes + 1, self.limits.nodes)?;
        self.nodes += 1;
        Ok(())
    }

    pub fn parse(mut self, bytes: usize) -> Result<RawPlan, Diagnostic> {
        self.limit(0..bytes, "file_size", bytes, self.limits.file_size)?;
        loop {
            let token = self.next()?;
            if self.frames.is_empty() && self.root.is_some() {
                if !matches!(token.kind, Kind::Eof) {
                    return Err(self.error(token.span, "expected end of JSON input"));
                }
                self.plan.root = self.root;
                return Ok(self.plan);
            }
            let expected = self.frames.last().map_or(Expect::Value, |f| f.expect);
            match expected {
                Expect::Value | Expect::ValueOrEnd => {
                    if matches!(token.kind, Kind::Comma) && !self.frames.is_empty() {
                        self.skip_comma(token.span)?;
                        continue;
                    }
                    if matches!(token.kind, Kind::RBracket)
                        && self
                            .frames
                            .last()
                            .is_some_and(|frame| matches!(frame.container, Container::Array(_)))
                    {
                        if matches!(expected, Expect::Value) {
                            self.trailing_comma();
                        }
                        self.close(token.span)?;
                    } else {
                        self.value(token)?;
                    }
                }
                Expect::Key | Expect::KeyOrEnd => {
                    if matches!(token.kind, Kind::Comma) {
                        self.skip_comma(token.span)?;
                        continue;
                    }
                    if matches!(token.kind, Kind::RBrace) {
                        if matches!(expected, Expect::Key) {
                            self.trailing_comma();
                        }
                        self.close(token.span)?;
                        continue;
                    }
                    if !matches!(token.kind, Kind::QStart) {
                        return Err(self.error(token.span, "JSON object key must be a string"));
                    }
                    self.container_slot(token.span.clone())?;
                    let (key, location) = self.string(token.span.start)?;
                    let frame = self.frames.last_mut().unwrap();
                    let Container::Object { key: pending, .. } = &mut frame.container else {
                        unreachable!()
                    };
                    *pending = Some((key, location));
                    frame.expect = Expect::Colon;
                }
                Expect::Colon => {
                    if !matches!(token.kind, Kind::Colon) {
                        return Err(self.error(token.span, "expected ':' after JSON object key"));
                    }
                    self.frames.last_mut().unwrap().expect = Expect::Value;
                }
                Expect::Separator => {
                    if matches!(token.kind, Kind::Comma) {
                        let frame = self.frames.last_mut().unwrap();
                        frame.separator = Some(token.span);
                        frame.expect = match frame.container {
                            Container::Array(_) => Expect::Value,
                            Container::Object { .. } => Expect::Key,
                        };
                    } else {
                        let valid = match &self.frames.last().unwrap().container {
                            Container::Array(_) => matches!(token.kind, Kind::RBracket),
                            Container::Object { .. } => matches!(token.kind, Kind::RBrace),
                        };
                        if !valid {
                            return Err(
                                self.error(token.span, "expected ',' or closing JSON delimiter")
                            );
                        }
                        self.close(token.span)?;
                    }
                }
            }
        }
    }

    #[cold]
    fn skip_comma(&mut self, span: Range<usize>) -> Result<(), Diagnostic> {
        // Recovery slots consume structural budget too: an invalid comma-only
        // container must not bypass admission and allocate unlimited diagnostics.
        self.reserve_value(span.clone())?;
        self.frames.last_mut().unwrap().skipped += 1;
        self.plan
            .diagnostics
            .push(self.error(span, "unexpected extra JSON comma"));
        Ok(())
    }

    #[cold]
    fn trailing_comma(&mut self) {
        let span = self
            .frames
            .last()
            .unwrap()
            .separator
            .clone()
            .expect("separator");
        self.plan
            .diagnostics
            .push(self.error(span, "trailing JSON comma is not allowed"));
    }

    fn value(&mut self, token: Token<'a>) -> Result<(), Diagnostic> {
        let scalar = match token.kind {
            Kind::LBracket | Kind::LBrace => {
                self.reserve_value(token.span.clone())?;
                let (container, expect) = if matches!(token.kind, Kind::LBracket) {
                    (Container::Array(Vec::new()), Expect::ValueOrEnd)
                } else {
                    (
                        Container::Object {
                            fields: Vec::new(),
                            key: None,
                        },
                        Expect::KeyOrEnd,
                    )
                };
                self.frames.push(Frame {
                    start: token.span.start,
                    expect,
                    container,
                    separator: None,
                    skipped: 0,
                });
                return Ok(());
            }
            Kind::QStart => {
                self.reserve_value(token.span.clone())?;
                let (text, location) = self.string(token.span.start)?;
                let id = self.plan.push(RawKind::String(text), location);
                self.attach(id);
                return Ok(());
            }
            Kind::Null => RawKind::Null,
            Kind::True => RawKind::Bool(true),
            Kind::False => RawKind::Bool(false),
            Kind::Number { float } => RawKind::Number {
                range: token.span.clone(),
                float,
            },
            _ => return Err(self.error(token.span, "expected a JSON value")),
        };
        self.reserve_value(token.span.clone())?;
        let id = self.plan.push(scalar, self.location(token.span));
        self.attach(id);
        Ok(())
    }

    fn close(&mut self, span: Range<usize>) -> Result<(), Diagnostic> {
        let frame = self.frames.pop().unwrap();
        let location = self.location(frame.start..span.end);
        let id = match frame.container {
            Container::Array(items) => self.plan.push(RawKind::Array(items), location),
            Container::Object { fields, .. } => self.plan.push(RawKind::Object(fields), location),
        };
        self.attach(id);
        Ok(())
    }
    fn attach(&mut self, id: DataNodeId) {
        if let Some(frame) = self.frames.last_mut() {
            match &mut frame.container {
                Container::Array(items) => items.push(id),
                Container::Object { fields, key } => {
                    let (name, key_location) = key.take().unwrap();
                    fields.push((
                        name,
                        DataField {
                            key_location,
                            value: id,
                        },
                    ));
                }
            }
            frame.expect = Expect::Separator;
        } else {
            self.root = Some(id);
        }
    }

    fn append(
        &mut self,
        output: &mut usize,
        text: &str,
        span: Range<usize>,
    ) -> Result<(), Diagnostic> {
        // Registered input fits u32 and each decoded byte is admitted once.
        let length = output
            .checked_add(text.len())
            .ok_or_else(|| self.error(span.clone(), "JSON string length overflow"))?;
        let payload = self
            .payload
            .checked_add(text.len())
            .ok_or_else(|| self.error(span.clone(), "JSON payload accounting overflow"))?;
        self.limit(span.clone(), "string_len", length, self.limits.string_len)?;
        self.limit(span, "payloads_bytes", payload, self.limits.payloads_bytes)?;
        self.payload = payload;
        *output = length;
        Ok(())
    }

    fn string(&mut self, start: usize) -> Result<(StringSpan, Location), Diagnostic> {
        let mut output = 0;
        let mut escaped = false;
        let mut high: Option<(u16, Range<usize>)> = None;
        loop {
            let token = self.next()?;
            let mut utf8 = [0; 4];
            escaped |= matches!(token.kind, Kind::EscChar(_) | Kind::EscUtf16(_));
            if let Some((first, origin)) = high.take() {
                let Kind::EscUtf16(second @ 0xdc00..=0xdfff) = token.kind else {
                    return Err(self.error(origin, "high surrogate requires a low surrogate"));
                };
                let ch = char::from_u32(
                    0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00,
                )
                .unwrap();
                self.append(
                    &mut output,
                    ch.encode_utf8(&mut utf8),
                    origin.start..token.span.end,
                )?;
                continue;
            }
            let text = match token.kind {
                Kind::QEnd => {
                    return Ok((
                        StringSpan {
                            range: start + 1..token.span.start,
                            escaped,
                        },
                        self.location(start..token.span.end),
                    ));
                }
                Kind::Text(text) => text,
                Kind::EscChar(ch) => ch.encode_utf8(&mut utf8),
                Kind::EscUtf16(first @ 0xd800..=0xdbff) => {
                    high = Some((first, token.span));
                    continue;
                }
                Kind::EscUtf16(0xdc00..=0xdfff) => {
                    return Err(self.error(token.span, "unexpected low surrogate"));
                }
                Kind::EscUtf16(value) => char::from_u32(u32::from(value))
                    .unwrap()
                    .encode_utf8(&mut utf8),
                Kind::Eof => return Err(self.error(start..token.span.end, "unclosed JSON string")),
                _ => unreachable!("string lexer mode"),
            };
            self.append(&mut output, text, token.span)?;
        }
    }
}
