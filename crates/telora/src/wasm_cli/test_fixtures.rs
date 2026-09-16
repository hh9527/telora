use std::collections::HashMap;
use telora_core::{
    DataLimits, Diagnostic, Loc, SourceDatabase, SystemDataFormat, TestContext,
    data_plan::{self, ValidatedDataPlan},
    test_plan::TestResult,
};

pub(super) fn diagnostic(message: impl Into<String>, origin: Option<Loc>) -> Diagnostic {
    let message = message.into();
    origin.map_or_else(
        || super::error(&message),
        |loc| Diagnostic::error(&message, loc),
    )
}

pub(super) fn location(sources: &SourceDatabase, words: [u32; 3]) -> Option<Loc> {
    sources
        .files()
        .find(|file| file.id().get() == (words[0] & 0xffff))
        .and_then(|file| file.byte_location(telora_core::source::CompactLoc(words)))
}

pub(super) struct Fixtures<'a, 'b> {
    pub context: &'a mut TestContext<'b>,
    pub limits: DataLimits,
    pub sources: &'a mut SourceDatabase,
    pub admitted_bytes: usize,
}

impl Fixtures<'_, '_> {
    pub fn prepare(
        &mut self,
        module: &str,
        case: &TestResult,
        label: &str,
        origin: Option<Loc>,
        cache: &mut HashMap<String, Result<String, String>>,
    ) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
        let error = |message: String| vec![diagnostic(message, origin)];
        let declaring = origin
            .map(|loc| self.sources.get(loc.source).name.to_string())
            .ok_or_else(|| error("fixture has no declaring module".into()))?;
        let host = self
            .context
            .host
            .as_deref_mut()
            .ok_or_else(|| error("fixture source host is unavailable".into()))?;
        let source = host
            .resolve(
                &declaring,
                self.context
                    .module_paths
                    .get(&declaring)
                    .map(|p| p.as_path()),
                label,
            )
            .map_err(error)?;
        let text = cache
            .entry(source.key.clone())
            .or_insert_with(|| host.read(&source, self.limits.file_size))
            .as_ref()
            .map_err(|e| error(e.clone()))?;
        if text.len() > self.limits.file_size {
            return Err(error("fixture file size limit exceeded".into()));
        }
        self.admitted_bytes = self.admitted_bytes.saturating_add(text.len());
        if self.admitted_bytes > self.context.limits.fixture_bytes {
            return Err(error("cumulative fixture input limit exceeded".into()));
        }
        let name = format!(
            "@test-ctx/{}/{}/{}",
            encode(module),
            encode(&case.name),
            case.fixtures
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("/")
        );
        let id = self.sources.add(name, text);
        let format = match source.format {
            SystemDataFormat::Json => data_plan::Format::Json,
            SystemDataFormat::Yaml => data_plan::Format::Yaml,
            SystemDataFormat::Toml => data_plan::Format::Toml,
        };
        let plan = data_plan::parse_registered_with_limits(self.sources, id, format, self.limits)?;
        Ok(plan)
    }
}

fn encode(text: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").unwrap();
        }
    }
    encoded
}
