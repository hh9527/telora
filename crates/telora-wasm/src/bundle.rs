//! A single Wasm file carries code and original input bytes, not Host-built values.
use crate::artifact::{Manifest, Source};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use telora_core::{SourceDatabase, SourceId, data_plan::Format};

const SECTION: &str = "telora.data";

#[derive(Serialize, Deserialize)]
pub struct ModuleData {
    pub symbol: u32,
    pub source: Source,
    pub format: u32,
    pub text: String,
}

#[derive(Serialize, Deserialize)]
struct Bundle {
    version: u32,
    modules: Vec<ModuleData>,
}

fn validate(modules: &[ModuleData], manifest: &Manifest) -> Result<(), String> {
    let mut expected: Vec<_> = manifest
        .data_modules
        .iter()
        .map(|module| module.symbol)
        .collect();
    let mut actual: Vec<_> = modules.iter().map(|module| module.symbol).collect();
    expected.sort_unstable();
    actual.sort_unstable();
    if actual != expected || actual.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("Wasm: bundle must contain each data module exactly once".into());
    }
    for module in modules {
        if module.source.id == 0 || !(1..=3).contains(&module.format) {
            return Err("Wasm: invalid bundled source or format".into());
        }
        let lines = telora_core::source::LineIndex::new(&module.text)
            .map_err(|error| format!("Wasm: invalid bundled source: {error}"))?;
        if !lines.ranges().eq(module.source.lines.iter().copied()) {
            return Err("Wasm: bundled source index does not match input bytes".into());
        }
        if manifest.sources.iter().chain(modules.iter().map(|item| &item.source))
            .any(|source| source.id == module.source.id
                && (source.name != module.source.name || source.lines != module.source.lines)) {
            return Err("Wasm: bundled source identity conflict".into());
        }
    }
    Ok(())
}

/// Package data at publication time without instantiating an execution engine.
pub fn build(
    bytes: &[u8],
    sources: &SourceDatabase,
    plans: &[(u32, SourceId, Format)],
) -> Result<Vec<u8>, String> {
    let manifest = Manifest::read(bytes)?;
    let mut modules = Vec::with_capacity(plans.len());
    for (symbol, id, format) in plans {
        let file = sources.files().find(|file| file.id() == *id)
            .ok_or("Wasm: bundle source is not registered")?;
        modules.push(ModuleData {
            symbol: *symbol,
            source: Source::from_file(file),
            format: match format { Format::Json => 1, Format::Yaml => 2, Format::Toml => 3 },
            text: file.text().contiguous().ok_or("Wasm: bundle input requires contiguous text")?.to_owned(),
        });
    }
    modules.sort_by_key(|module| module.symbol);
    validate(&modules, &manifest)?;
    let mut output = wasm_encoder::Module::new();
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| e.to_string())?;
        if let wasmparser::Payload::CustomSection(section) = &payload {
            if section.name() == SECTION {
                return Err("Wasm: artifact is already bundled".into());
            }
            if section.name() == "telora.manifest" {
                continue;
            }
        }
        if let Some((id, range)) = payload.as_section() {
            let start =
                usize::try_from(range.start).map_err(|_| "Wasm: section offset overflow")?;
            let end = usize::try_from(range.end).map_err(|_| "Wasm: section offset overflow")?;
            output.section(&wasm_encoder::RawSection {
                id,
                data: bytes
                    .get(start..end)
                    .ok_or("Wasm: section outside artifact")?,
            });
        }
    }
    output.section(&wasm_encoder::CustomSection {
        name: Cow::Borrowed("telora.manifest"),
        data: Cow::Owned(serde_json::to_vec(&manifest).map_err(|e| e.to_string())?),
    });
    output.section(&wasm_encoder::CustomSection {
        name: Cow::Borrowed(SECTION),
        data: Cow::Owned(
            serde_json::to_vec(&Bundle {
                version: 2,
                modules,
            })
            .map_err(|e| e.to_string())?,
        ),
    });
    Ok(output.finish())
}

pub(crate) fn read(bytes: &[u8], manifest: &Manifest) -> Result<Vec<ModuleData>, String> {
    let mut bundle = None;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        if let wasmparser::Payload::CustomSection(section) = payload.map_err(|e| e.to_string())?
            && section.name() == SECTION
        {
            if bundle.is_some() {
                return Err("Wasm: duplicate data bundle".into());
            }
            bundle =
                Some(telora_data::json_serde::from_slice::<Bundle>(section.data()).map_err(|e| e.to_string())?);
        }
    }
    let Some(bundle) = bundle else {
        return Ok(vec![]);
    };
    if bundle.version != 2 {
        return Err("Wasm: unsupported data bundle version".into());
    }
    validate(&bundle.modules, manifest)?;
    Ok(bundle.modules)
}
