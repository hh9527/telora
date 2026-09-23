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
    let mut module = wasm_encoder::Module::new();
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| e.to_string())?;
        if let wasmparser::Payload::CustomSection(section) = &payload {
            if section.name() == SECTION {
                return Err("artifact already published".into());
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
        version: 3,
        abi: crate::runtime_abi::VERSION,
        initialization_fuel,
        request_fuel,
        memory_limit: memory_limit as u64,
    };
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
    let mut module = wasm_encoder::Module::new();
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
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
