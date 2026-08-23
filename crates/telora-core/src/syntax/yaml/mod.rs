pub mod lexer;
pub mod parser;

pub use parser::CstData;

pub fn parse_document(
    source_id: crate::source::SourceId,
    source: &crate::document::DocumentText,
) -> super::Parse<CstData> {
    let mut diagnostics = Vec::new();
    let (tokens, spans) = lexer::tokenize_document(source, &mut diagnostics);
    let cst =
        parser::Parser::from_token_stream(source.byte_len(), tokens, spans).parse(&mut diagnostics);
    super::Parse {
        syntax: cst.into_data(),
        diagnostics: super::convert_diagnostics(source_id, diagnostics),
    }
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
