use super::{
    lexer::Token,
    parser::{Diagnostic, Span},
};
use codespan_reporting::diagnostic::Label;

// Parser protection, independent of the type solver's maxTypeDepth.
// Deliberately conservative while the remaining grammar recursion is audited.
const MAX_NESTING: usize = 32;

pub(super) fn check(tokens: &[Token], spans: &[Span]) -> Option<Diagnostic> {
    let mut closers = Vec::new();
    for (&token, span) in tokens.iter().zip(spans) {
        let closer = match token {
            Token::LParen | Token::SectionLParen => Some(Token::RParen),
            Token::LBracket => Some(Token::RBracket),
            Token::LBrace | Token::InterpolationStart => Some(Token::RBrace),
            _ => None,
        };
        if let Some(closer) = closer {
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
