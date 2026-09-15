use alloc::vec::Vec;
pub mod lexer;
pub mod parser;

pub use parser::CstData;

pub fn parse(source_id: crate::source::SourceId, source: &str) -> super::Parse<CstData> {
    let mut diagnostics = Vec::new();
    let (tokens, spans) = lexer::tokenize(source, &mut diagnostics);
    let cst = parse_tokens(source.len(), tokens, spans, &mut diagnostics);
    super::Parse {
        syntax: cst.into_data(),
        diagnostics: super::convert_diagnostics(source_id, diagnostics),
    }
}

pub fn parse_document(
    source_id: crate::source::SourceId,
    source: &crate::document::DocumentText,
) -> super::Parse<CstData> {
    let mut diagnostics = Vec::new();
    let (tokens, spans) = lexer::tokenize_document(source, &mut diagnostics);
    let cst = parse_tokens(source.byte_len(), tokens, spans, &mut diagnostics);
    super::Parse {
        syntax: cst.into_data(),
        diagnostics: super::convert_diagnostics(source_id, diagnostics),
    }
}

#[cfg(test)]
mod tests;

fn parse_tokens(
    source_len: usize,
    tokens: Vec<lexer::Token>,
    spans: Vec<parser::Span>,
    diagnostics: &mut Vec<parser::Diagnostic>,
) -> parser::Cst<'static> {
    use super::nesting::{self, Delimiter};
    use lexer::Token;
    let boundaries = tokens.iter().zip(&spans).filter_map(|(token, span)| {
        let boundary = match token {
            Token::LBrace => Delimiter::Open('}'),
            Token::LBracket => Delimiter::Open(']'),
            Token::RBrace => Delimiter::Close('}'),
            Token::RBracket => Delimiter::Close(']'),
            _ => return None,
        };
        Some((boundary, span.clone()))
    });
    if let Some(diagnostic) = nesting::check(boundaries) {
        diagnostics.push(diagnostic);
        return parser::Parser::from_token_stream(source_len, Vec::new(), Vec::new()).parse(diagnostics);
    }
    parser::Parser::from_token_stream(source_len, tokens, spans).parse(diagnostics)
}
