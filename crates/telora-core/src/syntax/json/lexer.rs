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
            .with_message("invalid token")
            .with_label(Label::primary((), span))
    }
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Token {
    EOF,
    True,
    False,
    Null,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    DoubleQuote,
    StringText,
    EscapeSequence,
    UnknownEscapeSequence,
    MalformedUnicodeEscape,
    UnterminatedEscapeSequence,
    Number,
    Whitespace,
    Error,
}

#[derive(Logos, Debug, PartialEq, Copy, Clone)]
#[logos(error = LexerError, extras = LexerState)]
enum NormalToken {
    #[token("true")]
    True,
    #[token("false")]
    False,
    #[token("null")]
    Null,
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
    DoubleQuote,
    #[regex(r"-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?")]
    Number,
    #[regex(r"[ \t\r\n]+")]
    Whitespace,
}

#[derive(Logos, Debug, PartialEq, Copy, Clone)]
#[logos(error = LexerError, extras = LexerState)]
enum StringToken {
    #[token("\"")]
    DoubleQuote,
    #[regex(r#"\\(u[0-9A-Fa-f]{4}|["\\/bfnrt])"#, priority = 5)]
    EscapeSequence,
    #[regex(r#"\\u[^"\\\x00-\x1f]{0,4}"#, priority = 4)]
    MalformedUnicodeEscape,
    #[regex(r#"\\."#, priority = 3)]
    UnknownEscapeSequence,
    #[token("\\", priority = 1)]
    UnterminatedEscapeSequence,
    #[regex(r#"[^\"\\\x00-\x1f]+"#)]
    StringText,
}

#[derive(Clone, Copy, Debug, Default)]
enum Mode {
    #[default]
    Normal,
    String,
}

#[derive(Debug, Default)]
struct LexerState {
    mode: Mode,
}

enum ActiveLexer<'source> {
    Normal(Lexer<'source, NormalToken>),
    String(Lexer<'source, StringToken>),
}

pub fn tokenize(source: &str, diags: &mut Vec<Diagnostic>) -> (Vec<Token>, Vec<Span>) {
    let mut tokens = Vec::new();
    let mut spans = Vec::new();
    let mut active = ActiveLexer::Normal(NormalToken::lexer(source));
    loop {
        let (token, span, error, next) = match active {
            ActiveLexer::Normal(mut lexer) => {
                let Some(result) = lexer.next() else {
                    break;
                };
                let span = lexer.span();
                let (token, error) = match result {
                    Ok(token) => (token.into(), false),
                    Err(_) => (Token::Error, true),
                };
                let next = if token == Token::DoubleQuote {
                    lexer.extras.mode = Mode::String;
                    ActiveLexer::String(lexer.morph())
                } else {
                    ActiveLexer::Normal(lexer)
                };
                (token, span, error, next)
            }
            ActiveLexer::String(mut lexer) => {
                let Some(result) = lexer.next() else {
                    break;
                };
                let span = lexer.span();
                let (token, error) = match result {
                    Ok(token) => (token.into(), false),
                    Err(_) => (Token::Error, true),
                };
                let next = if token == Token::DoubleQuote {
                    lexer.extras.mode = Mode::Normal;
                    ActiveLexer::Normal(lexer.morph())
                } else {
                    ActiveLexer::String(lexer)
                };
                (token, span, error, next)
            }
        };
        let error = error || token_is_invalid_escape(token);
        if error {
            let message = match token {
                Token::UnknownEscapeSequence => "unknown JSON escape",
                Token::MalformedUnicodeEscape => "malformed JSON Unicode escape",
                Token::UnterminatedEscapeSequence => "unterminated JSON escape",
                _ => "invalid token",
            };
            diags.push(
                Diagnostic::error()
                    .with_message(message)
                    .with_label(Label::primary((), span.clone())),
            );
        }
        tokens.push(token);
        spans.push(span);
        active = next;
    }
    (tokens, spans)
}

pub fn tokenize_document(
    source: &crate::document::DocumentText,
    diags: &mut Vec<Diagnostic>,
) -> (Vec<Token>, Vec<Span>) {
    tokenize_fragments(source.chunks(), diags)
}

fn tokenize_fragments<'a>(
    fragments: impl IntoIterator<Item = &'a str>,
    diags: &mut Vec<Diagnostic>,
) -> (Vec<Token>, Vec<Span>) {
    let mut tokens = Vec::new();
    let mut spans = Vec::new();
    let mut pending = String::new();
    let mut pending_start = 0;
    for fragment in fragments {
        pending.push_str(fragment);
        let mut local_diags = Vec::new();
        let (local_tokens, local_spans) = tokenize(&pending, &mut local_diags);
        let commit = stable_normal_prefix(&local_tokens);
        if commit == 0 {
            continue;
        }
        let committed_end = local_spans[commit - 1].end;
        tokens.extend_from_slice(&local_tokens[..commit]);
        spans.extend(
            local_spans[..commit]
                .iter()
                .map(|span| pending_start + span.start..pending_start + span.end),
        );
        for mut diagnostic in local_diags {
            if diagnostic
                .labels
                .iter()
                .all(|label| label.range.end <= committed_end)
            {
                for label in &mut diagnostic.labels {
                    label.range =
                        pending_start + label.range.start..pending_start + label.range.end;
                }
                diags.push(diagnostic);
            }
        }
        pending.drain(..committed_end);
        pending_start += committed_end;
    }
    if !pending.is_empty() {
        let mut local_diags = Vec::new();
        let (local_tokens, local_spans) = tokenize(&pending, &mut local_diags);
        tokens.extend(local_tokens);
        spans.extend(
            local_spans
                .into_iter()
                .map(|span| pending_start + span.start..pending_start + span.end),
        );
        for mut diagnostic in local_diags {
            for label in &mut diagnostic.labels {
                label.range = pending_start + label.range.start..pending_start + label.range.end;
            }
            diags.push(diagnostic);
        }
    }
    (tokens, spans)
}

fn stable_normal_prefix(tokens: &[Token]) -> usize {
    let mut string = false;
    let mut stable = 0;
    for (index, token) in tokens.iter().copied().enumerate() {
        if token == Token::DoubleQuote {
            string = !string;
        }
        if !string && index + 9 <= tokens.len() {
            stable = index + 1;
        }
    }
    stable
}

impl From<NormalToken> for Token {
    fn from(token: NormalToken) -> Self {
        match token {
            NormalToken::True => Self::True,
            NormalToken::False => Self::False,
            NormalToken::Null => Self::Null,
            NormalToken::LBrace => Self::LBrace,
            NormalToken::RBrace => Self::RBrace,
            NormalToken::LBracket => Self::LBracket,
            NormalToken::RBracket => Self::RBracket,
            NormalToken::Comma => Self::Comma,
            NormalToken::Colon => Self::Colon,
            NormalToken::DoubleQuote => Self::DoubleQuote,
            NormalToken::Number => Self::Number,
            NormalToken::Whitespace => Self::Whitespace,
        }
    }
}

impl From<StringToken> for Token {
    fn from(token: StringToken) -> Self {
        match token {
            StringToken::DoubleQuote => Self::DoubleQuote,
            StringToken::EscapeSequence => Self::EscapeSequence,
            StringToken::UnknownEscapeSequence => Self::UnknownEscapeSequence,
            StringToken::MalformedUnicodeEscape => Self::MalformedUnicodeEscape,
            StringToken::UnterminatedEscapeSequence => Self::UnterminatedEscapeSequence,
            StringToken::StringText => Self::StringText,
        }
    }
}

fn token_is_invalid_escape(token: Token) -> bool {
    matches!(
        token,
        Token::UnknownEscapeSequence
            | Token::MalformedUnicodeEscape
            | Token::UnterminatedEscapeSequence
    )
}

#[cfg(test)]
#[path = "lexer/tests/mod.rs"]
mod tests;
