use telora_core::{Diagnostic, SourceDatabase, source::Severity};
use telora_wasm::session::Session;

pub(super) fn location(sources: &SourceDatabase, words: [u32; 5]) -> Option<telora_core::Loc> {
    if words[1] == 0 || words[3] == 0 {
        return None;
    }
    let mut words = words;
    words[1] -= 1;
    words[3] -= 1;
    sources
        .files()
        .find(|file| file.id().get() == words[0])
        .and_then(|file| file.byte_location(telora_core::source::SourceCoordinates(words)))
}

/// Convert Guest protocol coordinates for the CLI renderer; no data parsing.
pub(super) fn parsed(
    events: serde_json::Value,
    sources: &SourceDatabase,
) -> Result<Vec<Diagnostic>, String> {
    let number = |value: &serde_json::Value| {
        value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| "invalid diagnostic coordinate".to_owned())
    };
    let text = |value: &serde_json::Value| {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "invalid diagnostic text".to_owned())
    };
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
        for loc in event["locs"].as_array().ok_or("invalid diagnostic locs")? {
            let range = loc;
            let name = text(&range["source"])?;
            let file = sources
                .files()
                .find(|file| file.name.as_ref() == name)
                .ok_or("unknown diagnostic source")?;
            let coordinates = [
                file.id().get(),
                number(&range["start"]["line"])?,
                number(&range["start"]["offset"])?,
                number(&range["end"]["line"])?,
                number(&range["end"]["offset"])?,
            ];
            diagnostic
                .locs
                .push(location(sources, coordinates).ok_or("invalid diagnostic range")?);
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
    events
        .into_iter()
        .map(|event| {
            let mut diagnostic = match event.locs.first().and_then(|loc| location(sources, *loc)) {
                Some(loc) => Diagnostic::error(&event.message, loc),
                None => super::error(&event.message),
            };
            diagnostic.severity = event.severity;
            for subject in event.locs.into_iter().skip(1) {
                if let Some(loc) = location(sources, subject) {
                    diagnostic.locs.push(loc);
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
