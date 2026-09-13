//! Test-only file input host. Fixture factories and leaves execute in Wasm.
use crate::{abi::*, output::Output, session::Session};
use std::path::Path;
use telora_core::{SourceDatabase, data_plan};

pub(super) fn select(
    session: &mut Session,
    mut value: u64,
    record: &serde_json::Value,
    root: &Path,
    sources: &SourceDatabase,
) -> Result<Option<u64>, String> {
    let mut sources = sources.clone();
    let indices = record["fixtures"]
        .as_array()
        .ok_or("missing fixture indices")?;
    let expected_sources = record["sources"]
        .as_array()
        .ok_or("missing fixture sources")?;
    for (depth, index) in indices.iter().enumerate() {
        let output = Output {
            memory: session.memory.data(&session.store),
            manifest: &session.manifest,
        };
        let (description, _) = output.payload(TESTS, output.word(value + DATA)?)?;
        if output.word(description)? != 3 {
            return Err("fixture index addresses a leaf".into());
        }
        let paths = output.word(description + 8)? as u64;
        let callback = output.word(description + 12)?;
        let (base, bytes) = output.payload(ARRAYS, output.word(paths + DATA)?)?;
        let start = output.word(paths + 20)? as u64;
        let end = output.word(paths + 24)? as u64;
        let index = index.as_u64().ok_or("invalid fixture index")?;
        if start > end || index >= end - start || end * 32 > bytes {
            return Err("fixture index out of range".into());
        }
        let path_value = base + (start + index) * 32;
        let requested = output.text(path_value)?;
        if expected_sources.get(depth).and_then(|value| value.as_str()) != Some(requested.as_str())
        {
            return Err(format!(
                "fixture path differs from default observation: {requested}"
            ));
        }
        let origin = output.word(path_value + SOURCE)?;
        let declaring = session
            .manifest
            .sources
            .iter()
            .find(|source| source.id == origin)
            .ok_or("fixture path has no declaring source")?
            .name
            .clone();
        let parsed = read(root, &declaring, &requested, &mut sources);
        let input = match parsed {
            Ok(plan) => {
                session.register_data_sources(&sources, &plan)?;
                session.initialize()?; // Register source names; initialization itself is cached.
                session.materialize_data(&plan)?
            }
            Err(reason) => {
                eprintln!("wasm fixture input rejected: {requested}: {reason}");
                if depth + 1 != expected_sources.len() {
                    return Err("fixture failed before the observed leaf".into());
                }
                return Ok(None);
            }
        };
        let args = session.allocate(4)?;
        session.write(args as usize, &input.to_le_bytes())?;
        let invoke = session
            .instance
            .get_typed_func::<(i32, i32), i32>(&session.store, "telora_invoke")
            .map_err(|e| e.to_string())?;
        value = invoke
            .call(&mut session.store, (callback as i32, args as i32))
            .map_err(|e| e.to_string())? as u32 as u64;
        if value == 0 {
            if depth + 1 != expected_sources.len() {
                return Err("factory failed before the observed leaf".into());
            }
            return Ok(None);
        }
    }
    if indices.len() != expected_sources.len() {
        return Err("fixture source depth mismatch".into());
    }
    let output = Output {
        memory: session.memory.data(&session.store),
        manifest: &session.manifest,
    };
    let (description, _) = output.payload(TESTS, output.word(value + DATA)?)?;
    if output.word(description)? == 3 {
        let paths = output.word(description + 8)? as u64;
        if output.word(paths + 20)? == output.word(paths + 24)? {
            // An empty fixture group has no leaf tests and is a failed case.
            return Ok(None);
        }
        return Err("nonempty fixture group has no remaining observation index".into());
    }
    Ok(Some(value))
}

fn read(
    root: &Path,
    declaring: &str,
    requested: &str,
    sources: &mut SourceDatabase,
) -> Result<data_plan::ValidatedDataPlan, String> {
    let (path, explicit) = match requested.split_once("://") {
        Some(("file+json", path)) => (path, Some(data_plan::Format::Json)),
        Some(("file+yaml", path)) => (path, Some(data_plan::Format::Yaml)),
        Some(("file+toml", path)) => (path, Some(data_plan::Format::Toml)),
        Some(_) => return Err("fixtures require local file sources".into()),
        None => (requested, None),
    };
    if Path::new(path).is_absolute() {
        return Err("absolute fixture path".into());
    }
    let declaring = declaring
        .strip_prefix("@src/")
        .ok_or("fixture declaring source is not local")?;
    let base = root.join(declaring);
    let file = base
        .parent()
        .ok_or("missing fixture base")?
        .join(path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !file.starts_with(root.canonicalize().map_err(|e| e.to_string())?) {
        return Err("fixture escapes source root".into());
    }
    let format = explicit
        .or_else(|| match file.extension().and_then(|ext| ext.to_str()) {
            Some("json") => Some(data_plan::Format::Json),
            Some("yaml" | "yml") => Some(data_plan::Format::Yaml),
            Some("toml") => Some(data_plan::Format::Toml),
            _ => None,
        })
        .ok_or("unknown fixture format")?;
    let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let source = sources.add(format!("@fixture/{}", sources.files().count()), &text);
    data_plan::parse_registered(sources, source, format).map_err(|ds| format!("{ds:?}"))
}
