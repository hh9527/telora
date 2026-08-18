use crate::bytecode::{BytecodeFunction, Constant, DebugOriginRange, Instruction, Register};
use crate::{Origin, WithOrigin};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RegisterId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConstantId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LabelId(pub u32);

#[derive(Clone, Debug)]
pub enum Operation {
    LoadConst {
        dst: RegisterId,
        constant: ConstantId,
    },
    Move {
        dst: RegisterId,
        src: RegisterId,
    },
    OwnDeclared {
        dst: RegisterId,
        owner: RegisterId,
        value: RegisterId,
    },
    AllocFunc {
        dst: RegisterId,
        static_id: Option<crate::FuncId>,
    },
    SealFunc {
        target: RegisterId,
        source: RegisterId,
    },
    AllocTypeSlot {
        dst: RegisterId,
    },
    ReadTypeSlot {
        dst: RegisterId,
        link: RegisterId,
    },
    SealTypeSlot {
        link: RegisterId,
        src: RegisterId,
    },
    AssertTypeSlotReady {
        link: RegisterId,
    },
    Add {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    Subtract {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    Multiply {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    Divide {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    Remainder {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    Negate {
        dst: RegisterId,
        src: RegisterId,
    },
    Not {
        dst: RegisterId,
        src: RegisterId,
    },
    LogicalNot {
        dst: RegisterId,
        src: RegisterId,
    },
    BitNot {
        dst: RegisterId,
        src: RegisterId,
    },
    BitAnd {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    BitOr {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    BitXor {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    Equal {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    NotEqual {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    LessThan {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    LessThanOrEqual {
        dst: RegisterId,
        left: RegisterId,
        right: RegisterId,
    },
    MakeArray {
        dst: RegisterId,
        items: Vec<RegisterId>,
    },
    ConcatArrays {
        dst: RegisterId,
        arrays: Vec<RegisterId>,
    },
    MakeTuple {
        dst: RegisterId,
        items: Vec<RegisterId>,
    },
    InterpolateString {
        dst: RegisterId,
        parts: Vec<RegisterId>,
    },
    MakeDict {
        dst: RegisterId,
        fields: Vec<(String, RegisterId)>,
    },
    MergeDicts {
        dst: RegisterId,
        dicts: Vec<RegisterId>,
    },
    GetField {
        dst: RegisterId,
        dict: RegisterId,
        field: String,
    },
    GetArray {
        dst: RegisterId,
        array: RegisterId,
        index: RegisterId,
    },
    ProjectTuple {
        dst: RegisterId,
        tuple: RegisterId,
        index: usize,
    },
    FieldExists {
        dst: RegisterId,
        value: RegisterId,
        field: String,
    },
    IsDict {
        dst: RegisterId,
        value: RegisterId,
    },
    TupleLengthEquals {
        dst: RegisterId,
        value: RegisterId,
        length: usize,
    },
    GetTuple {
        dst: RegisterId,
        tuple: RegisterId,
        index: usize,
    },
    TaggedTagEquals {
        dst: RegisterId,
        value: RegisterId,
        tag: RegisterId,
    },
    GetTaggedPayload {
        dst: RegisterId,
        value: RegisterId,
    },
    MakeClosure {
        dst: RegisterId,
        function: Box<Function>,
        captures: Vec<RegisterId>,
    },
    Call {
        base: RegisterId,
        argument_count: u32,
    },
    TailCall {
        base: RegisterId,
        argument_count: u32,
    },
    Jump {
        target: LabelId,
    },
    JumpIfFalse {
        condition: RegisterId,
        target: LabelId,
    },
    Return {
        src: RegisterId,
    },
    Fail {
        message: String,
    },
    Panic {
        message: RegisterId,
    },
    Raise {
        error: RegisterId,
    },
    Debug {
        value: RegisterId,
        module: String,
        line: u32,
        name: String,
        message: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub enum Item {
    Label(LabelId),
    Operation(WithOrigin<Operation>),
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub parameter_count: u32,
    pub capture_count: u32,
    pub register_count: u32,
    pub constants: Vec<Constant>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssembleError {
    pub message: String,
}

impl fmt::Display for AssembleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AssembleError {}

pub fn assemble(function: Function) -> Result<BytecodeFunction, AssembleError> {
    let register_count = usize::try_from(function.register_count)
        .map_err(|_| assembly_error("register count does not fit this platform"))?;
    let parameter_count = usize::try_from(function.parameter_count)
        .map_err(|_| assembly_error("parameter count does not fit this platform"))?;
    let capture_count = usize::try_from(function.capture_count)
        .map_err(|_| assembly_error("capture count does not fit this platform"))?;
    if parameter_count + capture_count > register_count {
        return Err(assembly_error(
            "parameters and captures exceed the declared register count",
        ));
    }

    let mut labels = HashMap::new();
    let mut pc = 0usize;
    for item in &function.items {
        match item {
            Item::Label(label) => {
                if labels.insert(*label, pc).is_some() {
                    return Err(assembly_error(format!("duplicate label {}", label.0)));
                }
            }
            Item::Operation(_) => pc += 1,
        }
    }

    let mut instructions = Vec::with_capacity(pc);
    let mut origins = Vec::with_capacity(pc);
    for item in function.items {
        let Item::Operation(operation) = item else {
            continue;
        };
        let instruction = lower_operation(
            operation.value,
            register_count,
            function.constants.len(),
            &labels,
        )?;
        instructions.push(instruction);
        origins.push(operation.origin);
    }
    if instructions.is_empty() {
        return Err(assembly_error(
            "function contains no executable instructions",
        ));
    }
    let debug_origins = compress_origins(&origins);
    Ok(BytecodeFunction::assembled_constants(
        function.name,
        parameter_count,
        capture_count,
        register_count,
        function.constants,
        instructions,
        debug_origins,
    ))
}

fn lower_operation(
    operation: Operation,
    register_count: usize,
    constant_count: usize,
    labels: &HashMap<LabelId, usize>,
) -> Result<Instruction, AssembleError> {
    let register = |id: RegisterId| -> Result<Register, AssembleError> {
        let index = usize::try_from(id.0).map_err(|_| assembly_error("register is too large"))?;
        if index >= register_count {
            return Err(assembly_error(format!(
                "register {} is out of bounds",
                id.0
            )));
        }
        Ok(Register(index))
    };
    let registers = |ids: Vec<RegisterId>| -> Result<Vec<Register>, AssembleError> {
        ids.into_iter().map(register).collect()
    };
    let label = |id: LabelId| -> Result<usize, AssembleError> {
        labels
            .get(&id)
            .copied()
            .ok_or_else(|| assembly_error(format!("undefined label {}", id.0)))
    };
    Ok(match operation {
        Operation::LoadConst { dst, constant } => {
            let constant =
                usize::try_from(constant.0).map_err(|_| assembly_error("constant is too large"))?;
            if constant >= constant_count {
                return Err(assembly_error(format!(
                    "constant {constant} is out of bounds"
                )));
            }
            Instruction::LoadConst {
                dst: register(dst)?,
                constant,
            }
        }
        Operation::Move { dst, src } => Instruction::Move {
            dst: register(dst)?,
            src: register(src)?,
        },
        Operation::OwnDeclared { dst, owner, value } => Instruction::OwnDeclared {
            dst: register(dst)?,
            owner: register(owner)?,
            value: register(value)?,
        },
        Operation::AllocFunc { dst, static_id } => Instruction::AllocFunc {
            dst: register(dst)?,
            static_id,
        },
        Operation::SealFunc { target, source } => Instruction::SealFunc {
            target: register(target)?,
            source: register(source)?,
        },
        Operation::AllocTypeSlot { dst } => Instruction::AllocTypeSlot {
            dst: register(dst)?,
        },
        Operation::ReadTypeSlot { dst, link } => Instruction::ReadTypeSlot {
            dst: register(dst)?,
            link: register(link)?,
        },
        Operation::SealTypeSlot { link, src } => Instruction::SealTypeSlot {
            link: register(link)?,
            src: register(src)?,
        },
        Operation::AssertTypeSlotReady { link } => Instruction::AssertTypeSlotReady {
            link: register(link)?,
        },
        Operation::Add { dst, left, right } => Instruction::Add {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::Subtract { dst, left, right } => Instruction::Subtract {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::Multiply { dst, left, right } => Instruction::Multiply {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::Divide { dst, left, right } => Instruction::Divide {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::Remainder { dst, left, right } => Instruction::Remainder {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::Negate { dst, src } => Instruction::Negate {
            dst: register(dst)?,
            src: register(src)?,
        },
        Operation::Not { dst, src } => Instruction::Not {
            dst: register(dst)?,
            src: register(src)?,
        },
        Operation::LogicalNot { dst, src } => Instruction::LogicalNot {
            dst: register(dst)?,
            src: register(src)?,
        },
        Operation::BitNot { dst, src } => Instruction::BitNot {
            dst: register(dst)?,
            src: register(src)?,
        },
        Operation::BitAnd { dst, left, right } => Instruction::BitAnd {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::BitOr { dst, left, right } => Instruction::BitOr {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::BitXor { dst, left, right } => Instruction::BitXor {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::Equal { dst, left, right } => Instruction::Equal {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::NotEqual { dst, left, right } => Instruction::NotEqual {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::LessThan { dst, left, right } => Instruction::LessThan {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::LessThanOrEqual { dst, left, right } => Instruction::LessThanOrEqual {
            dst: register(dst)?,
            left: register(left)?,
            right: register(right)?,
        },
        Operation::MakeArray { dst, items } => Instruction::MakeArray {
            dst: register(dst)?,
            items: registers(items)?,
        },
        Operation::ConcatArrays { dst, arrays } => Instruction::ConcatArrays {
            dst: register(dst)?,
            arrays: registers(arrays)?,
        },
        Operation::MakeTuple { dst, items } => Instruction::MakeTuple {
            dst: register(dst)?,
            items: registers(items)?,
        },
        Operation::InterpolateString { dst, parts } => Instruction::InterpolateString {
            dst: register(dst)?,
            parts: registers(parts)?,
        },
        Operation::MakeDict { dst, fields } => Instruction::MakeDict {
            dst: register(dst)?,
            fields: fields
                .into_iter()
                .map(|(name, value)| Ok((name, register(value)?)))
                .collect::<Result<_, AssembleError>>()?,
        },
        Operation::MergeDicts { dst, dicts } => Instruction::MergeDicts {
            dst: register(dst)?,
            dicts: registers(dicts)?,
        },
        Operation::GetField { dst, dict, field } => Instruction::GetField {
            dst: register(dst)?,
            dict: register(dict)?,
            field,
        },
        Operation::GetArray { dst, array, index } => Instruction::GetArray {
            dst: register(dst)?,
            array: register(array)?,
            index: register(index)?,
        },
        Operation::ProjectTuple { dst, tuple, index } => Instruction::ProjectTuple {
            dst: register(dst)?,
            tuple: register(tuple)?,
            index,
        },
        Operation::FieldExists { dst, value, field } => Instruction::FieldExists {
            dst: register(dst)?,
            value: register(value)?,
            field,
        },
        Operation::IsDict { dst, value } => Instruction::IsDict {
            dst: register(dst)?,
            value: register(value)?,
        },
        Operation::TupleLengthEquals { dst, value, length } => Instruction::TupleLengthEquals {
            dst: register(dst)?,
            value: register(value)?,
            length,
        },
        Operation::GetTuple { dst, tuple, index } => Instruction::GetTuple {
            dst: register(dst)?,
            tuple: register(tuple)?,
            index,
        },
        Operation::TaggedTagEquals { dst, value, tag } => Instruction::TaggedTagEquals {
            dst: register(dst)?,
            value: register(value)?,
            tag: register(tag)?,
        },
        Operation::GetTaggedPayload { dst, value } => Instruction::GetTaggedPayload {
            dst: register(dst)?,
            value: register(value)?,
        },
        Operation::MakeClosure {
            dst,
            function,
            captures,
        } => Instruction::MakeClosure {
            dst: register(dst)?,
            function: Arc::new(assemble(*function)?),
            captures: registers(captures)?,
        },
        Operation::Call {
            base,
            argument_count,
        } => {
            let base = register(base)?;
            let count = usize::try_from(argument_count)
                .map_err(|_| assembly_error("argument count is too large"))?;
            let end = base
                .0
                .checked_add(count)
                .ok_or_else(|| assembly_error("call window overflows"))?;
            if end >= register_count {
                return Err(assembly_error("call window is out of bounds"));
            }
            Instruction::Call {
                base,
                argument_count: count,
            }
        }
        Operation::TailCall {
            base,
            argument_count,
        } => {
            let base = register(base)?;
            let count = usize::try_from(argument_count)
                .map_err(|_| assembly_error("argument count is too large"))?;
            let end = base
                .0
                .checked_add(count)
                .ok_or_else(|| assembly_error("call window overflows"))?;
            if end >= register_count {
                return Err(assembly_error("call window is out of bounds"));
            }
            Instruction::TailCall {
                base,
                argument_count: count,
            }
        }
        Operation::Jump { target } => Instruction::Jump {
            target: label(target)?,
        },
        Operation::JumpIfFalse { condition, target } => Instruction::JumpIfFalse {
            condition: register(condition)?,
            target: label(target)?,
        },
        Operation::Return { src } => Instruction::Return {
            src: register(src)?,
        },
        Operation::Fail { message } => Instruction::Fail { message },
        Operation::Panic { message } => Instruction::Panic {
            message: register(message)?,
        },
        Operation::Raise { error } => Instruction::Raise {
            error: register(error)?,
        },
        Operation::Debug {
            value,
            module,
            line,
            name,
            message,
        } => Instruction::Debug {
            value: register(value)?,
            module,
            line,
            name,
            message,
        },
    })
}

fn compress_origins(origins: &[Origin]) -> Vec<DebugOriginRange> {
    let mut ranges: Vec<DebugOriginRange> = Vec::new();
    for (pc, origin) in origins.iter().copied().enumerate() {
        if let Some(last) = ranges.last_mut()
            && last.origin == origin
            && last.end == pc
        {
            last.end += 1;
            continue;
        }
        ranges.push(DebugOriginRange {
            start: pc,
            end: pc + 1,
            origin,
        });
    }
    ranges
}

fn assembly_error(message: impl Into<String>) -> AssembleError {
    AssembleError {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> Origin {
        Origin::Synthetic { derived_from: None }
    }

    #[test]
    fn resolves_labels_and_compresses_origins() {
        let function = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![Constant::Int(1)],
            items: vec![
                Item::Operation(WithOrigin {
                    value: Operation::LoadConst {
                        dst: RegisterId(0),
                        constant: ConstantId(0),
                    },
                    origin: origin(),
                }),
                Item::Operation(WithOrigin {
                    value: Operation::Jump { target: LabelId(0) },
                    origin: origin(),
                }),
                Item::Label(LabelId(0)),
                Item::Operation(WithOrigin {
                    value: Operation::Return { src: RegisterId(0) },
                    origin: origin(),
                }),
            ],
        };
        let bytecode = assemble(function).unwrap();
        assert!(matches!(
            bytecode.instructions()[1],
            crate::bytecode::Opcode::Jump { target: 2 }
        ));
        assert_eq!(bytecode.debug_origins().len(), 1);
        assert_eq!(bytecode.debug_origins()[0].end, 3);
    }

    #[test]
    fn rejects_undefined_labels_and_bad_registers() {
        let bad_label = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![],
            items: vec![Item::Operation(WithOrigin {
                value: Operation::Jump { target: LabelId(4) },
                origin: origin(),
            })],
        };
        assert!(
            assemble(bad_label)
                .unwrap_err()
                .message
                .contains("undefined label")
        );
        let bad_register = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![],
            items: vec![Item::Operation(WithOrigin {
                value: Operation::Return { src: RegisterId(1) },
                origin: origin(),
            })],
        };
        assert!(
            assemble(bad_register)
                .unwrap_err()
                .message
                .contains("out of bounds")
        );

        let bad_arguments = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![],
            items: vec![Item::Operation(WithOrigin {
                value: Operation::Call {
                    base: RegisterId(0),
                    argument_count: 2,
                },
                origin: origin(),
            })],
        };
        assert!(
            assemble(bad_arguments)
                .unwrap_err()
                .message
                .contains("call window")
        );

        let duplicate_label = Function {
            name: "test".into(),
            parameter_count: 0,
            capture_count: 0,
            register_count: 1,
            constants: vec![],
            items: vec![Item::Label(LabelId(0)), Item::Label(LabelId(0))],
        };
        assert!(
            assemble(duplicate_label)
                .unwrap_err()
                .message
                .contains("duplicate label")
        );
    }
}
