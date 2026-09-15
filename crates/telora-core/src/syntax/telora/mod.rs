pub mod ast;
pub mod cst;
mod token;
mod tree_sitter;
pub use cst::CstData;
pub use token::Token;

/// Parse with cooperative cancellation. A cancelled parse publishes no CST.
/// Checks during parsing (including source reads), token/CST traversal, and
/// structural validation. Individual node operations are not preemptible.
pub fn parse_document_cancellable(
    source_id: crate::source::SourceId,
    source: &crate::document::DocumentText,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<super::Parse<CstData>> {
    tree_sitter::parse_document_cancellable(source_id, source, cancelled)
}

pub fn parse(source_id: crate::source::SourceId, source: &str) -> super::Parse<CstData> {
    tree_sitter::parse(source_id, source)
}

pub fn parse_document(
    source_id: crate::source::SourceId,
    source: &crate::document::DocumentText,
) -> super::Parse<CstData> {
    tree_sitter::parse_document(source_id, source)
}

fn finish_parse_cancellable(
    source_id: crate::source::SourceId,
    syntax: CstData,
    diagnostics: Vec<cst::Diagnostic>,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<super::Parse<CstData>> {
    if cancelled() {
        return None;
    }
    let mut diagnostics = super::convert_diagnostics(source_id, diagnostics);
    let mut starts: std::collections::BTreeSet<_> = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.labels.first().map(|label| label.location.start))
        .collect();
    for issue in ast::validate_cancellable(source_id, &syntax, cancelled)? {
        if cancelled() {
            return None;
        }
        let diagnostic = issue.into_diagnostic();
        let start = diagnostic.labels[0].location.start;
        if starts.insert(start) {
            diagnostics.push(diagnostic);
        }
    }
    diagnostics.sort_by_key(|diagnostic| {
        diagnostic
            .labels
            .first()
            .map_or(u32::MAX, |label| label.location.start)
    });
    if cancelled() {
        return None;
    }
    Some(super::Parse {
        syntax,
        diagnostics,
    })
}

#[cfg(test)]
mod tests;
