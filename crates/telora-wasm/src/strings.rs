use crate::abi::*;
use wasm_encoder::{BlockType, Function, Instruction as I, ValType};

fn span(code: &mut Vec<I<'static>>, input: u32, pointer: u32, length: u32) {
    code.extend([
        I::LocalGet(input),
        I::I32Load8U(memory(DATA, 0)),
        I::I32Eqz,
        I::If(BlockType::Empty),
        I::LocalGet(input),
        I::I32Const(18),
        I::I32Add,
        I::LocalSet(pointer),
        I::LocalGet(input),
        I::I32Load8U(memory(17, 0)),
        I::LocalSet(length),
        I::Else,
        I::I32Const(table_address(STRINGS) as i32),
        I::LocalGet(input),
        I::I32Load(memory(20, 2)),
        I::Call(TABLE_GET),
        I::I32Load(memory(0, 2)),
        I::LocalGet(input),
        I::I32Load(memory(24, 2)),
        I::I32Add,
        I::LocalSet(pointer),
        I::LocalGet(input),
        I::I32Load(memory(28, 2)),
        I::LocalGet(input),
        I::I32Load(memory(24, 2)),
        I::I32Sub,
        I::LocalSet(length),
        I::End,
    ]);
}

pub(crate) fn compare() -> Function {
    // Parameters a,b; locals a_ptr,a_len,b_ptr,b_len,index,a_byte,b_byte.
    let mut function = Function::new([(7, ValType::I32)]);
    let mut code = vec![];
    span(&mut code, 0, 2, 3);
    span(&mut code, 1, 4, 5);
    code.extend([
        I::Block(BlockType::Empty),
        I::Loop(BlockType::Empty),
        I::LocalGet(6),
        I::LocalGet(3),
        I::I32GeU,
        I::LocalGet(6),
        I::LocalGet(5),
        I::I32GeU,
        I::I32Or,
        I::BrIf(1),
        I::LocalGet(2),
        I::LocalGet(6),
        I::I32Add,
        I::I32Load8U(memory(0, 0)),
        I::LocalSet(7),
        I::LocalGet(4),
        I::LocalGet(6),
        I::I32Add,
        I::I32Load8U(memory(0, 0)),
        I::LocalSet(8),
        I::LocalGet(7),
        I::LocalGet(8),
        I::I32Ne,
        I::If(BlockType::Empty),
        I::LocalGet(7),
        I::LocalGet(8),
        I::I32Sub,
        I::Return,
        I::End,
        I::LocalGet(6),
        I::I32Const(1),
        I::I32Add,
        I::LocalSet(6),
        I::Br(0),
        I::End,
        I::End,
        I::LocalGet(3),
        I::LocalGet(5),
        I::I32GtU,
        I::LocalGet(3),
        I::LocalGet(5),
        I::I32LtU,
        I::I32Sub,
        I::End,
    ]);
    for instruction in code {
        function.instruction(&instruction);
    }
    function
}
