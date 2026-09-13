use telora_core::{Diagnostic, SourceDatabase, source::Severity};
use telora_wasm::session::Session;

pub(super) fn collect(
    session: &Session,
    sources: &SourceDatabase,
) -> Result<Vec<Diagnostic>, String> {
    let location = |words: [u32; 3]| {
        sources
            .files()
            .find(|file| file.id().get() == words[0])
            .map(|file| telora_core::Loc {
                source: file.id(),
                start: words[1],
                end: words[2],
            })
    };
    Ok(session
        .diagnostics()?
        .into_iter()
        .map(|event| {
            let mut diagnostic = match location(event.origin) {
                Some(loc) => Diagnostic::error(&event.message, loc),
                None => super::error(&event.message),
            };
            diagnostic.severity = if event.warning {
                Severity::Warning
            } else {
                Severity::Error
            };
            for (index, subject) in event.subjects.into_iter().enumerate() {
                if subject == event.origin {
                    continue;
                }
                if let Some(loc) = location(subject) {
                    diagnostic = diagnostic
                        .with_secondary(format!("subject {} originated here", index + 1), loc);
                }
            }
            diagnostic
        })
        .collect())
}

pub(super) fn finish<T>(
    session: &Session,
    sources: &SourceDatabase,
    before: usize,
    result: Result<T, String>,
) -> Result<T, String> {
    let diagnostics = collect(session, sources)?;
    let mut errors = vec![];
    for diagnostic in diagnostics.iter().skip(before) {
        let rendered = sources.render(diagnostic);
        if diagnostic.severity == Severity::Error {
            errors.push(rendered);
        } else {
            eprintln!("{rendered}");
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    result
}
