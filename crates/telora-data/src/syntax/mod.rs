use alloc::vec::Vec;
pub mod toml;
pub mod yaml;

use crate::source::Diagnostic;

#[derive(Debug)]
pub struct Parse<T> {
    pub syntax: T,
    pub diagnostics: Vec<Diagnostic>,
}

impl<T> Parse<T> {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == crate::source::Severity::Error)
    }
}

pub fn convert_diagnostics(
    source: crate::source::SourceId,
    diagnostics: Vec<codespan_reporting::diagnostic::Diagnostic<()>>,
) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .map(|diagnostic| {
            let severity = match diagnostic.severity {
                codespan_reporting::diagnostic::Severity::Bug
                | codespan_reporting::diagnostic::Severity::Error => crate::source::Severity::Error,
                _ => crate::source::Severity::Warning,
            };
            let labels = diagnostic
                .labels
                .into_iter()
                .enumerate()
                .map(|(index, label)| crate::source::Label {
                    location: crate::source::Location::from_usize(source, label.range)
                        .expect("lexer span fits registered source"),
                    message: label.message,
                    primary: index == 0,
                })
                .collect();
            Diagnostic {
                severity,
                message: diagnostic.message,
                labels,
                notes: diagnostic.notes,
            }
        })
        .collect()
}
