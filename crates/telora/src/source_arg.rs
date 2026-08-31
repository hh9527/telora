use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use telora_core::{EntryDataSources, EvalSource, SystemDataFormat, SystemDataSource};

#[derive(Clone)]
pub(crate) struct NamedSource {
    name: String,
    source: SystemDataSource,
}

pub(crate) struct CollectedEntrySources {
    pub(crate) entry: EntryDataSources,
    pub(crate) locators: BTreeMap<String, String>,
}

pub(crate) fn is_stdin_source(src: &str) -> bool {
    matches!(
        src.split_once("://"),
        Some((scheme, "")) if scheme.starts_with("stdin+")
    )
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

pub(crate) fn collect_entry_sources(
    sources: Vec<NamedSource>,
) -> Result<CollectedEntrySources, String> {
    let mut entry = BTreeMap::new();
    let mut locators = BTreeMap::new();
    for source in sources {
        if entry.contains_key(&source.name) {
            return Err(format!(
                "source {:?} was provided more than once",
                source.name
            ));
        }
        let public_name = run_context_source_name(&source.name);
        locators.insert(public_name.clone(), source.source.src.clone());
        entry.insert(
            source.name,
            SystemDataSource {
                src: public_name,
                ..source.source
            },
        );
    }
    let stdin_count = locators
        .values()
        .filter(|locator| is_stdin_source(locator))
        .count();
    if stdin_count > 1 {
        return Err("standard input can provide at most one named source".into());
    }
    Ok(CollectedEntrySources { entry, locators })
}

pub(crate) fn collect_eval_sources(
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
        let public_name = run_context_source_name(&source.name).replace("@run-ctx/", "@eval-ctx/");
        let description = format!("eval source {public_name:?}");
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
            .map_err(|error| format!("eval source is not UTF-8: {error}"))?;
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

fn read_limited(reader: impl Read, max_bytes: usize, description: &str) -> Result<Vec<u8>, String> {
    let max_read = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    reader
        .take(max_read)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {description}: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "{description} exceeds file_size limit ({} > {max_bytes})",
            bytes.len()
        ));
    }
    Ok(bytes)
}

pub(crate) fn eval_source_names(sources: &[NamedSource]) -> Result<Vec<String>, String> {
    let mut names = sources
        .iter()
        .map(|source| source.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("an eval source name was provided more than once".into());
    }
    Ok(names)
}

fn run_context_source_name(key: &str) -> String {
    let mut encoded = String::with_capacity(key.len());
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    format!("@run-ctx/{encoded}")
}
