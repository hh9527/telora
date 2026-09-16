//! Contiguous borrowed cursor; quote/escape state is explicit, runs are bounded.
use super::lexer::{self, Normal};
use super::structure::Text;
use crate::source::{Diagnostic, Location, SourceId};
use alloc::string::String;

pub(super) struct Input<'a> {
    text: &'a str,
    pub offset: usize,
    source: SourceId,
}
impl<'a> Input<'a> {
    pub fn new(source: SourceId, text: &'a str) -> Self {
        Self {
            source,
            text,
            offset: 0,
        }
    }
    pub fn peek(&self) -> Option<u8> {
        self.nth(0)
    }
    pub fn nth(&self, n: usize) -> Option<u8> {
        self.text.as_bytes().get(self.offset + n).copied()
    }
    pub fn rest(&self) -> &'a str {
        &self.text[self.offset..]
    }
    pub fn slice(&self, start: usize) -> &'a str {
        &self.text[start..self.offset]
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
        let start = self.offset;
        self.offset += bytes;
        &self.text[start..self.offset]
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
        length: &mut usize,
        text: &str,
        start: usize,
        emit: &mut impl FnMut(&str, usize, Location) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        let next = length
            .checked_add(text.len())
            .ok_or_else(|| self.error(start, "data string length overflow"))?;
        emit(text, next, self.loc(start))?;
        *length = next;
        Ok(())
    }
    pub fn string(
        &mut self,
        value: bool,
        mut emit: impl FnMut(&str, usize, Location) -> Result<(), Diagnostic>,
    ) -> Result<Text, Diagnostic> {
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
        let body_start = self.offset;
        let mut output = 0;
        let mut transformed = false;
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error(start, "unclosed TOML string"));
            };
            if byte == quote {
                if !multiline {
                    self.take(1);
                    return Ok(Text {
                        range: if transformed {
                            start..self.offset
                        } else {
                            body_start..self.offset - if multiline { 3 } else { 1 }
                        },
                        decoded_len: output,
                        escaped: transformed,
                    });
                }
                let mut count = 0;
                while self.peek() == Some(quote) && count < 5 {
                    self.take(1);
                    count += 1;
                }
                if count >= 3 {
                    for _ in 3..count {
                        self.append(
                            &mut output,
                            if basic { "\"" } else { "'" },
                            start,
                            &mut emit,
                        )?;
                    }
                    return Ok(Text {
                        range: if transformed {
                            start..self.offset
                        } else {
                            body_start..self.offset - if multiline { 3 } else { 1 }
                        },
                        decoded_len: output,
                        escaped: transformed,
                    });
                }
                for _ in 0..count {
                    self.append(
                        &mut output,
                        if basic { "\"" } else { "'" },
                        start,
                        &mut emit,
                    )?;
                }
            } else if matches!(byte, b'\r' | b'\n') {
                if !multiline {
                    return Err(self.error(start, "newline in single-line TOML string"));
                }
                transformed |= byte == b'\r';
                self.newline();
                self.append(&mut output, "\n", start, &mut emit)?;
            } else if basic && byte == b'\\' {
                transformed = true;
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
                self.append(&mut output, ch.encode_utf8(&mut [0; 4]), start, &mut emit)?;
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
                self.append(&mut output, text, start, &mut emit)?;
            }
        }
    }
}
