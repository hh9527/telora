use std::fs;
use telora_core::{SystemDataFormat, SystemDataSource};

#[derive(Clone)]
pub(crate) struct NamedSource {
    name: String,
    source: SystemDataSource,
}

pub(crate) fn parse_fixture_source(value: &str) -> Result<SystemDataSource, String> {
    let source = parse_named_source(&format!("fixture={value}"))?.source;
    if is_stdin_source(&source.src) {
        return Err("fixtures require local file sources".into());
    }
    Ok(source)
}

pub(crate) fn is_stdin_source(src: &str) -> bool {
    matches!(
        src.split_once("://"),
        Some((scheme, "")) if scheme.starts_with("stdin+")
    )
}

pub(crate) fn reject_stdin_sources(sources: &[NamedSource]) -> Result<(), String> {
    if sources.iter().any(|source| is_stdin_source(&source.source.src)) {
        return Err("service reserves stdin for request input".into());
    }
    Ok(())
}

fn data_format(name: &str) -> Result<SystemDataFormat, String> {
    match name {
        "json" => Ok(SystemDataFormat::Json),
        "yaml" | "yml" => Ok(SystemDataFormat::Yaml),
        "toml" => Ok(SystemDataFormat::Toml),
        _ => Err(format!("unsupported source format {name:?}")),
    }
}

fn infer_data_format(path: &str) -> Result<SystemDataFormat, String> {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| {
            format!("source {path:?} has no recognized extension; use file+FORMAT://PATH")
        })?;
    data_format(&extension.to_ascii_lowercase())
}

pub(crate) fn parse_named_source(value: &str) -> Result<NamedSource, String> {
    let (name, spec) = value
        .split_once('=')
        .ok_or_else(|| "source must use NAME=SOURCE".to_owned())?;
    if name.is_empty() {
        return Err("source name must not be empty".into());
    }
    if spec.is_empty() {
        return Err("source location must not be empty".into());
    }
    let (format, src) = if let Some((scheme, rest)) = spec.split_once("://") {
        let (transport, format) = scheme
            .split_once('+')
            .ok_or_else(|| format!("source URI scheme {scheme:?} must be TRANSPORT+FORMAT"))?;
        if !matches!(transport, "file" | "stdin") {
            return Err(format!("unsupported source transport {transport:?}"));
        }
        if transport == "stdin" && !rest.is_empty() {
            return Err("stdin source URI must not contain a path".into());
        }
        if transport == "file" && rest.is_empty() {
            return Err("file source URI must contain a path".into());
        }
        (data_format(format)?, spec.to_owned())
    } else {
        (infer_data_format(spec)?, spec.to_owned())
    };
    Ok(NamedSource {
        name: name.to_owned(),
        source: SystemDataSource {
            src,
            format,
        },
    })
}

pub(crate) fn service_source_readers(
    mut sources: Vec<NamedSource>,
) -> impl Iterator<Item = Result<telora_wasm::transform_service::SourceReader<'static>, String>> {
    sources.sort_by(|a, b| a.name.cmp(&b.name));
    sources.into_iter().map(|source| {
        if is_stdin_source(&source.source.src) {
            return Err("service reserves stdin for request input".into());
        }
        let locator = source.source.src.as_str();
        let path = locator.split_once("://").map_or(locator, |(_, path)| path);
        let file = fs::File::open(path)
            .map_err(|error| format!("cannot read service source {:?}: {error}", source.name))?;
        let format = match source.source.format {
            SystemDataFormat::Json => telora_core::data_plan::Format::Json,
            SystemDataFormat::Yaml => telora_core::data_plan::Format::Yaml,
            SystemDataFormat::Toml => telora_core::data_plan::Format::Toml,
        };
        Ok(telora_wasm::transform_service::SourceReader {
            name: source.name, reader: Box::new(file), format,
        })
    })
}

pub(crate) use telora::static_input::read_limited;

pub(crate) fn service_source_names(sources: &[NamedSource]) -> Result<Vec<String>, String> {
    let mut names = sources
        .iter()
        .map(|source| source.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("an service source name was provided more than once".into());
    }
    Ok(names)
}
