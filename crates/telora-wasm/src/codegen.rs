use crate::{abi::*, emit, plan::Plan, runtime};
use std::borrow::Cow;
use telora_core::mir::SealedExecutable;
use wasm_encoder::*;

/// Generate a self-contained Wasm module from already sealed execution evidence.
pub fn compile_executable(executable: &SealedExecutable<'_>) -> Result<Vec<u8>, String> {
    let plan = Plan::new(executable)?;
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
    let mut functions = FunctionSection::new();
    functions.function(0).function(CALL_TYPE);
    functions
        .function(3)
        .function(CALL_TYPE)
        .function(2)
        .function(CALL_TYPE);
    for _ in &plan.functions {
        functions.function(CALL_TYPE);
    }
    let initialize = FIRST_FUNCTION + plan.functions.len() as u32;
    let entry = initialize + 1;
    functions.function(2).function(2);
    module.section(&functions);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: initialize as u64,
        maximum: Some(initialize as u64),
        shared: false,
    });
    module.section(&tables);
    let heap_start = STATIC_BASE
        .checked_add(
            (plan.demands.len() as u32)
                .checked_mul(DEMAND_BYTES)
                .ok_or("Wasm: demand allocation overflow")?,
        )
        .ok_or("Wasm: static memory overflow")?;
    let pages = u64::from(heap_start).div_ceil(65536).max(1);
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: pages,
        maximum: Some(65536),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&memories);
    let mut globals = GlobalSection::new();
    let mutable = GlobalType {
        val_type: ValType::I32,
        mutable: true,
        shared: false,
    };
    globals.global(mutable, &ConstExpr::i32_const(heap_start as i32));
    globals.global(mutable, &ConstExpr::i32_const(0));
    globals.global(mutable, &ConstExpr::i32_const(0));
    module.section(&globals);
    let mut exports = ExportSection::new();
    exports
        .export("memory", ExportKind::Memory, 0)
        .export("telora_alloc", ExportKind::Func, ALLOC)
        .export("telora_invoke", ExportKind::Func, INVOKE)
        .export("telora_table_push", ExportKind::Func, TABLE_PUSH)
        .export("telora_initialize", ExportKind::Func, initialize)
        .export("telora_entry", ExportKind::Func, entry)
        .export("telora_error", ExportKind::Global, ERROR_GLOBAL);
    module.section(&exports);
    let mut elements = ElementSection::new();
    elements.active(
        Some(0),
        &ConstExpr::i32_const(0),
        Elements::Functions(Cow::Owned((0..initialize).collect())),
    );
    module.section(&elements);
    let mut code = CodeSection::new();
    code.function(&runtime::allocator())
        .function(&runtime::invoke());
    code.function(&crate::tables::push())
        .function(&crate::tables::get())
        .function(&crate::tables::freeze());
    code.function(&crate::strings::compare());
    for &key in plan.functions.keys() {
        code.function(&emit::compile(executable.sealed_mir().mir(), &plan, key)?);
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
    code.function(&init);
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
    code.function(&root);
    module.section(&code);
    module.section(&CustomSection {
        name: Cow::Borrowed("telora.abi"),
        data: Cow::Owned(VERSION.to_le_bytes().to_vec()),
    });
    let manifest = crate::artifact::Manifest::build(executable, &plan.layouts)?;
    module.section(&CustomSection {
        name: Cow::Borrowed("telora.manifest"),
        data: Cow::Owned(serde_json::to_vec(&manifest).map_err(|e| e.to_string())?),
    });
    Ok(module.finish())
}
