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
    let plan = Plan::new(executable)?;
    let manifest = crate::artifact::Manifest::build(executable, &plan.layouts)?;
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
    let heap_start = STATIC_BASE
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
    globals.global(mutable, &ConstExpr::i32_const(0));
    globals.global(mutable, &ConstExpr::i32_const(0));
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
    for key in plan.demands.keys() {
        for instruction in [
            Instruction::I32Const(0),
            Instruction::I32Const(0),
            Instruction::Call(plan.functions[key]),
            Instruction::I32Eqz,
            Instruction::If(BlockType::Empty),
            Instruction::I32Const(0),
            Instruction::Return,
            Instruction::End,
        ] {
            init.instruction(&instruction);
        }
    }
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
    let mut symbols = SymbolTable::new();
    for index in 0..FIRST_FUNCTION {
        symbols.function(SymbolTable::WASM_SYM_UNDEFINED, index, None);
    }
    for index in FIRST_FUNCTION..count {
        let name = match index {
            n if n == initialize => "telora_initialize".to_owned(),
            n if n == entry => "telora_entry".to_owned(),
            n if n == entry + 1 => "telora_inject_data".to_owned(),
            n => format!("telora_fn_{n}"),
        };
        symbols.function(0, index, Some(&name));
    }
    for (index, name) in ["telora_error", "telora_phase"].iter().enumerate() {
        symbols.global(0, index as u32, Some(name));
    }
    symbols.table(SymbolTable::WASM_SYM_UNDEFINED, 0, None);
    module.section(LinkingSection::new().symbol_table(&symbols));
    module.section(&relocations);
    module.section(&CustomSection {
        name: Cow::Borrowed("telora.abi"),
        data: Cow::Owned(VERSION.to_le_bytes().to_vec()),
    });
    module.section(&CustomSection {
        name: Cow::Borrowed("telora.manifest"),
        data: Cow::Owned(serde_json::to_vec(&manifest).map_err(|e| e.to_string())?),
    });
    crate::link::link(&module.finish(), heap_start)
}
