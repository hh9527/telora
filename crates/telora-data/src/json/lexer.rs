//! Chunk boundaries carry no grammar meaning. Modes and the input cursor live
//! outside Logos; only an unfinished atom/escape crosses a chunk boundary.
use alloc::borrow::Cow;
use core::ops::Range;
use logos::Logos;

#[derive(Logos, Clone, Copy)]
enum Normal {
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token("\"")]
    Quote,
    #[regex(r"[ \t\r\n]+")]
    Space,
    #[regex(r"[a-zA-Z0-9_.+\-]+")]
    Atom,
}

#[derive(Logos, Clone, Copy)]
enum Quoted {
    #[token("\"")]
    End,
    #[token("\\")]
    Escape,
    #[regex(r#"[^"\\\x00-\x1f]+"#)]
    Text,
}

#[derive(Logos, Clone, Copy)]
enum Atom {
    #[token("true")]
    True,
    #[token("false")]
    False,
    #[token("null")]
    Null,
    #[regex(r"-?(0|[1-9][0-9]*)")]
    Integer,
    #[regex(r"-?(0|[1-9][0-9]*)(\.[0-9]+([eE][+-]?[0-9]+)?|[eE][+-]?[0-9]+)")]
    Float,
}

#[derive(Debug)]
pub(super) enum Kind<'a> {
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    QStart,
    QEnd,
    Text(&'a str),
    EscChar(char),
    EscUtf16(u16),
    True,
    False,
    Null,
    Number { float: bool },
    Eof,
}

pub(super) struct Token<'a> {
    pub kind: Kind<'a>,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub(super) struct Error {
    pub message: &'static str,
    pub span: Range<usize>,
}

#[derive(Default)]
enum Mode {
    #[default]
    Normal,
    String,
}

pub(super) struct Scanner<'a, I> {
    chunks: I,
    remaining: &'a str,
    offset: usize,
    mode: Mode,
}

impl<'a, I: Iterator<Item = &'a str>> Scanner<'a, I> {
    pub fn new(chunks: I) -> Self {
        Self {
            chunks,
            remaining: "",
            offset: 0,
            mode: Mode::Normal,
        }
    }

    fn ready(&mut self) -> bool {
        while self.remaining.is_empty() {
            let Some(chunk) = self.chunks.next() else {
                return false;
            };
            self.remaining = chunk;
        }
        true
    }

    fn take(&mut self, bytes: usize) -> &'a str {
        let (text, rest) = self.remaining.split_at(bytes);
        self.remaining = rest;
        self.offset += bytes;
        text
    }

    fn character(&mut self) -> Option<char> {
        if !self.ready() {
            return None;
        }
        let ch = self.remaining.chars().next()?;
        self.take(ch.len_utf8());
        Some(ch)
    }

    fn error(&self, start: usize, message: &'static str) -> Error {
        Error {
            message,
            span: start..self.offset,
        }
    }

    pub fn next(&mut self) -> Result<Token<'a>, Error> {
        loop {
            if !self.ready() {
                return Ok(Token {
                    kind: Kind::Eof,
                    span: self.offset..self.offset,
                });
            }
            let start = self.offset;
            let kind = match self.mode {
                Mode::Normal => {
                    let mut lexer = Normal::lexer(self.remaining);
                    let token = lexer.next().expect("nonempty input");
                    let text = self.take(lexer.span().end);
                    match token {
                        Ok(Normal::Space) => continue,
                        Ok(Normal::LBrace) => Kind::LBrace,
                        Ok(Normal::RBrace) => Kind::RBrace,
                        Ok(Normal::LBracket) => Kind::LBracket,
                        Ok(Normal::RBracket) => Kind::RBracket,
                        Ok(Normal::Comma) => Kind::Comma,
                        Ok(Normal::Colon) => Kind::Colon,
                        Ok(Normal::Quote) => {
                            self.mode = Mode::String;
                            Kind::QStart
                        }
                        Ok(Normal::Atom) => self.atom(start, text)?,
                        Err(_) => return Err(self.error(start, "invalid JSON token")),
                    }
                }
                Mode::String => {
                    // Bound a Text token even for callers supplying one giant
                    // chunk, so quota checks run before scanning its whole tail.
                    let mut end = self.remaining.len().min(4096);
                    while !self.remaining.is_char_boundary(end) {
                        end -= 1;
                    }
                    let mut lexer = Quoted::lexer(&self.remaining[..end]);
                    let token = lexer.next().expect("nonempty input");
                    let text = self.take(lexer.span().end);
                    match token {
                        Ok(Quoted::End) => {
                            self.mode = Mode::Normal;
                            Kind::QEnd
                        }
                        Ok(Quoted::Text) => Kind::Text(text),
                        Ok(Quoted::Escape) => self.escape(start)?,
                        Err(_) => {
                            return Err(
                                self.error(start, "unescaped control character in JSON string")
                            );
                        }
                    }
                }
            };
            return Ok(Token {
                kind,
                span: start..self.offset,
            });
        }
    }

    fn atom(&mut self, start: usize, text: &'a str) -> Result<Kind<'a>, Error> {
        let mut text = Cow::Borrowed(text);
        while self.remaining.is_empty() && self.ready() {
            let length = self
                .remaining
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
                .count();
            if length == 0 {
                break;
            }
            text.to_mut().push_str(self.take(length));
        }
        let mut lexer = Atom::lexer(text.as_ref());
        let token = lexer.next();
        if lexer.span().end != text.len() {
            return Err(self.error(start, "invalid JSON literal or number"));
        }
        Ok(match token {
            Some(Ok(Atom::True)) => Kind::True,
            Some(Ok(Atom::False)) => Kind::False,
            Some(Ok(Atom::Null)) => Kind::Null,
            Some(Ok(Atom::Integer)) => Kind::Number { float: false },
            Some(Ok(Atom::Float)) => Kind::Number { float: true },
            _ => return Err(self.error(start, "invalid JSON literal or number")),
        })
    }

    fn escape(&mut self, start: usize) -> Result<Kind<'a>, Error> {
        let ch = self
            .character()
            .ok_or_else(|| self.error(start, "unterminated JSON escape"))?;
        Ok(Kind::EscChar(match ch {
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'b' => '\u{0008}',
            'f' => '\u{000c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'u' => {
                let mut value = 0u16;
                for _ in 0..4 {
                    let digit =
                        self.character()
                            .and_then(|ch| ch.to_digit(16))
                            .ok_or_else(|| {
                                self.error(start, "Unicode escape requires four hex digits")
                            })?;
                    value = value * 16 + digit as u16;
                }
                return Ok(Kind::EscUtf16(value));
            }
            _ => return Err(self.error(start, "unknown JSON escape")),
        }))
    }
}
