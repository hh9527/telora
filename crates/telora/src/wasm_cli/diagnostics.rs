use telora_core::{Diagnostic, SourceDatabase, source::Severity};
use telora_wasm::session::Session;

fn debug(session: &Session) -> Result<(), String> {
    for event in session.take_debug_events()? {
        eprintln!(
            "{}",
            serde_json::to_string(&event).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

pub(super) fn collect(
    session: &Session,
    sources: &SourceDatabase,
) -> Result<Vec<Diagnostic>, String> {
    debug(session)?;
    Ok(convert(session.diagnostics()?, sources))
}

pub(super) fn convert(
    events: Vec<telora_wasm::diagnostic_output::Diagnostic>,
    sources: &SourceDatabase,
) -> Vec<Diagnostic> {
    let location = |words: [u32; 5]| {
        sources
            .files()
            .find(|file| file.id().get() == words[0])
            .and_then(|file| file.byte_location(telora_core::source::SourceCoordinates(words)))
    };
    events
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
        .collect()
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
