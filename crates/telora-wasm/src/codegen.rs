use crate::{
    abi::*,
    emit,
    object::{ObjectCode, ObjectFunction},
    plan::Plan,
};
use std::borrow::Cow;
use telora_core::mir::SealedExecutable;
use wasm_encoder::*;

/// Generate a self-contained Wasm module from already sealed execution evidence.
pub fn compile_executable(executable: &SealedExecutable<'_>) -> Result<Vec<u8>, String> {
    compile(executable, Mode::Value).map(|linked| linked.bytes)
}

/// Check continues independent initialization demands, never a failed function body.
pub fn compile_check(executable: &SealedExecutable<'_>) -> Result<Vec<u8>, String> {
    compile(executable, Mode::Check).map(|linked| linked.bytes)
}

/// A service has an additional initialization phase after module evaluation.
pub fn compile_service(executable: &SealedExecutable<'_>) -> Result<Vec<u8>, String> {
    compile_service_build(executable).map(|linked| linked.bytes)
}

#[derive(Clone, Copy)]
enum Mode {
    Value,
    Check,
    Service,
}

pub(crate) fn compile_service_build(
    executable: &SealedExecutable<'_>,
) -> Result<crate::specialization::LinkedProgram, String> {
    compile(executable, Mode::Service)
}

pub(crate) fn compile_staged_build(
    executable: &SealedExecutable<'_>,
) -> Result<crate::specialization::LinkedProgram, String> {
    compile(executable, Mode::Value)
}

fn compile(
    executable: &SealedExecutable<'_>,
    mode: Mode,
) -> Result<crate::specialization::LinkedProgram, String> {
    let mut plan = Plan::new(executable)?;
    let helpers = vec![
        ("telora_initialize", 2),
        ("telora_entry", 2),
        ("telora_inject_data", BINARY_TYPE),
        ("telora_materialize_data", BINARY_TYPE),
    ];
    plan.generated_helpers = helpers.len() as u32 + plan.format_regions.functions.len() as u32;
    let mut manifest = crate::artifact::Manifest::build(executable, &plan.layouts)?;
    for (&symbol, &key) in &plan.globals {
        let mir = executable.sealed_mir().mir();
        let definition = &mir.symbols[symbol.index()];
        let module = definition
            .module
            .map(|id| mir.modules[id.index()].name.as_str())
            .unwrap_or("");
        manifest.globals.push(crate::artifact::Global {
            symbol: symbol.index() as u32,
            name: format!("{module}::{}", definition.name),
            ty: key.ty(executable.sealed_mir().mir(), key.node)?.index() as u32,
            demand: plan.demands[&key],
        });
    }
    let global_symbols = plan
        .globals
        .iter()
        .map(|(&symbol, &key)| (key, symbol))
        .collect::<std::collections::BTreeMap<_, _>>();
    for key in plan.demands.keys() {
        let mir = executable.sealed_mir().mir();
        let symbol = global_symbols.get(key).copied();
        manifest
            .initialization_roots
            .push(crate::artifact::InitializationRoot {
                node: key.node.index() as u32,
                module: mir.modules[mir.hir[key.node.index()].module.index()]
                    .name
                    .clone(),
                symbol: symbol.map(|id| id.index() as u32),
                name: symbol.map(|id| mir.symbols[id.index()].name.clone()),
                origin: mir
                    .sources
                    .get(mir.hir[key.node.index()].location.source)
                    .coordinates(mir.hir[key.node.index()].location)
                    .0,
            });
    }
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], [ValType::I32]);
    types
        .ty()
        .function([ValType::I32, ValType::I32], [ValType::I32]);
    types.ty().function([], [ValType::I32]);
    types
        .ty()
        .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]);
    types.ty().function(
        [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32],
    );
    types
        .ty()
        .function([ValType::F64, ValType::F64], [ValType::F64]);
    types.ty().function([ValType::I32; 5], [ValType::I32]);
    module.section(&types);
    let mut imports = ImportSection::new();
    for (name, ty) in [
        ("telora_alloc", 0),
        ("telora_invoke", CALL_TYPE),
        ("telora_table_push", 3),
        ("telora_table_get", BINARY_TYPE),
        ("telora_freeze", 2),
        ("telora_string_compare", BINARY_TYPE),
        ("telora_source_name", 0),
        ("telora_subject_label", 0),
        ("telora_sort_pairs", BINARY_TYPE),
        ("telora_duplicate_key_message", 0),
        ("telora_text_query", 3),
        ("telora_text_build", 4),
        ("telora_text_split", 3),
        ("telora_path", BINARY_TYPE),
        ("telora_format_render", 0),
        ("telora_format_message", BINARY_TYPE),
        ("telora_format_join", BINARY_TYPE),
        ("telora_template_prepare", 0),
        ("telora_member_message", 3),
        ("telora_regex", 3),
        ("telora_hash", 3),
        ("telora_json_write", 3),
        ("telora_json_parse", 0),
        ("telora_toml_parse", 0),
        ("telora_yaml_parse", 0),
        ("telora_float_remainder", 5),
        ("telora_source_range", 0),
        ("telora_content_write", 3),
        ("telora_content_slice", 4),
        ("telora_heap_address", 0),
        ("telora_heap_copy", 3),
        ("telora_frame_enter", BINARY_TYPE),
        ("telora_frame_leave", 2),
        ("telora_frame_tail", 3),
        ("telora_frame_slot", BINARY_TYPE),
        ("telora_roots_enter", 0),
        ("telora_roots_leave", 2),
        ("telora_roots_set", 6),
        ("telora_roots_safepoint", 2),
        ("telora_array_can_append", 0),
        ("telora_content_append", 3),
        ("telora_service_metadata_publish", BINARY_TYPE),
        ("telora_build_time_active", 2),
        ("telora_build_information_publish", BINARY_TYPE),
        ("telora_dict_key_hash", 0),
        ("telora_column_map", 0),
    ] {
        imports.import("env", name, EntityType::Function(ty));
    }
    imports.import(
        "env",
        "__indirect_function_table",
        EntityType::Table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: 0,
            maximum: None,
            shared: false,
        }),
    );
    imports.import(
        "env",
        "__linear_memory",
        EntityType::Memory(MemoryType {
            minimum: 0,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        }),
    );
    module.section(&imports);
    let mut functions = FunctionSection::new();
    for _ in &plan.functions {
        functions.function(CALL_TYPE);
    }
    let initialize = FIRST_FUNCTION + plan.functions.len() as u32;
    for &(_, ty) in &helpers {
        functions.function(ty);
    }
    for _ in &plan.format_regions.functions {
        functions.function(CALL_TYPE);
    }
    module.section(&functions);
    let heap_start = crate::compose::static_base()?
        .checked_add(
            (plan.demands.len() as u32)
                .checked_mul(DEMAND_BYTES)
                .ok_or("Wasm: demand allocation overflow")?,
        )
        .ok_or("Wasm: static memory overflow")?;
    let mut globals = GlobalSection::new();
    let mutable = GlobalType {
        val_type: ValType::I32,
        mutable: true,
        shared: false,
    };
    for _ in 0..GLOBAL_COUNT {
        globals.global(mutable, &ConstExpr::i32_const(0));
    }
    module.section(&globals);
    let mut elements = ElementSection::new();
    elements.active(
        Some(0),
        &ConstExpr::i32_const(1),
        Elements::Functions(Cow::Owned(
            (FIRST_FUNCTION..initialize)
                .chain(plan.format_regions.functions.values().copied())
                .collect(),
        )),
    );
    module.section(&elements);
    let count = initialize + plan.generated_helpers;
    let mut code = ObjectCode::default();
    for &key in plan.functions.keys() {
        code.function(emit::compile(executable.sealed_mir().mir(), &plan, key)?);
    }
    let mut init = Function::new([]);
    for instruction in [
        Instruction::GlobalGet(PHASE_GLOBAL),
        Instruction::I32Const(2),
        Instruction::I32Eq,
        Instruction::If(BlockType::Empty),
        Instruction::I32Const(1),
        Instruction::Return,
        Instruction::End,
        Instruction::GlobalGet(PHASE_GLOBAL),
        Instruction::If(BlockType::Empty),
        Instruction::I32Const(0),
        Instruction::Return,
        Instruction::End,
        Instruction::I32Const(1),
        Instruction::GlobalSet(PHASE_GLOBAL),
    ] {
        init.instruction(&instruction);
    }
    for (index, key) in plan.demands.keys().enumerate() {
        for instruction in [
            Instruction::I32Const((index + 1) as i32),
            Instruction::GlobalSet(INITIALIZATION_ROOT_GLOBAL),
            Instruction::I32Const(0),
            Instruction::I32Const(0),
            Instruction::I32Const(0),
            Instruction::Call(plan.functions[key]),
            Instruction::I32Const(0),
            Instruction::GlobalSet(INITIALIZATION_ROOT_GLOBAL),
        ] {
            init.instruction(&instruction);
        }
        if matches!(mode, Mode::Check) {
            init.instruction(&Instruction::Drop);
            continue;
        }
        for instruction in [
            Instruction::I32Eqz,
            Instruction::If(BlockType::Empty),
            Instruction::I32Const(0),
            Instruction::Return,
            Instruction::End,
        ] {
            init.instruction(&instruction);
        }
    }
    // Failed demands retain their root diagnostic. Never freeze or publish a
    // session with errors, even when its final independent demand succeeded.
    for instruction in [
        Instruction::GlobalGet(PHASE_GLOBAL),
        Instruction::I32Const(3),
        Instruction::I32Eq,
        Instruction::If(BlockType::Empty),
        Instruction::I32Const(0),
        Instruction::Return,
        Instruction::End,
    ] {
        init.instruction(&instruction);
    }
    if !matches!(mode, Mode::Service) {
        init.instruction(&Instruction::Call(FREEZE))
            .instruction(&Instruction::Drop);
    }
    init.instruction(&Instruction::I32Const(2))
        .instruction(&Instruction::GlobalSet(PHASE_GLOBAL))
        .instruction(&Instruction::I32Const(1))
        .instruction(&Instruction::End);
    code.function(ObjectFunction::relocate(&init, count, 0)?);
    let mut root = Function::new([]);
    for instruction in [
        Instruction::GlobalGet(PHASE_GLOBAL),
        Instruction::I32Const(2),
        Instruction::I32Ne,
        Instruction::If(BlockType::Empty),
        Instruction::I32Const(0),
        Instruction::Return,
        Instruction::End,
    ] {
        root.instruction(&instruction);
    }
    root.instruction(&Instruction::I32Const(0))
        .instruction(&Instruction::I32Const(0))
        .instruction(&Instruction::I32Const(0))
        .instruction(&Instruction::Call(plan.functions[&plan.root]))
        .instruction(&Instruction::End);
    code.function(ObjectFunction::relocate(&root, count, 0)?);
    code.function(ObjectFunction::relocate(
        &crate::data_input::injector(&plan, &manifest),
        count,
        2,
    )?);
    code.function(crate::data_parse_ops::materializer(
        executable.sealed_mir().mir(),
        &plan,
        manifest.value_type,
    )?);
    for &key in plan.format_regions.functions.keys() {
        code.function(emit::compile_writer(
            executable.sealed_mir().mir(),
            &plan,
            key,
        )?);
    }
    let (code, relocations) = code.finish(5);
    module.section(&code);
    let mut static_image = plan.reflection.clone();
    static_image.extend_from_slice(&plan.origins.borrow().bytes);
    if !static_image.is_empty() {
        let mut data = DataSection::new();
        data.active(0, &ConstExpr::i32_const(0), static_image.iter().copied());
        module.section(&data);
    }
    let mut symbols = SymbolTable::new();
    let names = crate::function_names::FunctionNames::new(executable.sealed_mir().mir());
    let function_keys: Vec<_> = plan.functions.keys().copied().collect();
    let mut function_names = NameMap::new();
    for index in 0..FIRST_FUNCTION {
        symbols.function(SymbolTable::WASM_SYM_UNDEFINED, index, None);
    }
    for index in FIRST_FUNCTION..count {
        let name = if index >= initialize + helpers.len() as u32 {
            let key = *plan
                .format_regions
                .functions
                .keys()
                .nth((index - initialize - helpers.len() as u32) as usize)
                .unwrap();
            format!("{}_writer", names.name(key, index))
        } else if index >= initialize {
            helpers[(index - initialize) as usize].0.to_owned()
        } else {
            names.name(function_keys[(index - FIRST_FUNCTION) as usize], index)
        };
        symbols.function(0, index, Some(&name));
        function_names.append(index, &name);
    }
    for (index, name) in ["telora_error", "telora_phase", "telora_initialization_root"]
        .iter()
        .enumerate()
    {
        symbols.global(0, index as u32, Some(name));
    }
    symbols.table(SymbolTable::WASM_SYM_UNDEFINED, 0, None);
    if !static_image.is_empty() {
        symbols.data(
            0,
            "telora_type_image",
            Some(DataSymbolDefinition {
                index: 0,
                offset: 0,
                size: static_image.len() as u32,
            }),
        );
        // wasm-encoder does not yet expose the segment-info subsection.
        let mut linking = vec![];
        2u32.encode(&mut linking);
        let mut segment = vec![];
        1u32.encode(&mut segment);
        ".rodata.telora.types".encode(&mut segment);
        3u32.encode(&mut segment);
        0u32.encode(&mut segment);
        linking.push(5);
        (segment.len() as u32).encode(&mut linking);
        linking.extend(segment);
        symbols.encode(&mut linking);
        module.section(&CustomSection {
            name: "linking".into(),
            data: linking.into(),
        });
    } else {
        module.section(LinkingSection::new().symbol_table(&symbols));
    }
    module.section(&relocations);
    let mut name_section = NameSection::new();
    name_section.functions(&function_names);
    module.section(&name_section);
    module.section(&CustomSection {
        name: Cow::Borrowed("telora.abi"),
        data: Cow::Owned(VERSION.to_le_bytes().to_vec()),
    });
    module.section(&CustomSection {
        name: Cow::Borrowed("telora.tooling"),
        data: Cow::Owned(serde_json::to_vec(&manifest).map_err(|e| e.to_string())?),
    });
    let service = if matches!(mode, Mode::Service) {
        Some(crate::service_abi::contract(
            &manifest,
            initialize - FIRST_FUNCTION,
        )?)
    } else {
        None
    };
    let ready_demands = plan
        .demands
        .iter()
        .enumerate()
        .map(|(index, (&key, &address))| {
            let ty = key
                .result_type(executable.sealed_mir().mir())?
                .ok_or("Wasm: demand has no result")?;
            let width = match &plan.layouts[ty.index()].layout {
                telora_core::candidate_layout::State::Known { shape } => {
                    u32::try_from(shape.value_bytes).map_err(|_| "Wasm: demand too wide")?
                }
                telora_core::candidate_layout::State::Uninhabited { .. } => 0,
                _ => return Err("Wasm: demand has no closed layout".into()),
            };
            Ok(crate::specialization::Demand {
                index: index as u32,
                function: plan.functions[&key] - FIRST_FUNCTION,
                address,
                width,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    crate::compose::link(
        &module.finish(),
        heap_start,
        &manifest.sources,
        &manifest.types,
        &plan
            .demands
            .keys()
            .map(|key| {
                key.ty(executable.sealed_mir().mir(), key.node)
                    .map(|ty| ty.index() as u32)
            })
            .collect::<Result<Vec<_>, _>>()?,
        ready_demands,
        service,
        &helpers
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (*name, initialize - FIRST_FUNCTION + i as u32))
            .collect::<Vec<_>>(),
    )
}
