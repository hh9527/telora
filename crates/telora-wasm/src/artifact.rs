//! Persistent metadata contains identities and positions, never source text.
use serde::{Deserialize, Serialize};
use telora_core::mir::{SealedExecutable, TypeConstructor as T, TypeState};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub abi: u32,
    pub entry_type: u32,
    pub types: Vec<TypeDesc>,
    pub sources: Vec<Source>,
    pub locations: Vec<Location>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypeDesc {
    pub kind: Kind,
    pub arguments: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Int,
    Float,
    Bool,
    Unit,
    Function,
    Unsupported,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub id: u32,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Location {
    pub source: u32,
    pub start: u32,
    pub end: u32,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

impl Manifest {
    pub(crate) fn build(executable: &SealedExecutable<'_>) -> Result<Self, String> {
        let mir = executable.sealed_mir().mir();
        let TypeState::Known(entry) = mir.ty_slots[executable.root().index()] else {
            return Err("Wasm: entry has no sealed type".into());
        };
        let sources = mir
            .sources
            .files()
            .map(|file| Source {
                id: file.id().get(),
                name: file.name.to_string(),
            })
            .collect();
        let mut positions = std::collections::BTreeSet::new();
        let mut locations = vec![];
        for node in &mir.hir {
            let loc = node.location;
            if !positions.insert((loc.source.get(), loc.start, loc.end)) {
                continue;
            }
            let file = mir.sources.get(loc.source);
            let start = file.position(loc.start);
            let end = file.position(loc.end);
            locations.push(Location {
                source: loc.source.get(),
                start: loc.start,
                end: loc.end,
                line: start.line,
                column: start.column,
                end_line: end.line,
                end_column: end.column,
            });
        }
        locations.sort_by_key(|loc| (loc.source, loc.start, loc.end));
        let types = mir
            .types
            .iter()
            .map(|ty| TypeDesc {
                kind: match ty.constructor {
                    T::Int => Kind::Int,
                    T::Float => Kind::Float,
                    T::Bool => Kind::Bool,
                    T::Tuple if ty.arguments.is_empty() => Kind::Unit,
                    T::Function => Kind::Function,
                    _ => Kind::Unsupported,
                },
                arguments: ty.arguments.iter().map(|id| id.index() as u32).collect(),
            })
            .collect();
        Ok(Self {
            abi: crate::abi::VERSION,
            entry_type: entry.index() as u32,
            types,
            sources,
            locations,
        })
    }

    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        let mut manifest = None;
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            if let wasmparser::Payload::CustomSection(section) =
                payload.map_err(|e| e.to_string())?
                && section.name() == "telora.manifest"
            {
                if manifest.is_some() {
                    return Err("Wasm: duplicate manifest".into());
                }
                manifest = Some(
                    serde_json::from_slice::<Self>(section.data()).map_err(|e| e.to_string())?,
                );
            }
        }
        let manifest = manifest.ok_or("Wasm: missing manifest")?;
        if manifest.abi != crate::abi::VERSION {
            return Err("Wasm: unsupported artifact ABI version".into());
        }
        if manifest.entry_type as usize >= manifest.types.len()
            || manifest.types.iter().any(|ty| {
                ty.arguments
                    .iter()
                    .any(|&arg| arg as usize >= manifest.types.len())
            })
        {
            return Err("Wasm: invalid manifest TypeId".into());
        }
        Ok(manifest)
    }
}
