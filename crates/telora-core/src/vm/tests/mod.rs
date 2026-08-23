use super::*;
use crate::bytecode::Constant;
use crate::{Atom, BytecodeFunction, Instruction, NativeFunction, Register};

fn run(
    vm: &mut Vm,
    registers: usize,
    constants: Vec<Constant>,
    instructions: Vec<Instruction>,
) -> Result<ExecutionWorld, RuntimeError> {
    vm.execute(
        &BytecodeFunction::new("test", registers, constants, instructions),
        1_000,
    )
}

include!("part-01.rs");
