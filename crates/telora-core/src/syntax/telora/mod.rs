pub mod ast;
pub mod lexer;
mod nesting;
pub mod parser;

pub use parser::CstData;

pub fn parse(source_id: crate::source::SourceId, source: &str) -> super::Parse<CstData> {
    let mut diagnostics = Vec::new();
    let (tokens, spans) = lexer::tokenize(source, &mut diagnostics);
    let cst = parse_tokens(source.len(), tokens, spans, &mut diagnostics);
    finish_parse(source_id, cst.into_data(), diagnostics)
}

pub fn parse_document(
    source_id: crate::source::SourceId,
    source: &crate::document::DocumentText,
) -> super::Parse<CstData> {
    let mut diagnostics = Vec::new();
    let (tokens, spans) = lexer::tokenize_document(source, &mut diagnostics);
    let cst = parse_tokens(source.byte_len(), tokens, spans, &mut diagnostics);
    finish_parse(source_id, cst.into_data(), diagnostics)
}

fn parse_tokens(
    source_len: usize,
    tokens: Vec<lexer::Token>,
    spans: Vec<parser::Span>,
    diagnostics: &mut Vec<parser::Diagnostic>,
) -> parser::Cst<'static> {
    if let Some(diagnostic) = nesting::check(&tokens, &spans) {
        diagnostics.push(diagnostic);
        // Rejected input must never reach the recursive grammar. An empty CST
        // allows downstream diagnostic consumers to use the usual Parse API.
        return parser::Parser::from_token_stream(source_len, Vec::new(), Vec::new())
            .parse(diagnostics);
    }
    parser::Parser::from_token_stream(source_len, tokens, spans).parse(diagnostics)
}

fn finish_parse(
    source_id: crate::source::SourceId,
    syntax: CstData,
    diagnostics: Vec<parser::Diagnostic>,
) -> super::Parse<CstData> {
    let mut diagnostics = super::convert_diagnostics(source_id, diagnostics);
    for issue in ast::validate(source_id, &syntax) {
        let diagnostic = issue.into_diagnostic();
        let start = diagnostic.labels[0].location.start;
        if !diagnostics.iter().any(|existing| {
            existing
                .labels
                .first()
                .is_some_and(|label| label.location.start == start)
        }) {
            diagnostics.push(diagnostic);
        }
    }
    diagnostics.sort_by_key(|diagnostic| {
        diagnostic
            .labels
            .first()
            .map_or(u32::MAX, |label| label.location.start)
    });
    super::Parse {
        syntax,
        diagnostics,
    }
}

#[cfg(test)]
mod tests;
