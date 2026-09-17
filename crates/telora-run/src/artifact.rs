//! Only the execution envelope and bundled inputs are needed, not MIR/type schemas.
use anyhow::{Result, bail, ensure};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct Publication {
    pub version: u32,
    pub abi: u32,
    pub fuel: u64,
    pub memory_limit: u64,
}

#[derive(Deserialize)]
struct Manifest {
    abi: u32,
    data_modules: Vec<DataModule>,
}
#[derive(Deserialize)]
struct DataModule {
    symbol: u32,
}
#[derive(Deserialize)]
pub(crate) struct Source {
    pub id: u32,
    pub name: String,
}
#[derive(Deserialize)]
pub(crate) struct ModuleData {
    pub symbol: u32,
    pub source: Source,
    pub format: u32,
    pub text: String,
}
#[derive(Deserialize)]
struct Bundle {
    version: u32,
    modules: Vec<ModuleData>,
}

pub(crate) struct Artifact {
    pub publication: Publication,
    pub modules: Vec<ModuleData>,
}

impl Artifact {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let (mut publication, mut manifest, mut bundle) = (None, None, None);
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
                        ensure!(bundle.is_none(), "duplicate data bundle");
                        bundle = Some(serde_json::from_slice::<Bundle>(s.data())?);
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
            publication.version == 2
                && publication.abi == telora_wasm_shared::abi::VERSION
                && manifest.abi == publication.abi,
            "unsupported publication/Guest ABI version"
        );
        ensure!(
            publication.fuel > 0 && publication.memory_limit > 0,
            "invalid execution limits"
        );
        let modules = {
            let bundle = bundle.ok_or_else(|| anyhow::anyhow!("missing data bundle"))?;
            ensure!(bundle.version == 2, "unsupported data bundle");
            let mut expected: Vec<_> = manifest.data_modules.iter().map(|m| m.symbol).collect();
            let mut actual: Vec<_> = bundle.modules.iter().map(|m| m.symbol).collect();
            expected.sort_unstable();
            actual.sort_unstable();
            ensure!(
                expected == actual && !actual.windows(2).any(|p| p[0] == p[1]),
                "invalid data module membership"
            );
            let mut ids = std::collections::BTreeSet::new();
            for module in &bundle.modules {
                if module.source.id == 0
                    || !ids.insert(module.source.id)
                    || !(1..=3).contains(&module.format)
                {
                    bail!("invalid data source identity or format");
                }
            }
            bundle.modules
        };
        Ok(Self {
            publication,
            modules,
        })
    }
}
