//! Local recognition by Logos; quoting state and offsets are owned by Scanner.
//! Boundary discovery and decoding share the same quote/escape transitions.
use core::ops::Range;
use logos::Logos;

#[derive(Logos, Debug, PartialEq)]
pub(super) enum LineToken {
    #[regex(r" +")]
    Spaces,
    #[regex(r"\t+")]
    Tabs,
    #[regex(r"\r\n|\r|\n")]
    Eol,
    #[regex(r"[^ \t\r\n]+")]
    Text,
}

#[derive(Logos)]
enum Normal {
    #[token("\"")]
    Double,
    #[token("'")]
    Single,
    #[token(":")]
    Colon,
    #[token("#")]
    Hash,
    #[token("[")]
    Array,
    #[token("]")]
    EndArray,
    #[token("{")]
    Object,
    #[token("}")]
    EndObject,
    #[token(",")]
    Comma,
    #[regex(r"\s+")]
    Space,
    #[regex(r#"[^\s"':#\[\]{},]+"#)]
    Text,
}

#[derive(Logos)]
enum Double {
    #[token("\"")]
    End,
    #[token("\\")]
    Escape,
    #[regex(r#"[^"\\]+"#)]
    Text,
}

#[derive(Logos)]
enum Single {
    #[token("''")]
    Quote,
    #[token("'")]
    End,
    #[regex(r"[^']+")]
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind {
    Start,
    End,
    Text,
    Space,
    Colon,
    Hash,
    Open,
    Close,
    Comma,
    Escape(Option<char>),
    Quote,
}

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    Single,
    Double,
}

pub(super) struct Token {
    pub kind: Kind,
    pub span: Range<usize>,
}

pub(super) struct Scanner<'a> {
    text: &'a str,
    pub pos: usize,
    mode: Mode,
}

impl<'a> Scanner<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            pos: 0,
            mode: Mode::Normal,
        }
    }
    pub fn character(&mut self) -> Option<char> {
        let ch = self.text[self.pos..].chars().next()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }
    pub fn next(&mut self) -> Option<Token> {
        if self.pos == self.text.len() {
            return None;
        }
        let start = self.pos;
        let mut end = (start + 4096).min(self.text.len());
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }
        // A doubled quote must not be divided by our bounded scanning window.
        let run = &self.text[start..end];
        let (kind, size) = match self.mode {
            Mode::Normal => {
                let mut lexer = Normal::lexer(run);
                let kind = match lexer
                    .next()
                    .expect("nonempty run")
                    .expect("complete normal alphabet")
                {
                    Normal::Double => {
                        self.mode = Mode::Double;
                        Kind::Start
                    }
                    Normal::Single => {
                        self.mode = Mode::Single;
                        Kind::Start
                    }
                    Normal::Colon => Kind::Colon,
                    Normal::Hash => Kind::Hash,
                    Normal::Array | Normal::Object => Kind::Open,
                    Normal::EndArray | Normal::EndObject => Kind::Close,
                    Normal::Comma => Kind::Comma,
                    Normal::Space => Kind::Space,
                    Normal::Text => Kind::Text,
                };
                (kind, lexer.span().end)
            }
            Mode::Single => {
                let mut lexer = Single::lexer(run);
                let kind = match lexer
                    .next()
                    .expect("nonempty run")
                    .expect("complete single alphabet")
                {
                    Single::End => {
                        self.mode = Mode::Normal;
                        Kind::End
                    }
                    Single::Quote => Kind::Quote,
                    Single::Text => Kind::Text,
                };
                (kind, lexer.span().end)
            }
            Mode::Double => {
                let mut lexer = Double::lexer(run);
                let kind = match lexer
                    .next()
                    .expect("nonempty run")
                    .expect("complete double alphabet")
                {
                    Double::End => {
                        self.mode = Mode::Normal;
                        Kind::End
                    }
                    Double::Escape => Kind::Escape(None),
                    Double::Text => Kind::Text,
                };
                (kind, lexer.span().end)
            }
        };
        self.pos += size;
        let kind = if matches!(kind, Kind::Escape(_)) {
            Kind::Escape(self.character())
        } else {
            kind
        };
        Some(Token {
            kind,
            span: start..self.pos,
        })
    }
}

/// These searches intentionally don't validate escapes: decoding owns errors,
/// while quotes must still shield punctuation from the structural parser.
pub(super) fn mapping(text: &str) -> Option<usize> {
    let mut scanner = Scanner::new(text);
    let mut depth = 0usize;
    while let Some(token) = scanner.next() {
        match token.kind {
            Kind::Open => depth += 1,
            Kind::Close => depth = depth.saturating_sub(1),
            Kind::Hash if comment(text, token.span.start) => return None,
            Kind::Colon
                if depth == 0
                    && text[token.span.end..]
                        .chars()
                        .next()
                        .is_none_or(char::is_whitespace) =>
            {
                return Some(token.span.start);
            }
            _ => {}
        }
    }
    None
}
fn comment(text: &str, at: usize) -> bool {
    at == 0 || text[..at].ends_with(char::is_whitespace)
}
pub(super) fn uncomment(text: &str) -> &str {
    let mut scanner = Scanner::new(text);
    while let Some(token) = scanner.next() {
        if token.kind == Kind::Hash && comment(text, token.span.start) {
            return &text[..token.span.start];
        }
    }
    text
}
pub(super) fn scalar_end(text: &str, stops: &[u8]) -> usize {
    let mut scanner = Scanner::new(text);
    while let Some(token) = scanner.next() {
        if matches!(token.kind, Kind::Colon | Kind::Comma | Kind::Close)
            && stops.contains(&text.as_bytes()[token.span.start])
        {
            return token.span.start;
        }
    }
    text.len()
}
