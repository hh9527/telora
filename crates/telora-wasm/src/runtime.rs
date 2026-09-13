//! Runtime helpers emitted into the artifact, with no Rust/JavaScript imports.
use crate::abi::*;
use wasm_encoder::{BlockType, Function, Instruction as I, ValType};

pub(crate) fn allocator() -> Function {
    let mut function = Function::new([(3, ValType::I32), (1, ValType::I64)]);
    let instructions = [
        I::GlobalGet(BUMP_GLOBAL),
        I::LocalSet(1),
        I::LocalGet(1),
        I::I64ExtendI32U,
        I::LocalGet(0),
        I::I64ExtendI32U,
        I::I64Add,
        I::I64Const(7),
        I::I64Add,
        I::I64Const(-8),
        I::I64And,
        I::LocalTee(4),
        I::I64Const(0xffff_fff8),
        I::I64GtU,
        I::If(BlockType::Empty),
        I::Unreachable,
        I::End,
        I::LocalGet(4),
        I::I32WrapI64,
        I::LocalSet(2),
        I::LocalGet(4),
        I::I64Const(65535),
        I::I64Add,
        I::I64Const(16),
        I::I64ShrU,
        I::I32WrapI64,
        I::LocalTee(3),
        I::MemorySize(0),
        I::I32GtU,
        I::If(BlockType::Empty),
        I::LocalGet(3),
        I::MemorySize(0),
        I::I32Sub,
        I::MemoryGrow(0),
        I::I32Const(-1),
        I::I32Eq,
        I::If(BlockType::Empty),
        I::Unreachable,
        I::End,
        I::End,
        I::LocalGet(2),
        I::GlobalSet(BUMP_GLOBAL),
        I::LocalGet(1),
        I::End,
    ];
    for instruction in instructions {
        function.instruction(&instruction);
    }
    function
}

pub(crate) fn invoke() -> Function {
    let mut function = Function::new([(1, ValType::I32)]);
    for instruction in [
        I::LocalGet(0),
        I::I32Load(memory(ENVIRONMENT, 2)),
        I::LocalTee(2),
        I::I32Eqz,
        I::If(BlockType::Result(ValType::I32)),
        I::I32Const(0),
        I::Else,
        I::I32Const(table_address(ENVIRONMENTS) as i32),
        I::LocalGet(2),
        I::I32Const(1),
        I::I32Sub,
        I::Call(TABLE_GET),
        I::I32Load(memory(0, 2)),
        I::End,
        I::LocalGet(1),
        I::LocalGet(0),
        I::I32Load(memory(DATA, 2)),
        I::CallIndirect {
            type_index: CALL_TYPE,
            table_index: 0,
        },
        I::End,
    ] {
        function.instruction(&instruction);
    }
    function
}
