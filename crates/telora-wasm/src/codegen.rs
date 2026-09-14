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
    compile(executable, false)
}

/// Check continues independent initialization demands, never a failed function body.
pub fn compile_check(executable: &SealedExecutable<'_>) -> Result<Vec<u8>, String> {
    compile(executable, true)
}

fn compile(executable: &SealedExecutable<'_>, check: bool) -> Result<Vec<u8>, String> {
    let plan = Plan::new(executable)?;
    let mut manifest = crate::artifact::Manifest::build(executable, &plan.layouts)?;
    for (&symbol, &key) in &plan.globals {
        let mir = executable.sealed_mir().mir();
        let definition = &mir.symbols[symbol.index()];
        let module = definition.module.map(|id| mir.modules[id.index()].name.as_str()).unwrap_or("");
        manifest.globals.push(crate::artifact::Global {
            symbol: symbol.index() as u32,
            name: format!("{module}::{}", definition.name),
            ty: key.ty(executable.sealed_mir().mir(), key.node)?.index() as u32,
            demand: plan.demands[&key],
        });
    }
    let global_symbols = plan.globals.iter().map(|(&symbol, &key)| (key, symbol))
        .collect::<std::collections::BTreeMap<_, _>>();
    for key in plan.demands.keys() {
        let mir = executable.sealed_mir().mir();
        let symbol = global_symbols.get(key).copied();
        manifest.initialization_roots.push(crate::artifact::InitializationRoot {
            node: key.node.index() as u32,
            module: mir.modules[mir.hir[key.node.index()].module.index()].name.clone(),
            symbol: symbol.map(|id| id.index() as u32),
            name: symbol.map(|id| mir.symbols[id.index()].name.clone()),
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
    module.section(&types);
    let mut imports = ImportSection::new();
    for (name, ty) in [
        ("telora_alloc", 0),
        ("telora_invoke", CALL_TYPE),
        ("telora_table_push", 3),
        ("telora_table_get", CALL_TYPE),
        ("telora_freeze", 2),
        ("telora_string_compare", CALL_TYPE),
        ("telora_source_name", 0),
        ("telora_subject_label", 0),
        ("telora_sort_pairs", CALL_TYPE),
        ("telora_duplicate_key_message", 0),
        ("telora_text_query", 3),
        ("telora_text_build", 4),
        ("telora_text_split", 3),
        ("telora_path", CALL_TYPE),
        ("telora_format_render", 0),
        ("telora_format_message", CALL_TYPE),
        ("telora_format_join", CALL_TYPE),
        ("telora_template_prepare", 0),
        ("telora_member_message", 3),
        ("telora_regex", 3),
        ("telora_hash", 3),
        ("telora_json_write", 3),
        ("telora_json_parse", 0),
        ("telora_toml_parse", 0),
        ("telora_yaml_parse", 0),
        ("telora_float_remainder", 5),
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
    let entry = initialize + 1;
    functions.function(2).function(2).function(CALL_TYPE);
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
        Elements::Functions(Cow::Owned((FIRST_FUNCTION..initialize).collect())),
    );
    module.section(&elements);
    let count = entry + 2;
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
            Instruction::Call(plan.functions[key]),
            Instruction::I32Const(0),
            Instruction::GlobalSet(INITIALIZATION_ROOT_GLOBAL),
        ] {
            init.instruction(&instruction);
        }
        if check {
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
    ] { init.instruction(&instruction); }
    init.instruction(&Instruction::Call(FREEZE))
        .instruction(&Instruction::Drop)
        .instruction(&Instruction::I32Const(2))
        .instruction(&Instruction::GlobalSet(PHASE_GLOBAL))
        .instruction(&Instruction::I32Const(1))
        .instruction(&Instruction::End);
    code.function(ObjectFunction::relocate(&init, count)?);
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
        .instruction(&Instruction::Call(plan.functions[&plan.root]))
        .instruction(&Instruction::End);
    code.function(ObjectFunction::relocate(&root, count)?);
    code.function(ObjectFunction::relocate(
        &crate::data_input::injector(&plan, &manifest),
        count,
    )?);
    let (code, relocations) = code.finish(5);
    module.section(&code);
    if !plan.reflection.is_empty() {
        let mut data = DataSection::new();
        data.active(0, &ConstExpr::i32_const(0), plan.reflection.iter().copied());
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
        let name = match index {
            n if n == initialize => "telora_initialize".to_owned(),
            n if n == entry => "telora_entry".to_owned(),
            n if n == entry + 1 => "telora_inject_data".to_owned(),
            n => names.name(function_keys[(n - FIRST_FUNCTION) as usize], n),
        };
        symbols.function(0, index, Some(&name));
        function_names.append(index, &name);
    }
    for (index, name) in ["telora_error", "telora_phase", "telora_call_source", "telora_call_start", "telora_call_end"].iter().enumerate() {
        symbols.global(0, index as u32, Some(name));
    }
    symbols.table(SymbolTable::WASM_SYM_UNDEFINED, 0, None);
    if !plan.reflection.is_empty() {
        symbols.data(
            0,
            "telora_type_image",
            Some(DataSymbolDefinition {
                index: 0,
                offset: 0,
                size: plan.reflection.len() as u32,
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
        name: Cow::Borrowed("telora.manifest"),
        data: Cow::Owned(serde_json::to_vec(&manifest).map_err(|e| e.to_string())?),
    });
    crate::compose::link(&module.finish(), heap_start)
}
