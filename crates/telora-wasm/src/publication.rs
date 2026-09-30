//! Experimental executable-file envelope. Source execution does not use it.
use serde::{Deserialize, Serialize};

pub const SECTION: &str = "telora.build";

#[derive(Serialize, Deserialize)]
pub struct Publication {
    pub version: u32,
    pub abi: u32,
    pub initialization_fuel: u64,
    pub request_fuel: u64,
    pub memory_limit: u64,
}

pub fn finish(
    bytes: &[u8],
    memory_limit: usize,
    initialization_fuel: u64,
    request_fuel: u64,
) -> Result<Vec<u8>, String> {
    let manifest = crate::artifact::Manifest::read(bytes)?;
    let parts = crate::template::Parts::read(bytes)?;
    if !manifest.data_modules.is_empty()
        && !parts.custom.iter().any(|(name, _)| {
            name == "telora.modules" || name == telora_wasm_shared::snapshot_artifact::SECTION
        })
    {
        return Err("data modules must be compiled before publication".into());
    }
    let mut module = wasm_encoder::Module::new();
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| e.to_string())?;
        if let wasmparser::Payload::CustomSection(section) = &payload {
            if section.name() == SECTION {
                return Err("artifact already published".into());
            }
            if matches!(section.name(), "telora.tooling" | "telora.modules" | "name") {
                continue;
            }
        }
        if let Some((id, range)) = payload.as_section() {
            module.section(&wasm_encoder::RawSection {
                id,
                data: &bytes[range.start as usize..range.end as usize],
            });
        }
    }
    let metadata = Publication {
        version: 4,
        abi: crate::runtime_abi::VERSION,
        initialization_fuel,
        request_fuel,
        memory_limit: memory_limit as u64,
    };
    module.section(&wasm_encoder::CustomSection {
        name: "telora.manifest".into(),
        data: serde_json::to_vec(&serde_json::json!({"abi": manifest.abi}))
            .map_err(|e| e.to_string())?
            .into(),
    });
    module.section(&wasm_encoder::CustomSection {
        name: SECTION.into(),
        data: serde_json::to_vec(&metadata)
            .map_err(|e| e.to_string())?
            .into(),
    });
    Ok(module.finish())
}

pub fn attach_snapshot(
    bytes: &[u8],
    snapshot: &telora_wasm_shared::snapshot_artifact::Snapshot,
) -> Result<Vec<u8>, String> {
    let bytes = crate::bundle::remove_image(bytes)?;
    let mut module = wasm_encoder::Module::new();
    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        let payload = payload.map_err(|error| error.to_string())?;
        if let wasmparser::Payload::CustomSection(section) = &payload
            && section.name() == telora_wasm_shared::snapshot_artifact::SECTION
        {
            return Err("artifact already contains a service snapshot".into());
        }
        if let Some((id, range)) = payload.as_section() {
            module.section(&wasm_encoder::RawSection {
                id,
                data: &bytes[range.start as usize..range.end as usize],
            });
        }
    }
    module.section(&wasm_encoder::CustomSection {
        name: telora_wasm_shared::snapshot_artifact::SECTION.into(),
        data: telora_wasm_shared::snapshot_artifact::encode(snapshot)?.into(),
    });
    Ok(module.finish())
}
