//! In-memory append to prelinked RT; handles only our fixed relocation vocabulary.
use crate::{
    abi::FIRST_FUNCTION,
    template::{self, Parts},
};
use wasm_encoder::*;
mod trace_image;

pub(crate) fn static_base() -> Result<u32, String> {
    Ok(template::runtime()?.heap_base)
}

pub(crate) fn link(
    object: &[u8],
    reserved_bytes: u32,
    sources: &[crate::artifact::Source],
    types: &[crate::artifact::TypeDesc],
    demands: &[u32],
    function_demands: &[Vec<u32>],
    service: Option<telora_wasm_shared::service::Contract>,
    generated_exports: &[(&str, u32)],
) -> Result<Vec<u8>, String> {
    let rt = template::runtime()?;
    let mut program = Parts::read(object)?;
    let mut output = Parts::read(rt.bytes)?;
    let generated = program.count(3)?;
    let type_count = program.count(1)?;
    let table_base = u32::try_from(rt.table.initial).map_err(|_| "Wasm: table exceeds wasm32")?;
    let image_base = reserved_bytes
        .checked_add(7)
        .ok_or("Wasm: static overflow")?
        & !7;
    if image_base < rt.heap_base {
        return Err("Wasm: program overlaps runtime static storage".into());
    }
    let mut imports = vec![];
    let mut data = vec![];
    for part in wasmparser::Parser::new(0).parse_all(object) {
        match part.map_err(|e| e.to_string())? {
            wasmparser::Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(|e| e.to_string())?;
                    if let wasmparser::TypeRef::Func(_) = import.ty {
                        imports.push(
                            *rt.exports
                                .get(import.name)
                                .ok_or_else(|| format!("Wasm: RT lacks {}", import.name))?,
                        );
                    }
                }
            }
            wasmparser::Payload::DataSection(reader) => {
                for segment in reader {
                    let segment = segment.map_err(|e| e.to_string())?;
                    if !data.is_empty() {
                        return Err("Wasm: expected one generated type image".into());
                    }
                    data.extend_from_slice(segment.data);
                }
            }
            _ => {}
        }
    }
    while data.len() % 8 != 0 {
        data.push(0);
    }
    let trace_base = image_base
        .checked_add(u32::try_from(data.len()).map_err(|_| "Wasm: trace image overflow")?)
        .ok_or("Wasm: trace address overflow")?;
    data.extend_from_slice(&trace_image::encode(types)?);
    while data.len() % 4 != 0 {
        data.push(0);
    }
    let function_demands_base = image_base
        .checked_add(u32::try_from(data.len()).map_err(|_| "Wasm: dependency image overflow")?)
        .ok_or("Wasm: dependency address overflow")?;
    let header_bytes = u32::try_from(function_demands.len())
        .ok()
        .and_then(|count| count.checked_mul(8))
        .ok_or("Wasm: dependency header overflow")?;
    let mut roots_pointer = function_demands_base
        .checked_add(header_bytes)
        .ok_or("Wasm: dependency root address overflow")?;
    for roots in function_demands {
        data.extend_from_slice(&roots_pointer.to_le_bytes());
        let count = u32::try_from(roots.len()).map_err(|_| "Wasm: too many function roots")?;
        data.extend_from_slice(&count.to_le_bytes());
        roots_pointer = roots_pointer
            .checked_add(
                count
                    .checked_mul(4)
                    .ok_or("Wasm: dependency roots overflow")?,
            )
            .ok_or("Wasm: dependency root address overflow")?;
    }
    for roots in function_demands {
        for root in roots {
            data.extend_from_slice(&root.to_le_bytes());
        }
    }
    let mut source_names = Vec::new();
    for source in sources {
        while data.len() % 8 != 0 {
            data.push(0);
        }
        let pointer = image_base
            .checked_add(u32::try_from(data.len()).map_err(|_| "Wasm: source name image overflow")?)
            .ok_or("Wasm: source name address overflow")?;
        let length = u32::try_from(source.name.len()).map_err(|_| "Wasm: source name too long")?;
        data.extend_from_slice(source.name.as_bytes());
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let index_pointer = image_base
            .checked_add(u32::try_from(data.len()).map_err(|_| "Wasm: source index overflow")?)
            .ok_or("Wasm: source index address overflow")?;
        let count = u32::try_from(source.lines.len()).map_err(|_| "Wasm: too many source lines")?;
        for range in &source.lines {
            for word in range {
                data.extend_from_slice(&word.to_le_bytes());
            }
        }
        source_names.push((source.id, pointer, length, index_pointer, count));
    }
    let service_base = if let Some(mut contract) = service {
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let base = image_base
            .checked_add(u32::try_from(data.len()).map_err(|_| "Wasm: service image too large")?)
            .ok_or("Wasm: service image overflow")?;
        for slot in [
            &mut contract.initialize,
            &mut contract.entry,
            &mut contract.materialize,
        ] {
            if *slot >= generated {
                return Err("Wasm: invalid service callback".into());
            }
            *slot = table_base
                .checked_add(*slot)
                .ok_or("Wasm: service callback overflow")?;
        }
        for word in contract.words() {
            data.extend_from_slice(&word.to_le_bytes());
        }
        Some(base)
    } else {
        None
    };
    if imports.len() != FIRST_FUNCTION as usize {
        return Err("Wasm: generated RT import contract changed".into());
    }
    let function = |index: u32| -> Result<u32, String> {
        if index < FIRST_FUNCTION {
            Ok(imports[index as usize])
        } else if index - FIRST_FUNCTION < generated {
            rt.functions
                .checked_add(index - FIRST_FUNCTION)
                .ok_or("Wasm: function overflow".into())
        } else {
            Err("Wasm: invalid generated function".into())
        }
    };
    let relocations = program
        .custom
        .iter()
        .find(|(n, _)| n == "reloc.CODE")
        .ok_or("Wasm: generated code lacks relocations")?
        .1
        .clone();
    let code = program
        .sections
        .get_mut(&10)
        .ok_or("Wasm: generated code missing")?;
    let mut reader = wasmparser::BinaryReader::new(&relocations, 0);
    reader.read_var_u32().map_err(|e| e.to_string())?;
    let count = reader.read_var_u32().map_err(|e| e.to_string())?;
    for _ in 0..count {
        let kind = reader.read_u8().map_err(|e| e.to_string())?;
        let offset = reader.read_var_u32().map_err(|e| e.to_string())? as usize;
        let symbol = reader.read_var_u32().map_err(|e| e.to_string())?;
        let (value, signed) = match kind {
            0 => (function(symbol)?, false),
            1 => (
                table_base
                    .checked_add(
                        symbol
                            .checked_sub(FIRST_FUNCTION)
                            .ok_or("Wasm: non-program function pointer")?,
                    )
                    .ok_or("Wasm: table overflow")?,
                true,
            ),
            4 => {
                let addend = reader.read_var_i32().map_err(|e| e.to_string())?;
                if addend != 0 {
                    return Err("Wasm: unexpected image addend".into());
                }
                (image_base, true)
            }
            6 => (
                rt.types.checked_add(symbol).ok_or("Wasm: type overflow")?,
                false,
            ),
            7 => (
                rt.globals
                    .checked_add(
                        symbol
                            .checked_sub(FIRST_FUNCTION + generated)
                            .ok_or("Wasm: invalid global relocation")?,
                    )
                    .ok_or("Wasm: global overflow")?,
                false,
            ),
            20 => (0, false),
            _ => return Err(format!("Wasm: unsupported generated relocation {kind}")),
        };
        let target = code
            .get_mut(offset..offset + 5)
            .ok_or("Wasm: relocation outside code")?;
        let mut value = if signed {
            value as i32 as i64
        } else {
            value as i64
        };
        for (index, byte) in target.iter_mut().enumerate() {
            *byte = (value as u8 & 127) | if index < 4 { 128 } else { 0 };
            value >>= 7;
        }
    }
    output.append(
        1,
        program
            .sections
            .get(&1)
            .ok_or("Wasm: generated types missing")?,
    )?;
    let mut boot_type = TypeSection::new();
    boot_type.ty().function([], []);
    output.append_section(&boot_type)?;
    let mut funcs = FunctionSection::new();
    let mut reader = wasmparser::BinaryReader::new(program.sections.get(&3).unwrap(), 0);
    reader.read_var_u32().map_err(|e| e.to_string())?;
    for _ in 0..generated {
        funcs.function(rt.types + reader.read_var_u32().map_err(|e| e.to_string())?);
    }
    funcs.function(rt.types + type_count);
    output.append_section(&funcs)?;
    output.append(
        6,
        program
            .sections
            .get(&6)
            .ok_or("Wasm: generated globals missing")?,
    )?;
    let mut elements = ElementSection::new();
    elements.active(
        Some(0),
        &ConstExpr::i32_const(table_base as i32),
        Elements::Functions(std::borrow::Cow::Owned(
            (rt.functions..rt.functions + generated).collect(),
        )),
    );
    output.append_section(&elements)?;
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: rt.table.initial + generated as u64,
        maximum: rt.table.maximum.map(|n| n + generated as u64),
        shared: false,
    });
    output.sections.insert(4, template::payload(&tables));
    if !data.is_empty() {
        let mut section = DataSection::new();
        section.active(
            0,
            &ConstExpr::i32_const(image_base as i32),
            data.iter().copied(),
        );
        output.append_section(&section)?;
    }
    let heap = image_base
        .checked_add(u32::try_from(data.len()).map_err(|_| "Wasm: image overflow")?)
        .and_then(|n| n.checked_add(7))
        .ok_or("Wasm: heap overflow")?
        & !7;
    let mut memory = MemorySection::new();
    memory.memory(MemoryType {
        minimum: rt.memory.initial.max((heap as u64).div_ceil(65536)),
        maximum: rt.memory.maximum,
        memory64: false,
        shared: false,
        page_size_log2: rt.memory.page_size_log2,
    });
    output.sections.insert(5, template::payload(&memory));
    output.append(10, program.sections.get(&10).unwrap())?;
    let mut boot = Function::new([]);
    boot.instruction(&Instruction::I32Const(heap as i32))
        .instruction(&Instruction::Call(
            *rt.exports
                .get("telora_reserve_static")
                .ok_or("Wasm: template lacks heap initializer")?,
        ));
    boot.instruction(&Instruction::I32Const(trace_base as i32))
        .instruction(&Instruction::I32Const(rt.heap_base as i32))
        .instruction(&Instruction::I32Const(demands.len() as i32))
        .instruction(&Instruction::Call(
            *rt.exports
                .get("telora_collection_bootstrap")
                .ok_or("Wasm: missing collection bootstrap")?,
        ));
    boot.instruction(&Instruction::I32Const(function_demands_base as i32))
        .instruction(&Instruction::I32Const(table_base as i32))
        .instruction(&Instruction::I32Const(function_demands.len() as i32))
        .instruction(&Instruction::Call(
            *rt.exports
                .get("telora_function_dependencies_bootstrap")
                .ok_or("Wasm: missing function dependency bootstrap")?,
        ));
    for (index, ty) in demands.iter().copied().enumerate() {
        let offset = rt.heap_base + index as u32 * crate::abi::DEMAND_BYTES + 8;
        boot.instruction(&Instruction::I32Const(offset as i32))
            .instruction(&Instruction::I32Const(ty as i32))
            .instruction(&Instruction::I32Store(MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
    }
    for (id, pointer, length, index_pointer, count) in source_names {
        boot.instruction(&Instruction::I32Const(id as i32))
            .instruction(&Instruction::I32Const(pointer as i32))
            .instruction(&Instruction::I32Const(length as i32))
            .instruction(&Instruction::Call(
                *rt.exports
                    .get("telora_register_source")
                    .ok_or("Wasm: missing source registry")?,
            ))
            .instruction(&Instruction::Drop);
        boot.instruction(&Instruction::I32Const(id as i32))
            .instruction(&Instruction::I32Const(index_pointer as i32))
            .instruction(&Instruction::I32Const(count as i32))
            .instruction(&Instruction::Call(
                *rt.exports
                    .get("telora_static_source_index")
                    .ok_or("Wasm: missing source index registration")?,
            ));
    }
    if let Some(base) = service_base {
        boot.instruction(&Instruction::I32Const(base as i32))
            .instruction(&Instruction::Call(
                *rt.exports
                    .get("telora_service_bootstrap")
                    .ok_or("Wasm: missing service bootstrap")?,
            ));
    }
    boot.instruction(&Instruction::End);
    let mut boot_code = CodeSection::new();
    boot_code.function(&boot);
    output.append_section(&boot_code)?;
    let mut start = vec![];
    (rt.functions + generated).encode(&mut start);
    output.sections.insert(8, start);
    let mut exports = ExportSection::new();
    for &(name, index) in generated_exports {
        if index >= generated {
            return Err("Wasm: invalid generated export index".into());
        }
        exports.export(name, ExportKind::Func, rt.functions + index);
    }
    exports.export("telora_error", ExportKind::Global, rt.globals);
    exports.export("telora_phase", ExportKind::Global, rt.globals + 1);
    exports.export(
        "telora_initialization_root",
        ExportKind::Global,
        rt.globals + 2,
    );
    // Reset restores every global, including the Rust stack pointer. Values
    // live in linear memory; function tables are fixed by this linker.
    for index in 0..rt.globals + crate::abi::GLOBAL_COUNT {
        exports.export(
            &format!("telora_reset_global_{index}"),
            ExportKind::Global,
            index,
        );
    }
    output.append_section(&exports)?;
    let mut names = rt.names.clone();
    for (name, bytes) in &program.custom {
        if name == "name" {
            for (index, name) in template::read_names(bytes)? {
                names.insert(function(index)?, name);
            }
        }
    }
    names.insert(rt.functions + generated, "telora_bootstrap".into());
    let mut map = NameMap::new();
    for (index, name) in names {
        map.append(index, &name);
    }
    let mut section = NameSection::new();
    section.functions(&map);
    output
        .custom
        .retain(|(name, _)| name != "name" && !name.starts_with(".debug"));
    let bytes = template::payload(&section);
    let mut r = wasmparser::BinaryReader::new(&bytes, 0);
    r.read_string().map_err(|e| e.to_string())?;
    output.custom.push((
        "name".into(),
        bytes[r.original_position() as usize..].to_vec(),
    ));
    output.custom.extend(
        program
            .custom
            .into_iter()
            .filter(|(name, _)| name.starts_with("telora.")),
    );
    Ok(output.module())
}
