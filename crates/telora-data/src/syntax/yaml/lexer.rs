use super::parser::{Diagnostic, Span};
use alloc::{string::String, vec::Vec};

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Token {
    EOF,
    Line,
    Error,
}

pub fn tokenize(source: &str, _diagnostics: &mut Vec<Diagnostic>) -> (Vec<Token>, Vec<Span>) {
    let mut tokens = Vec::new();
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            tokens.push(Token::Line);
            spans.push(start..index + 1);
            start = index + 1;
        }
    }
    if start < source.len() {
        tokens.push(Token::Line);
        spans.push(start..source.len());
    }
    (tokens, spans)
}

pub fn tokenize_document(
    source: &crate::document::DocumentText,
    _diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<Token>, Vec<Span>) {
    let mut tokens = Vec::new();
    let mut spans = Vec::new();
    let mut pending = String::new();
    let mut offset = 0usize;
    for fragment in source.chunks() {
        pending.push_str(fragment);
        while let Some(newline) = pending.find('\n') {
            let end = offset + newline + 1;
            tokens.push(Token::Line);
            spans.push(offset..end);
            pending.drain(..newline + 1);
            offset = end;
        }
    }
    if !pending.is_empty() {
        tokens.push(Token::Line);
        spans.push(offset..source.byte_len());
    }
    (tokens, spans)
}
