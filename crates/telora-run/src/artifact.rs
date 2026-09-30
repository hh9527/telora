//! Minimal publication envelope; compiled values and runtime types live in Wasm.
use anyhow::{Result, bail, ensure};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Publication {
    pub version: u32,
    pub abi: u32,
    pub initialization_fuel: u64,
    pub request_fuel: u64,
    pub memory_limit: u64,
}

#[derive(Deserialize)]
struct Manifest {
    abi: u32,
}

pub(crate) struct Artifact {
    pub publication: Publication,
    pub snapshot: Option<telora_wasm_shared::snapshot_artifact::Snapshot>,
}

impl Artifact {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let (mut publication, mut manifest, mut snapshot) = (None, None, None);
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            match payload? {
                wasmparser::Payload::CustomSection(s) => match s.name() {
                    "telora.build" => {
                        ensure!(publication.is_none(), "duplicate publication metadata");
                        publication = Some(serde_json::from_slice::<Publication>(s.data())?);
                    }
                    "telora.manifest" => {
                        ensure!(manifest.is_none(), "duplicate manifest");
                        manifest = Some(serde_json::from_slice::<Manifest>(s.data())?);
                    }
                    "telora.data" => {
                        bail!("source-text data bundles are not supported");
                    }
                    "telora.tooling" => {
                        bail!("compiler tooling metadata is not a publication artifact");
                    }
                    telora_wasm_shared::snapshot_artifact::SECTION => {
                        ensure!(snapshot.is_none(), "duplicate service snapshot");
                        snapshot = Some(
                            telora_wasm_shared::snapshot_artifact::decode(s.data())
                                .map_err(anyhow::Error::msg)?,
                        );
                    }
                    _ => {}
                },
                wasmparser::Payload::MemorySection(memories) => {
                    ensure!(memories.count() == 1, "expected one Guest memory");
                    for memory in memories {
                        let memory = memory?;
                        ensure!(
                            !memory.memory64 && !memory.shared && memory.page_size_log2.is_none(),
                            "unsupported memory layout"
                        );
                    }
                }
                _ => {}
            }
        }
        let publication =
            publication.ok_or_else(|| anyhow::anyhow!("not a telora build artifact"))?;
        let manifest = manifest.ok_or_else(|| anyhow::anyhow!("missing manifest"))?;
        ensure!(
            publication.version == 4
                && publication.abi == telora_wasm_shared::abi::VERSION
                && manifest.abi == publication.abi,
            "unsupported publication/Guest ABI version"
        );
        ensure!(
            publication.memory_limit > 0
                && publication.initialization_fuel > 0
                && publication.request_fuel > 0,
            "invalid execution limits"
        );
        Ok(Self {
            publication,
            snapshot,
        })
    }
}
