use std::collections::BTreeMap;
use std::fs;
use std::io;
use telora_core::{EvalSource, SystemDataFormat, SystemDataSource};

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
            has_default: false,
        },
    })
}

pub(crate) fn collect_service_sources(
    sources: Vec<NamedSource>,
    max_bytes: usize,
) -> Result<BTreeMap<String, EvalSource>, String> {
    let mut collected = BTreeMap::new();
    let mut read_stdin = false;
    for source in sources {
        if collected.contains_key(&source.name) {
            return Err(format!(
                "source {:?} was provided more than once",
                source.name
            ));
        }
        let public_name = service_source_name(&source.name);
        let description = format!("service source {public_name:?}");
        let locator = source.source.src.as_str();
        let bytes = if let Some((scheme, location)) = locator.split_once("://") {
            if scheme.starts_with("stdin+") {
                if read_stdin {
                    return Err("standard input can provide at most one named source".into());
                }
                read_stdin = true;
                read_limited(io::stdin().lock(), max_bytes, &description)?
            } else {
                let file = fs::File::open(location)
                    .map_err(|error| format!("cannot read {description}: {error}"))?;
                read_limited(file, max_bytes, &description)?
            }
        } else {
            let file = fs::File::open(locator)
                .map_err(|error| format!("cannot read {description}: {error}"))?;
            read_limited(file, max_bytes, &description)?
        };
        let text = String::from_utf8(bytes)
            .map_err(|error| format!("service source is not UTF-8: {error}"))?;
        collected.insert(
            source.name,
            EvalSource {
                source_name: public_name,
                format: source.source.format,
                text,
            },
        );
    }
    Ok(collected)
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

fn service_source_name(key: &str) -> String {
    let mut encoded = String::with_capacity(key.len());
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    format!("@service/{encoded}")
}
