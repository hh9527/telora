use telora_core::mir::{HirId, HirKind, Mir, Role, SealedExecutable};
use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction, Module,
    TypeSection, ValType,
};

/// First portable vertical slice: an integer literal export, with no host imports.
/// This deliberately rejects everything outside this admission boundary.
/// The scalar export ABI is experimental, not the final value/runtime ABI.
pub fn compile_scalar(executable: &SealedExecutable<'_>) -> Result<Vec<u8>, String> {
    if !executable.properties().is_empty() || !executable.checks().is_empty() {
        return Err("Wasm scalar slice does not support metadata initialization".into());
    }
    let mir = executable.sealed_mir().mir();
    let value = integer(mir, executable.root())?;
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I64]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut exports = ExportSection::new();
    exports.export("telora_entry", ExportKind::Func, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    let mut function = Function::new([]);
    function.instruction(&Instruction::I64Const(value));
    function.instruction(&Instruction::End);
    code.function(&function);
    module.section(&code);
    Ok(module.finish())
}

fn integer(mir: &Mir, mut id: HirId) -> Result<i64, String> {
    loop {
        let node = &mir.hir[id.index()];
        match &node.kind {
            HirKind::Int(value) => return Ok(*value),
            HirKind::Binding { .. } | HirKind::TypeAscription => {
                id = node
                    .children
                    .iter()
                    .find(|edge| edge.role == Role::Value)
                    .ok_or("Wasm scalar declaration has no value")?
                    .node;
            }
            other => return Err(format!("Wasm scalar slice does not support {other:?}")),
        }
    }
}
