use telora_core::{Diagnostic, SourceDatabase, source::Severity};
use telora_wasm::session::Session;

/// Convert Guest protocol coordinates for the CLI renderer; no data parsing.
pub(super) fn parsed(events: serde_json::Value, sources: &SourceDatabase) -> Result<Vec<Diagnostic>, String> {
    let number = |value: &serde_json::Value| value.as_u64()
        .and_then(|n| u32::try_from(n).ok()).ok_or_else(|| "invalid diagnostic coordinate".to_owned());
    let text = |value: &serde_json::Value| value.as_str().map(str::to_owned)
        .ok_or_else(|| "invalid diagnostic text".to_owned());
    let array = events.as_array().ok_or("invalid diagnostic array")?;
    let mut result = Vec::new();
    for event in array {
        let mut diagnostic = super::error(text(&event["message"])?);
        diagnostic.severity = match event["severity"].as_str() {
            Some("Error") => Severity::Error,
            Some("Warning") => Severity::Warning,
            Some("Info") => Severity::Info,
            _ => return Err("invalid diagnostic severity".into()),
        };
        for label in event["labels"].as_array().ok_or("invalid diagnostic labels")? {
            let range = &label["location"];
            let name = text(&range["source"])?;
            let file = sources.files().find(|file| file.name.as_ref() == name)
                .ok_or("unknown diagnostic source")?;
            let coordinates = telora_core::source::SourceCoordinates([
                file.id().get(), number(&range["start"]["line"])?, number(&range["start"]["offset"])?,
                number(&range["end"]["line"])?, number(&range["end"]["offset"])?,
            ]);
            diagnostic.labels.push(telora_core::source::Label {
                location: file.byte_location(coordinates).ok_or("invalid diagnostic range")?,
                message: text(&label["message"])?,
                primary: label["primary"].as_bool().ok_or("invalid diagnostic label")?,
            });
        }
        for note in event["notes"].as_array().ok_or("invalid diagnostic notes")? {
            diagnostic.notes.push(text(note)?);
        }
        result.push(diagnostic);
    }
    Ok(result)
}

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
