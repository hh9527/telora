//! Emit a relocatable caller for the Rust RT linking probe.
//! This is an object-format probe, not the SealedExecutable backend yet.
use telora_wasm::object::{ObjectCode, ObjectFunction};
use wasm_encoder::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("expected output .o")?;
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I64], [ValType::I64]);
    types.ty().function([], [ValType::I64]);
    types.ty().function([ValType::I32], [ValType::I64]);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("env", "rt_apply", EntityType::Function(0));
    imports.import("env", "rt_map_sum", EntityType::Function(2));
    imports.import(
        "env",
        "__indirect_function_table",
        EntityType::Table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: 1,
            maximum: None,
            shared: false,
        }),
    );
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(1).function(0).function(1);
    module.section(&functions);
    let mut elements = ElementSection::new();
    elements.active(
        Some(0),
        &ConstExpr::i32_const(1),
        Elements::Functions(vec![3].into()),
    );
    module.section(&elements);
    let mut code = ObjectCode::default();
    let mut answer = ObjectFunction::new(Function::new([]));
    answer.instruction(&Instruction::I64Const(21));
    answer.call(0);
    answer.instruction(&Instruction::End);
    code.function(answer);
    let mut callback = ObjectFunction::new(Function::new([]));
    callback.instruction(&Instruction::LocalGet(0));
    callback.instruction(&Instruction::I64Const(2));
    callback.instruction(&Instruction::I64Mul);
    callback.instruction(&Instruction::End);
    code.function(callback);
    let mut array = ObjectFunction::new(Function::new([]));
    array
        .function_pointer(2)
        .call(3)
        .instruction(&Instruction::End);
    code.function(array);
    let (code, relocations) = code.finish(4);
    module.section(&code);
    let mut symbols = SymbolTable::new();
    symbols.function(SymbolTable::WASM_SYM_UNDEFINED, 0, None);
    symbols.function(0, 2, Some("answer"));
    symbols.function(0, 3, Some("telora_callback"));
    symbols.function(SymbolTable::WASM_SYM_UNDEFINED, 1, None);
    symbols.function(0, 4, Some("array_answer"));
    symbols.table(SymbolTable::WASM_SYM_UNDEFINED, 0, None);
    module.section(LinkingSection::new().symbol_table(&symbols));
    module.section(&relocations);
    std::fs::write(path, module.finish())?;
    Ok(())
}
