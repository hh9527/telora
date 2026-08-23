use super::parser::{Diagnostic, Span};
use codespan_reporting::diagnostic::Label;
use logos::{Lexer, Logos};

#[derive(Debug, Clone, PartialEq, Default)]
pub enum LexerError {
    #[default]
    Invalid,
}

impl LexerError {
    pub fn into_diagnostic(self, span: Span) -> Diagnostic {
        Diagnostic::error()
            .with_message("invalid TOML token")
            .with_label(Label::primary((), span))
    }
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Token {
    EOF,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Equals,
    Dot,
    String,
    Atom,
    Newline,
    Whitespace,
    Comment,
    Error,
}

#[derive(Logos, Debug, PartialEq, Copy, Clone)]
#[logos(error = LexerError)]
enum LexToken {
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
    #[token("=")]
    Equals,
    #[token(".")]
    Dot,
    #[token("\"", scan_basic_string)]
    #[token("'", scan_literal_string)]
    String,
    #[regex(r"[A-Za-z0-9_+:-]+(\.[A-Za-z0-9_+:-]+)*")]
    Atom,
    #[regex(r"\r?\n")]
    Newline,
    #[regex(r"[ \t]+")]
    Whitespace,
    #[regex(r"#[^\r\n]*", allow_greedy = true)]
    Comment,
}

fn scan_basic_string(lexer: &mut Lexer<'_, LexToken>) -> bool {
    scan_string(lexer, b'"', true)
}

fn scan_literal_string(lexer: &mut Lexer<'_, LexToken>) -> bool {
    scan_string(lexer, b'\'', false)
}

fn scan_string(lexer: &mut Lexer<'_, LexToken>, quote: u8, escaped: bool) -> bool {
    let remainder = lexer.remainder().as_bytes();
    let multiline = remainder.starts_with(&[quote, quote]);
    let mut index = if multiline { 2 } else { 0 };
    while index < remainder.len() {
        if multiline && remainder[index..].starts_with(&[quote, quote, quote]) {
            let quote_count = remainder[index..]
                .iter()
                .take_while(|byte| **byte == quote)
                .count()
                .min(5);
            lexer.bump(index + quote_count);
            return true;
        }
        if !multiline && remainder[index] == quote {
            lexer.bump(index + 1);
            return true;
        }
        if !multiline && matches!(remainder[index], b'\n' | b'\r') {
            lexer.bump(index);
            return false;
        }
        if escaped && remainder[index] == b'\\' {
            index += 1;
            if index < remainder.len() {
                index += 1;
            }
        } else {
            index += 1;
        }
    }
    lexer.bump(remainder.len());
    false
}

pub fn tokenize(source: &str, diagnostics: &mut Vec<Diagnostic>) -> (Vec<Token>, Vec<Span>) {
    let mut tokens = Vec::new();
    let mut spans = Vec::new();
    let mut lexer = LexToken::lexer(source);
    let mut square_depth = 0usize;
    while let Some(result) = lexer.next() {
        let span = lexer.span();
        let token = match result {
            Ok(token) => token.into(),
            Err(error) => {
                diagnostics.push(error.into_diagnostic(span.clone()));
                Token::Error
            }
        };
        let token = match token {
            Token::LBracket => {
                square_depth += 1;
                token
            }
            Token::RBracket => {
                square_depth = square_depth.saturating_sub(1);
                token
            }
            Token::Newline if square_depth > 0 => Token::Whitespace,
            _ => token,
        };
        tokens.push(token);
        spans.push(span);
    }
    let mut merged_tokens = Vec::with_capacity(tokens.len());
    let mut merged_spans = Vec::with_capacity(spans.len());
    let mut index = 0usize;
    while index < tokens.len() {
        if index + 2 < tokens.len()
            && tokens[index] == Token::Atom
            && tokens[index + 1] == Token::Whitespace
            && tokens[index + 2] == Token::Atom
            && &source[spans[index + 1].clone()] == " "
            && looks_like_date(&source[spans[index].clone()])
            && looks_like_time(&source[spans[index + 2].clone()])
        {
            merged_tokens.push(Token::Atom);
            merged_spans.push(spans[index].start..spans[index + 2].end);
            index += 3;
        } else {
            merged_tokens.push(tokens[index]);
            merged_spans.push(spans[index].clone());
            index += 1;
        }
    }
    tokens = merged_tokens;
    spans = merged_spans;
    if tokens.last() != Some(&Token::Newline) {
        tokens.push(Token::Newline);
        spans.push(source.len()..source.len());
    }
    (tokens, spans)
}

pub fn tokenize_document(
    source: &crate::document::DocumentText,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<Token>, Vec<Span>) {
    tokenize_fragments(source.chunks(), source.byte_len(), diagnostics)
}

fn tokenize_fragments<'a>(
    fragments: impl IntoIterator<Item = &'a str>,
    source_len: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<Token>, Vec<Span>) {
    let mut tokens = Vec::new();
    let mut spans = Vec::new();
    let mut pending = String::new();
    let mut pending_start = 0usize;
    for fragment in fragments {
        pending.push_str(fragment);
        let boundary = stable_statement_boundary(&pending);
        if boundary == 0 {
            continue;
        }
        append_segment(
            &pending[..boundary],
            pending_start,
            &mut tokens,
            &mut spans,
            diagnostics,
        );
        pending.drain(..boundary);
        pending_start += boundary;
    }
    if !pending.is_empty() {
        append_segment(
            &pending,
            pending_start,
            &mut tokens,
            &mut spans,
            diagnostics,
        );
    } else if tokens.last() != Some(&Token::Newline) {
        tokens.push(Token::Newline);
        spans.push(source_len..source_len);
    }
    (tokens, spans)
}

fn append_segment(
    segment: &str,
    offset: usize,
    tokens: &mut Vec<Token>,
    spans: &mut Vec<Span>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut local_diagnostics = Vec::new();
    let (local_tokens, local_spans) = tokenize(segment, &mut local_diagnostics);
    tokens.extend(local_tokens);
    spans.extend(
        local_spans
            .into_iter()
            .map(|span| offset + span.start..offset + span.end),
    );
    for mut diagnostic in local_diagnostics {
        for label in &mut diagnostic.labels {
            label.range = offset + label.range.start..offset + label.range.end;
        }
        diagnostics.push(diagnostic);
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ScanMode {
    Normal,
    Basic,
    Literal,
    MultilineBasic,
    MultilineLiteral,
    Comment,
}

fn stable_statement_boundary(source: &str) -> usize {
    let bytes = source.as_bytes();
    let mut mode = ScanMode::Normal;
    let mut square_depth = 0usize;
    let mut stable = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match mode {
            ScanMode::Normal => match bytes[index] {
                b'#' => mode = ScanMode::Comment,
                b'[' => square_depth += 1,
                b']' => square_depth = square_depth.saturating_sub(1),
                b'"' if bytes[index..].starts_with(b"\"\"\"") => {
                    mode = ScanMode::MultilineBasic;
                    index += 2;
                }
                b'\'' if bytes[index..].starts_with(b"'''") => {
                    mode = ScanMode::MultilineLiteral;
                    index += 2;
                }
                b'"' => mode = ScanMode::Basic,
                b'\'' => mode = ScanMode::Literal,
                b'\n' if square_depth == 0 => stable = index + 1,
                _ => {}
            },
            ScanMode::Comment => {
                if bytes[index] == b'\n' {
                    mode = ScanMode::Normal;
                    if square_depth == 0 {
                        stable = index + 1;
                    }
                }
            }
            ScanMode::Basic => match bytes[index] {
                b'\\' => index += usize::from(index + 1 < bytes.len()),
                b'"' => mode = ScanMode::Normal,
                _ => {}
            },
            ScanMode::Literal => {
                if bytes[index] == b'\'' {
                    mode = ScanMode::Normal;
                }
            }
            ScanMode::MultilineBasic => {
                if bytes[index..].starts_with(b"\"\"\"") {
                    mode = ScanMode::Normal;
                    index += bytes[index..]
                        .iter()
                        .take_while(|byte| **byte == b'"')
                        .count()
                        .min(5)
                        - 1;
                } else if bytes[index] == b'\\' {
                    index += usize::from(index + 1 < bytes.len());
                }
            }
            ScanMode::MultilineLiteral => {
                if bytes[index..].starts_with(b"'''") {
                    mode = ScanMode::Normal;
                    index += bytes[index..]
                        .iter()
                        .take_while(|byte| **byte == b'\'')
                        .count()
                        .min(5)
                        - 1;
                }
            }
        }
        index += 1;
    }
    stable
}

fn looks_like_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
}

fn looks_like_time(value: &str) -> bool {
    value.len() >= 8
        && value.as_bytes().get(2) == Some(&b':')
        && value.as_bytes().get(5) == Some(&b':')
}

impl From<LexToken> for Token {
    fn from(token: LexToken) -> Self {
        match token {
            LexToken::LBrace => Self::LBrace,
            LexToken::RBrace => Self::RBrace,
            LexToken::LBracket => Self::LBracket,
            LexToken::RBracket => Self::RBracket,
            LexToken::Comma => Self::Comma,
            LexToken::Equals => Self::Equals,
            LexToken::Dot => Self::Dot,
            LexToken::String => Self::String,
            LexToken::Atom => Self::Atom,
            LexToken::Newline => Self::Newline,
            LexToken::Whitespace => Self::Whitespace,
            LexToken::Comment => Self::Comment,
        }
    }
}

#[cfg(test)]
#[path = "lexer/tests/mod.rs"]
mod tests;
