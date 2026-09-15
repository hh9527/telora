use super::{
    lexer::Token,
    parser::{Diagnostic, Span},
};
use codespan_reporting::diagnostic::Label;

// Parser protection, independent of the type solver's maxTypeDepth.
// Deliberately conservative while the remaining grammar recursion is audited.
const MAX_NESTING: usize = 32;

fn closer(token: Token) -> Option<Token> {
    match token {
        Token::LParen | Token::SectionLParen => Some(Token::RParen),
        Token::LBracket => Some(Token::RBracket),
        Token::LBrace | Token::InterpolationStart => Some(Token::RBrace),
        _ => None,
    }
}

// Depth before each token, used by grammar actions to distinguish an operand
// from explicitly delimited subexpressions. This temporary table dies with the
// parser, not with the CST. Mismatched closers cannot cancel an open boundary.
pub(super) fn depths(tokens: &[Token]) -> Vec<u32> {
    let mut closers = Vec::new();
    tokens
        .iter()
        .map(|&token| {
            let depth = u32::try_from(closers.len()).expect("source token count fits u32");
            if let Some(expected) = closer(token) {
                closers.push(expected);
            } else if closers.last() == Some(&token) {
                closers.pop();
            }
            depth
        })
        .collect()
}

pub(super) fn check(tokens: &[Token], spans: &[Span]) -> Option<Diagnostic> {
    let mut closers = Vec::new();
    for (&token, span) in tokens.iter().zip(spans) {
        if let Some(closer) = closer(token) {
            if closers.len() == MAX_NESTING {
                return Some(
                    Diagnostic::error()
                        .with_message(format!(
                            "syntax nesting exceeds parser limit ({MAX_NESTING})"
                        ))
                        .with_label(Label::primary((), span.clone())),
                );
            }
            closers.push(closer);
        } else if matches!(token, Token::RParen | Token::RBracket | Token::RBrace) {
            // Leave mismatches to ordinary parser recovery, but never let a
            // mismatched closer reduce our nesting estimate.
            if closers.last() == Some(&token) {
                closers.pop();
            }
        }
    }
    None
}
