//! Prelinked RT indices never move. Only generated program indices are assigned.
use std::{collections::BTreeMap, sync::OnceLock};
use wasm_encoder::{Encode, RawSection, Section};

pub(crate) struct Parts {
    pub sections: BTreeMap<u8, Vec<u8>>,
    pub custom: Vec<(String, Vec<u8>)>,
}
impl Parts {
    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        if bytes.get(..8) != Some(&[0, 97, 115, 109, 1, 0, 0, 0]) {
            return Err("Wasm: invalid module header".into());
        }
        let mut reader = wasmparser::BinaryReader::new(&bytes[8..], 8);
        let mut parts = Self {
            sections: BTreeMap::new(),
            custom: vec![],
        };
        while !reader.eof() {
            let id = reader.read_u8().map_err(|e| e.to_string())?;
            let size = reader.read_var_u32().map_err(|e| e.to_string())? as usize;
            let payload = reader.read_bytes(size).map_err(|e| e.to_string())?;
            if id == 0 {
                let mut r = wasmparser::BinaryReader::new(payload, 0);
                let name = r.read_string().map_err(|e| e.to_string())?.to_owned();
                parts
                    .custom
                    .push((name, payload[r.original_position() as usize..].to_vec()));
            } else if parts.sections.insert(id, payload.to_vec()).is_some() {
                return Err("Wasm: duplicate section".into());
            }
        }
        Ok(parts)
    }
    pub fn count(&self, id: u8) -> Result<u32, String> {
        self.sections
            .get(&id)
            .map(|bytes| split(bytes).map(|x| x.0))
            .unwrap_or(Ok(0))
    }
    pub fn append(&mut self, id: u8, bytes: &[u8]) -> Result<(), String> {
        let (added, body) = split(bytes)?;
        let (count, old) = self
            .sections
            .get(&id)
            .map(|b| split(b))
            .transpose()?
            .unwrap_or((0, &[]));
        let mut out = vec![];
        count
            .checked_add(added)
            .ok_or("Wasm: section count overflow")?
            .encode(&mut out);
        out.extend_from_slice(old);
        out.extend_from_slice(body);
        self.sections.insert(id, out);
        Ok(())
    }
    pub fn append_section(&mut self, section: &impl Section) -> Result<(), String> {
        self.append(section.id(), &payload(section))
    }
    pub fn module(self) -> Vec<u8> {
        let mut module = wasm_encoder::Module::new();
        for id in [1, 2, 3, 4, 5, 6, 7, 8, 9, 12, 10, 11] {
            if let Some(data) = self.sections.get(&id) {
                module.section(&RawSection { id, data });
            }
        }
        for (name, data) in self.custom {
            module.section(&wasm_encoder::CustomSection {
                name: name.into(),
                data: data.into(),
            });
        }
        module.finish()
    }
}
pub(crate) fn split(bytes: &[u8]) -> Result<(u32, &[u8]), String> {
    let mut reader = wasmparser::BinaryReader::new(bytes, 0);
    let count = reader.read_var_u32().map_err(|e| e.to_string())?;
    Ok((count, &bytes[reader.original_position() as usize..]))
}
pub(crate) fn payload(section: &impl Section) -> Vec<u8> {
    let mut bytes = vec![];
    section.encode(&mut bytes);
    let mut r = wasmparser::BinaryReader::new(&bytes, 0);
    r.read_var_u32().unwrap();
    bytes[r.original_position() as usize..].to_vec()
}

pub(crate) struct Runtime {
    pub bytes: &'static [u8],
    pub functions: u32,
    pub types: u32,
    pub globals: u32,
    pub table: wasmparser::TableType,
    pub memory: wasmparser::MemoryType,
    pub heap_base: u32,
    pub exports: BTreeMap<String, u32>,
    pub names: BTreeMap<u32, String>,
}
impl Runtime {
    fn load() -> Result<Self, String> {
        let bytes: &'static [u8] = include_bytes!(concat!(env!("OUT_DIR"), "/telora-rt.wasm"));
        let parts = Parts::read(bytes)?;
        if parts.count(2)? != 0 || parts.sections.contains_key(&8) {
            return Err("Wasm: runtime template must have no imports or start".into());
        }
        let mut exports = BTreeMap::new();
        let mut heap_global = None;
        let mut globals = vec![];
        let mut table = None;
        let mut memory = None;
        let mut names = BTreeMap::new();
        for part in wasmparser::Parser::new(0).parse_all(bytes) {
            match part.map_err(|e| e.to_string())? {
                wasmparser::Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export.map_err(|e| e.to_string())?;
                        if export.kind == wasmparser::ExternalKind::Func {
                            exports.insert(export.name.into(), export.index);
                        }
                        if export.name == "__heap_base" {
                            heap_global = Some(export.index);
                        }
                    }
                }
                wasmparser::Payload::GlobalSection(reader) => {
                    for global in reader {
                        let global = global.map_err(|e| e.to_string())?;
                        let value = match global
                            .init_expr
                            .get_operators_reader()
                            .read()
                            .map_err(|e| e.to_string())?
                        {
                            wasmparser::Operator::I32Const { value } => Some(value as u32),
                            _ => None,
                        };
                        globals.push(value);
                    }
                }
                wasmparser::Payload::TableSection(reader) => {
                    for item in reader {
                        if table.replace(item.map_err(|e| e.to_string())?.ty).is_some() {
                            return Err("Wasm: runtime must have one table".into());
                        }
                    }
                }
                wasmparser::Payload::MemorySection(reader) => {
                    for item in reader {
                        if memory.replace(item.map_err(|e| e.to_string())?).is_some() {
                            return Err("Wasm: runtime must have one memory".into());
                        }
                    }
                }
                wasmparser::Payload::CustomSection(section) if section.name() == "name" => {
                    names = read_names(section.data())?;
                }
                _ => {}
            }
        }
        let heap_base = heap_global
            .and_then(|i| globals.get(i as usize).copied().flatten())
            .ok_or("Wasm: template lacks fixed heap base")?;
        Ok(Self {
            bytes,
            functions: parts.count(3)?,
            types: parts.count(1)?,
            globals: globals.len() as u32,
            table: table.ok_or("Wasm: template lacks function table")?,
            memory: memory.ok_or("Wasm: template lacks memory")?,
            heap_base,
            exports,
            names,
        })
    }
}
pub(crate) fn read_names(bytes: &[u8]) -> Result<BTreeMap<u32, String>, String> {
    let mut names = BTreeMap::new();
    for subsection in wasmparser::NameSectionReader::new(wasmparser::BinaryReader::new(bytes, 0)) {
        if let wasmparser::Name::Function(reader) = subsection.map_err(|e| e.to_string())? {
            for name in reader {
                let name = name.map_err(|e| e.to_string())?;
                names.insert(name.index, name.name.into());
            }
        }
    }
    Ok(names)
}
pub(crate) fn runtime() -> Result<&'static Runtime, String> {
    static RT: OnceLock<Result<Runtime, String>> = OnceLock::new();
    RT.get_or_init(Runtime::load).as_ref().map_err(Clone::clone)
}
