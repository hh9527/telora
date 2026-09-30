//! Compile data-module values into a passive Wasm data segment, never source text.
use crate::{artifact::Manifest, session::Session, template::Parts};
use telora_core::{SourceDatabase, SourceId, data_plan::Format};
use wasm_encoder::{
    CodeSection, ConstExpr, DataCountSection, DataSection, Encode, Function, FunctionSection,
    Instruction as I, TypeSection, ValType,
};

const SECTION: &str = "telora.modules";

pub fn build(
    bytes: &[u8],
    sources: &SourceDatabase,
    plans: &[(u32, SourceId, Format)],
) -> Result<Vec<u8>, String> {
    build_with_limits(bytes, sources, plans, 100_000_000, 64 * 1024 * 1024)
}

pub fn build_with_limits(
    bytes: &[u8],
    sources: &SourceDatabase,
    plans: &[(u32, SourceId, Format)],
    fuel: u64,
    memory: usize,
) -> Result<Vec<u8>, String> {
    let mut parts = Parts::read(bytes)?;
    if parts.custom.iter().any(|(name, _)| name == SECTION) {
        return Err("Wasm: data modules are already compiled".into());
    }
    let manifest = Manifest::read(bytes)?;
    let mut expected: Vec<_> = manifest
        .data_modules
        .iter()
        .map(|module| module.symbol)
        .collect();
    let mut actual: Vec<_> = plans.iter().map(|module| module.0).collect();
    expected.sort_unstable();
    actual.sort_unstable();
    if expected != actual || actual.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("Wasm: compile each data module exactly once".into());
    }
    if plans.is_empty() {
        return Ok(bytes.to_vec());
    }
    let mut session = Session::load_with_limits(bytes, fuel, memory)?;
    for &(symbol, source, format) in plans {
        let value = session
            .parse_data_source(sources.get(source), format)?
            .map_err(|diagnostics| diagnostics.to_string())?;
        session.inject_data_value(symbol, value)?;
    }
    let alloc = session.exports.alloc;
    let result = alloc
        .call(&mut session.store, (8, 4))
        .map_err(|e| e.to_string())?;
    session
        .instance
        .get_typed_func::<u32, ()>(&session.store, "telora_data_image_export")
        .map_err(|e| e.to_string())?
        .call(&mut session.store, result)
        .map_err(|e| e.to_string())?;
    let raw = session.memory.data(&session.store);
    let word = |offset| {
        u32::from_le_bytes(
            raw[result as usize + offset..result as usize + offset + 4]
                .try_into()
                .unwrap(),
        )
    };
    let pointer = word(0) as usize;
    let length = word(4) as usize;
    let image = raw
        .get(
            pointer
                ..pointer
                    .checked_add(length)
                    .ok_or("Wasm: data image overflow")?,
        )
        .ok_or("Wasm: invalid data image range")?
        .to_vec();
    let metadata = serde_json::to_vec(&session.manifest).map_err(|e| e.to_string())?;
    for (name, data) in &mut parts.custom {
        if name == "telora.tooling" {
            *data = metadata.clone();
        }
    }
    let mut exports = std::collections::BTreeMap::new();
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        if let wasmparser::Payload::ExportSection(reader) = payload.map_err(|e| e.to_string())? {
            for export in reader {
                let export = export.map_err(|e| e.to_string())?;
                if export.kind == wasmparser::ExternalKind::Func {
                    exports.insert(export.name.to_owned(), export.index);
                }
            }
        }
    }
    let function = |name: &str| {
        exports
            .get(name)
            .copied()
            .ok_or_else(|| format!("Wasm: missing {name}"))
    };
    let original_start =
        wasmparser::BinaryReader::new(parts.sections.get(&8).ok_or("Wasm: missing start")?, 0)
            .read_var_u32()
            .map_err(|e| e.to_string())?;
    let index = parts.count(11)?;
    let mut data = DataSection::new();
    data.passive(image);
    parts.append_section(&data)?;
    parts.sections.insert(
        12,
        crate::template::payload(&DataCountSection { count: index + 1 }),
    );
    let ty = parts.count(1)?;
    let mut types = TypeSection::new();
    types.ty().function([], []);
    parts.append_section(&types)?;
    let start = parts.count(3)?;
    let mut functions = FunctionSection::new();
    functions.function(ty);
    parts.append_section(&functions)?;
    let mut boot = Function::new([(1, ValType::I32)]);
    for instruction in [
        I::Call(original_start),
        I::I32Const(length as i32),
        I::I32Const(1),
        I::Call(function("mem-alloc")?),
        I::LocalSet(0),
        I::LocalGet(0),
        I::I32Const(0),
        I::I32Const(length as i32),
        I::MemoryInit {
            mem: 0,
            data_index: index,
        },
        I::DataDrop(index),
        I::LocalGet(0),
        I::I32Const(length as i32),
        I::Call(function("telora_data_image_import")?),
        I::LocalGet(0),
        I::I32Const(length as i32),
        I::I32Const(1),
        I::Call(function("mem-free")?),
        I::End,
    ] {
        boot.instruction(&instruction);
    }
    let mut code = CodeSection::new();
    code.function(&boot);
    parts.append_section(&code)?;
    let mut start_section = Vec::new();
    start.encode(&mut start_section);
    parts.sections.insert(8, start_section);
    let mut marker = Vec::new();
    original_start.encode(&mut marker);
    parts.custom.push((SECTION.into(), marker));
    Ok(parts.module())
}

/// A ready snapshot supersedes the module-data image; keep its static addresses.
pub(crate) fn remove_image(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut parts = Parts::read(bytes)?;
    let Some((_, marker)) = parts.custom.iter().find(|(name, _)| name == SECTION) else {
        return Ok(bytes.to_vec());
    };
    let start = wasmparser::BinaryReader::new(marker, 0)
        .read_var_u32()
        .map_err(|e| e.to_string())?;
    let data_count = parts.count(11)?;
    let function_count = parts.count(3)?;
    let mut data = DataSection::new();
    let mut code = CodeSection::new();
    let mut at = 0;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        match payload.map_err(|e| e.to_string())? {
            wasmparser::Payload::DataSection(reader) => {
                for (index, segment) in reader.into_iter().enumerate() {
                    let segment = segment.map_err(|e| e.to_string())?;
                    if index + 1 == data_count as usize {
                        continue;
                    }
                    match segment.kind {
                        wasmparser::DataKind::Active {
                            memory_index,
                            offset_expr,
                        } => {
                            let mut reader = offset_expr.get_operators_reader();
                            let wasmparser::Operator::I32Const { value } =
                                reader.read().map_err(|e| e.to_string())?
                            else {
                                return Err("Wasm: unsupported data offset".into());
                            };
                            data.active(
                                memory_index,
                                &ConstExpr::i32_const(value),
                                segment.data.iter().copied(),
                            );
                        }
                        wasmparser::DataKind::Passive => {
                            return Err("Wasm: unexpected passive data".into());
                        }
                    }
                }
            }
            wasmparser::Payload::CodeSectionEntry(body) => {
                at += 1;
                if at == function_count {
                    let mut empty = Function::new([]);
                    empty.instruction(&I::Call(start)).instruction(&I::End);
                    code.function(&empty);
                } else {
                    code.raw(body.as_bytes());
                }
            }
            _ => {}
        }
    }
    parts.sections.insert(11, crate::template::payload(&data));
    parts.sections.insert(10, crate::template::payload(&code));
    parts.sections.remove(&12);
    let mut start_section = Vec::new();
    start.encode(&mut start_section);
    parts.sections.insert(8, start_section);
    parts.custom.retain(|(name, _)| name != SECTION);
    Ok(parts.module())
}
