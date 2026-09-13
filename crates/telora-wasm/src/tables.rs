//! Fixed-width table slots own independent payload allocations. HeapIds survive growth.
use crate::abi::*;
use wasm_encoder::{BlockType, Function, Instruction as I, ValType};

pub(crate) fn push() -> Function {
    // table address, payload pointer, byte length -> HeapId.
    // locals: count, capacity, old buffer, new capacity, new buffer, slot.
    let mut f = Function::new([(6, ValType::I32)]);
    for instruction in [
        I::LocalGet(0),
        I::I32Load(memory(4, 2)),
        I::LocalSet(3),
        I::LocalGet(0),
        I::I32Load(memory(8, 2)),
        I::LocalSet(4),
        I::LocalGet(0),
        I::I32Load(memory(0, 2)),
        I::LocalSet(5),
        I::LocalGet(3),
        I::LocalGet(4),
        I::I32Eq,
        I::If(BlockType::Empty),
        I::LocalGet(4),
        I::I32Const(0x0fff_ffff),
        I::I32GtU,
        I::If(BlockType::Empty),
        I::Unreachable,
        I::End,
        I::LocalGet(4),
        I::I32Eqz,
        I::If(BlockType::Result(ValType::I32)),
        I::I32Const(8),
        I::Else,
        I::LocalGet(4),
        I::I32Const(2),
        I::I32Mul,
        I::End,
        I::LocalTee(6),
        I::I32Const(8),
        I::I32Mul,
        I::Call(ALLOC),
        I::LocalSet(7),
        I::LocalGet(7),
        I::LocalGet(5),
        I::LocalGet(3),
        I::I32Const(8),
        I::I32Mul,
        I::MemoryCopy {
            src_mem: 0,
            dst_mem: 0,
        },
        I::LocalGet(0),
        I::LocalGet(7),
        I::I32Store(memory(0, 2)),
        I::LocalGet(0),
        I::LocalGet(6),
        I::I32Store(memory(8, 2)),
        I::LocalGet(7),
        I::LocalSet(5),
        I::End,
        I::LocalGet(5),
        I::LocalGet(3),
        I::I32Const(8),
        I::I32Mul,
        I::I32Add,
        I::LocalSet(8),
        I::LocalGet(8),
        I::LocalGet(1),
        I::I32Store(memory(0, 2)),
        I::LocalGet(8),
        I::LocalGet(2),
        I::I32Store(memory(4, 2)),
        I::LocalGet(0),
        I::LocalGet(3),
        I::I32Const(1),
        I::I32Add,
        I::I32Store(memory(4, 2)),
        I::LocalGet(3),
        I::End,
    ] {
        f.instruction(&instruction);
    }
    f
}

pub(crate) fn get() -> Function {
    let mut f = Function::new([]);
    for instruction in [
        I::LocalGet(1),
        I::LocalGet(0),
        I::I32Load(memory(4, 2)),
        I::I32GeU,
        I::If(BlockType::Empty),
        I::Unreachable,
        I::End,
        I::LocalGet(0),
        I::I32Load(memory(0, 2)),
        I::LocalGet(1),
        I::I32Const(8),
        I::I32Mul,
        I::I32Add,
        I::End,
    ] {
        f.instruction(&instruction);
    }
    f
}

pub(crate) fn freeze() -> Function {
    let mut f = Function::new([]);
    for table in 0..TABLE_COUNT {
        let address = table_address(table) as i32;
        for instruction in [
            I::I32Const(address),
            I::I32Const(address),
            I::I32Load(memory(4, 2)),
            I::I32Store(memory(12, 2)),
        ] {
            f.instruction(&instruction);
        }
    }
    f.instruction(&I::I32Const(1)).instruction(&I::End);
    f
}
