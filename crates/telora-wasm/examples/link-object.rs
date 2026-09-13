//! Emit a relocatable caller for the Rust RT linking probe.
//! This is an object-format probe, not the SealedExecutable backend yet.
use wasm_encoder::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("expected output .o")?;
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I64], [ValType::I64]);
    types.ty().function([], [ValType::I64]);
    module.section(&types);
    let mut imports = ImportSection::new();
    imports.import("env", "rt_apply", EntityType::Function(0));
    module.section(&imports);
    let mut functions = FunctionSection::new();
    functions.function(1).function(0);
    module.section(&functions);
    let mut code = CodeSection::new();
    let mut answer = Function::new([]);
    answer.instruction(&Instruction::I64Const(21));
    // Object relocations require a five-byte index so the linker can patch it.
    answer.raw([0x10, 0x80, 0x80, 0x80, 0x80, 0x00]);
    answer.instruction(&Instruction::End);
    code.function(&answer);
    let mut callback = Function::new([]);
    callback.instruction(&Instruction::LocalGet(0));
    callback.instruction(&Instruction::I64Const(2));
    callback.instruction(&Instruction::I64Mul);
    callback.instruction(&Instruction::End);
    code.function(&callback);
    module.section(&code);
    let mut symbols = SymbolTable::new();
    symbols.function(SymbolTable::WASM_SYM_UNDEFINED, 0, None);
    symbols.function(0, 1, Some("answer"));
    symbols.function(0, 2, Some("telora_callback"));
    module.section(LinkingSection::new().symbol_table(&symbols));
    // Code section index 3, one R_WASM_FUNCTION_INDEX_LEB relocation,
    // code payload offset 6, symbol index 0 (rt_apply).
    module.section(&CustomSection {
        name: "reloc.CODE".into(),
        data: vec![3, 1, 0, 6, 0].into(),
    });
    std::fs::write(path, module.finish())?;
    Ok(())
}
