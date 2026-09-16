//! A forward-only chunk cursor. String text is consumed in bounded borrowed runs;
//! quote/escape state survives chunk boundaries without a growing pending buffer.
use super::build::Build;
use super::lexer::{self, Normal};
use crate::source::{Diagnostic, Location, SourceId};
use alloc::{string::String, vec::Vec};

pub(super) struct Input<'a> {
    chunks: Vec<&'a str>,
    chunk: usize,
    local: usize,
    pub offset: usize,
    source: SourceId,
}

impl<'a> Input<'a> {
    pub fn new(source: SourceId, chunks: impl Iterator<Item = &'a str>) -> Self {
        Self {
            source,
            chunks: chunks.filter(|s| !s.is_empty()).collect(),
            chunk: 0,
            local: 0,
            offset: 0,
        }
    }
    pub fn peek(&self) -> Option<u8> {
        self.nth(0)
    }
    pub fn nth(&self, mut n: usize) -> Option<u8> {
        let mut chunk = self.chunk;
        let mut local = self.local;
        while let Some(text) = self.chunks.get(chunk) {
            let remaining = text.len() - local;
            if n < remaining {
                return Some(text.as_bytes()[local + n]);
            }
            n -= remaining;
            chunk += 1;
            local = 0;
        }
        None
    }
    pub fn rest(&self) -> &'a str {
        self.chunks
            .get(self.chunk)
            .map_or("", |text| &text[self.local..])
    }
    pub fn run(&self) -> &'a str {
        let text = self.rest();
        let mut end = text.len().min(4096);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    }
    pub fn key_run(&self) -> usize {
        lexer::key(self.run())
    }
    pub fn atom_run(&self) -> usize {
        match lexer::normal(self.run()) {
            Some((Normal::Atom, n)) => n,
            _ => 0,
        }
    }
    pub fn take(&mut self, bytes: usize) -> &'a str {
        let text = &self.rest()[..bytes];
        self.local += bytes;
        self.offset += bytes;
        if self
            .chunks
            .get(self.chunk)
            .is_some_and(|s| self.local == s.len())
        {
            self.chunk += 1;
            self.local = 0;
        }
        text
    }
    pub fn bump(&mut self) -> Option<char> {
        let ch = self.rest().chars().next()?;
        self.take(ch.len_utf8());
        Some(ch)
    }
    pub fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.take(1);
            true
        } else {
            false
        }
    }
    pub fn loc(&self, start: usize) -> Location {
        Location::from_usize(self.source, start..self.offset).expect("registered TOML span")
    }
    pub fn error(&self, start: usize, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.loc(start))
    }
    pub fn newline(&mut self) -> bool {
        if self.eat(b'\r') {
            self.eat(b'\n');
            true
        } else {
            self.eat(b'\n')
        }
    }
    pub fn space(&mut self, lines: bool) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t') => {
                    let (Normal::Space, n) = lexer::normal(self.run()).expect("space token") else {
                        unreachable!()
                    };
                    self.take(n);
                }
                Some(b'\r' | b'\n') if lines => {
                    self.newline();
                }
                Some(b'#') if lines => self.comment(),
                _ => break,
            }
        }
    }
    pub fn comment(&mut self) {
        while self.peek().is_some_and(|b| !matches!(b, b'\r' | b'\n')) {
            self.bump();
        }
    }
    fn append(
        &self,
        build: &Build,
        out: &mut String,
        text: &str,
        start: usize,
        value: bool,
    ) -> Result<(), Diagnostic> {
        build.string_size(out.len() + text.len(), self.loc(start), value)?;
        out.push_str(text);
        Ok(())
    }
    pub fn string(&mut self, build: &Build, value: bool) -> Result<String, Diagnostic> {
        let start = self.offset;
        let quote = self.peek().expect("quote");
        self.take(1);
        let multiline = self.peek() == Some(quote) && self.nth(1) == Some(quote);
        if multiline {
            self.take(1);
            self.take(1);
            if !value {
                return Err(self.error(start, "multiline TOML strings cannot be keys"));
            }
            self.newline();
        }
        let basic = quote == b'"';
        let mut output = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error(start, "unclosed TOML string"));
            };
            if byte == quote {
                if !multiline {
                    self.take(1);
                    return Ok(output);
                }
                let mut count = 0;
                while self.peek() == Some(quote) && count < 5 {
                    self.take(1);
                    count += 1;
                }
                if count >= 3 {
                    for _ in 3..count {
                        self.append(
                            build,
                            &mut output,
                            if basic { "\"" } else { "'" },
                            start,
                            value,
                        )?;
                    }
                    return Ok(output);
                }
                for _ in 0..count {
                    self.append(
                        build,
                        &mut output,
                        if basic { "\"" } else { "'" },
                        start,
                        value,
                    )?;
                }
            } else if matches!(byte, b'\r' | b'\n') {
                if !multiline {
                    return Err(self.error(start, "newline in single-line TOML string"));
                }
                self.newline();
                self.append(build, &mut output, "\n", start, value)?;
            } else if basic && byte == b'\\' {
                self.take(1);
                if multiline && matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    self.space(false);
                    if !self.newline() {
                        return Err(self.error(start, "TOML line continuation requires a newline"));
                    }
                    while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                        self.bump();
                    }
                    continue;
                }
                let ch = match self.bump() {
                    Some('b') => '\u{0008}',
                    Some('t') => '\t',
                    Some('n') => '\n',
                    Some('f') => '\u{000c}',
                    Some('r') => '\r',
                    Some('"') => '"',
                    Some('\\') => '\\',
                    Some(code @ ('u' | 'U')) => {
                        let mut scalar = 0u32;
                        for _ in 0..if code == 'u' { 4 } else { 8 } {
                            let digit = self
                                .bump()
                                .and_then(|c| c.to_digit(16))
                                .ok_or_else(|| self.error(start, "invalid TOML Unicode escape"))?;
                            scalar = scalar * 16 + digit;
                        }
                        char::from_u32(scalar)
                            .ok_or_else(|| self.error(start, "invalid TOML Unicode scalar"))?
                    }
                    None => return Err(self.error(start, "unterminated TOML escape")),
                    _ => return Err(self.error(start, "unknown TOML escape")),
                };
                self.append(
                    build,
                    &mut output,
                    ch.encode_utf8(&mut [0; 4]),
                    start,
                    value,
                )?;
            } else {
                let end = lexer::text(self.run(), basic);
                if end == 0
                    || self.run()[..end]
                        .chars()
                        .any(|ch| ch.is_control() && ch != '\t')
                {
                    return Err(
                        self.error(start, "TOML String contains a forbidden control character")
                    );
                }
                let text = self.take(end);
                self.append(build, &mut output, text, start, value)?;
            }
        }
    }
}
