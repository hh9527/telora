use crate::{Atom, NativeFunction, Origin};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Register(pub usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ValueLinkId(pub usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextLinkId(pub usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProtoLinkId(pub usize);

#[derive(Clone, Debug)]
pub enum Constant {
    Placeholder,
    Int(i64),
    Float(f64),
    String(Arc<str>),
    Bytes(Arc<[u8]>),
    Atom(Atom),
    Native(NativeFunction),
}

#[derive(Clone, Debug)]
pub enum Instruction {
    LoadConst {
        dst: Register,
        constant: usize,
    },
    Move {
        dst: Register,
        src: Register,
    },
    OwnDeclared {
        dst: Register,
        owner: Register,
        value: Register,
    },
    AllocFunc {
        dst: Register,
        static_id: Option<crate::FuncId>,
    },
    SealFunc {
        target: Register,
        source: Register,
    },
    AllocTypeSlot {
        dst: Register,
    },
    ReadTypeSlot {
        dst: Register,
        link: Register,
    },
    SealTypeSlot {
        link: Register,
        src: Register,
    },
    AssertTypeSlotReady {
        link: Register,
    },
    Add {
        dst: Register,
        left: Register,
        right: Register,
    },
    Subtract {
        dst: Register,
        left: Register,
        right: Register,
    },
    Multiply {
        dst: Register,
        left: Register,
        right: Register,
    },
    Divide {
        dst: Register,
        left: Register,
        right: Register,
    },
    Remainder {
        dst: Register,
        left: Register,
        right: Register,
    },
    Negate {
        dst: Register,
        src: Register,
    },
    Not {
        dst: Register,
        src: Register,
    },
    LogicalNot {
        dst: Register,
        src: Register,
    },
    BitNot {
        dst: Register,
        src: Register,
    },
    BitAnd {
        dst: Register,
        left: Register,
        right: Register,
    },
    BitOr {
        dst: Register,
        left: Register,
        right: Register,
    },
    BitXor {
        dst: Register,
        left: Register,
        right: Register,
    },
    Equal {
        dst: Register,
        left: Register,
        right: Register,
    },
    NotEqual {
        dst: Register,
        left: Register,
        right: Register,
    },
    LessThan {
        dst: Register,
        left: Register,
        right: Register,
    },
    LessThanOrEqual {
        dst: Register,
        left: Register,
        right: Register,
    },
    MakeArray {
        dst: Register,
        items: Vec<Register>,
    },
    ConcatArrays {
        dst: Register,
        arrays: Vec<Register>,
    },
    MakeTuple {
        dst: Register,
        items: Vec<Register>,
    },
    InterpolateString {
        dst: Register,
        parts: Vec<Register>,
    },
    MakeDict {
        dst: Register,
        fields: Vec<(String, Register)>,
    },
    MergeDicts {
        dst: Register,
        dicts: Vec<Register>,
    },
    GetField {
        dst: Register,
        dict: Register,
        field: String,
    },
    GetArray {
        dst: Register,
        array: Register,
        index: Register,
    },
    ProjectTuple {
        dst: Register,
        tuple: Register,
        index: usize,
    },
    FieldExists {
        dst: Register,
        value: Register,
        field: String,
    },
    IsDict {
        dst: Register,
        value: Register,
    },
    TupleLengthEquals {
        dst: Register,
        value: Register,
        length: usize,
    },
    GetTuple {
        dst: Register,
        tuple: Register,
        index: usize,
    },
    TaggedTagEquals {
        dst: Register,
        value: Register,
        tag: Register,
    },
    GetTaggedPayload {
        dst: Register,
        value: Register,
    },
    MakeClosure {
        dst: Register,
        function: Arc<BytecodeFunction>,
        captures: Vec<Register>,
    },
    Call {
        base: Register,
        argument_count: usize,
    },
    TailCall {
        base: Register,
        argument_count: usize,
    },
    Jump {
        target: usize,
    },
    JumpIfFalse {
        condition: Register,
        target: usize,
    },
    Return {
        src: Register,
    },
    Fail {
        message: String,
    },
    Panic {
        message: Register,
    },
    Raise {
        error: Register,
    },
    Debug {
        value: Register,
        module: String,
        line: u32,
        name: String,
        message: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub enum Opcode {
    LoadConst {
        dst: Register,
        value: ValueLinkId,
    },
    Move {
        dst: Register,
        src: Register,
    },
    OwnDeclared {
        dst: Register,
        owner: Register,
        value: Register,
    },
    AllocFunc {
        dst: Register,
        static_id: Option<crate::FuncId>,
    },
    SealFunc {
        target: Register,
        source: Register,
    },
    AllocTypeSlot {
        dst: Register,
    },
    ReadTypeSlot {
        dst: Register,
        link: Register,
    },
    SealTypeSlot {
        link: Register,
        src: Register,
    },
    AssertTypeSlotReady {
        link: Register,
    },
    Add {
        dst: Register,
        left: Register,
        right: Register,
    },
    Subtract {
        dst: Register,
        left: Register,
        right: Register,
    },
    Multiply {
        dst: Register,
        left: Register,
        right: Register,
    },
    Divide {
        dst: Register,
        left: Register,
        right: Register,
    },
    Remainder {
        dst: Register,
        left: Register,
        right: Register,
    },
    Negate {
        dst: Register,
        src: Register,
    },
    Not {
        dst: Register,
        src: Register,
    },
    LogicalNot {
        dst: Register,
        src: Register,
    },
    BitNot {
        dst: Register,
        src: Register,
    },
    BitAnd {
        dst: Register,
        left: Register,
        right: Register,
    },
    BitOr {
        dst: Register,
        left: Register,
        right: Register,
    },
    BitXor {
        dst: Register,
        left: Register,
        right: Register,
    },
    Equal {
        dst: Register,
        left: Register,
        right: Register,
    },
    NotEqual {
        dst: Register,
        left: Register,
        right: Register,
    },
    LessThan {
        dst: Register,
        left: Register,
        right: Register,
    },
    LessThanOrEqual {
        dst: Register,
        left: Register,
        right: Register,
    },
    MakeArray {
        dst: Register,
        items: Vec<Register>,
    },
    ConcatArrays {
        dst: Register,
        arrays: Vec<Register>,
    },
    MakeTuple {
        dst: Register,
        items: Vec<Register>,
    },
    InterpolateString {
        dst: Register,
        parts: Vec<Register>,
    },
    MakeDict {
        dst: Register,
        fields: Vec<(TextLinkId, Register)>,
    },
    MergeDicts {
        dst: Register,
        dicts: Vec<Register>,
    },
    GetField {
        dst: Register,
        dict: Register,
        field: TextLinkId,
    },
    GetArray {
        dst: Register,
        array: Register,
        index: Register,
    },
    ProjectTuple {
        dst: Register,
        tuple: Register,
        index: usize,
    },
    FieldExists {
        dst: Register,
        value: Register,
        field: TextLinkId,
    },
    IsDict {
        dst: Register,
        value: Register,
    },
    TupleLengthEquals {
        dst: Register,
        value: Register,
        length: usize,
    },
    GetTuple {
        dst: Register,
        tuple: Register,
        index: usize,
    },
    TaggedTagEquals {
        dst: Register,
        value: Register,
        tag: Register,
    },
    GetTaggedPayload {
        dst: Register,
        value: Register,
    },
    MakeClosure {
        dst: Register,
        prototype: ProtoLinkId,
        captures: Vec<Register>,
    },
    Call {
        base: Register,
        argument_count: usize,
    },
    TailCall {
        base: Register,
        argument_count: usize,
    },
    Jump {
        target: usize,
    },
    JumpIfFalse {
        condition: Register,
        target: usize,
    },
    Return {
        src: Register,
    },
    Fail {
        message: String,
    },
    Panic {
        message: Register,
    },
    Raise {
        error: Register,
    },
    Debug {
        value: Register,
        module: String,
        line: u32,
        name: String,
        message: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct FuncByteCode {
    name: Arc<str>,
    parameter_count: usize,
    capture_count: usize,
    register_count: usize,
    instructions: Vec<Opcode>,
    debug_origins: Vec<DebugOriginRange>,
}

impl FuncByteCode {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) const fn parameter_count(&self) -> usize {
        self.parameter_count
    }
}

#[derive(Clone, Debug, Default)]
pub struct LinkingTable {
    values: Vec<Constant>,
    external_values: Vec<Option<Arc<str>>>,
    text: Vec<Arc<str>>,
    prototypes: Vec<Arc<BytecodeFunction>>,
}

impl LinkingTable {
    pub(crate) fn values(&self) -> &[Constant] {
        &self.values
    }

    pub(crate) fn external_value(&self, index: usize) -> Option<&str> {
        self.external_values.get(index)?.as_deref()
    }

    pub(crate) fn text(&self) -> &[Arc<str>] {
        &self.text
    }

    pub(crate) fn prototypes(&self) -> &[Arc<BytecodeFunction>] {
        &self.prototypes
    }
}

#[derive(Clone, Debug)]
pub struct BytecodeFunction {
    code: Arc<FuncByteCode>,
    links: LinkingTable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DebugOriginRange {
    pub start: usize,
    pub end: usize,
    pub origin: Origin,
}

impl BytecodeFunction {
    pub(crate) fn from_linked_code(code: Arc<FuncByteCode>) -> Self {
        Self {
            code,
            links: LinkingTable::default(),
        }
    }

    pub fn new(
        name: impl Into<Arc<str>>,
        register_count: usize,
        constants: Vec<Constant>,
        instructions: Vec<Instruction>,
    ) -> Self {
        Self::with_signature(name, 0, 0, register_count, constants, instructions)
    }

    pub fn with_signature(
        name: impl Into<Arc<str>>,
        parameter_count: usize,
        capture_count: usize,
        register_count: usize,
        constants: Vec<Constant>,
        instructions: Vec<Instruction>,
    ) -> Self {
        Self::assembled_constants(
            name,
            parameter_count,
            capture_count,
            register_count,
            constants,
            instructions,
            Vec::new(),
        )
    }

    pub(crate) fn assembled_constants(
        name: impl Into<Arc<str>>,
        parameter_count: usize,
        capture_count: usize,
        register_count: usize,
        constants: Vec<Constant>,
        instructions: Vec<Instruction>,
        debug_origins: Vec<DebugOriginRange>,
    ) -> Self {
        let mut links = LinkingTable {
            external_values: vec![None; constants.len()],
            values: constants,
            ..LinkingTable::default()
        };
        let instructions = instructions
            .into_iter()
            .map(|instruction| link_instruction(instruction, &mut links))
            .collect();
        Self {
            code: Arc::new(FuncByteCode {
                name: name.into(),
                parameter_count,
                capture_count,
                register_count,
                instructions,
                debug_origins,
            }),
            links,
        }
    }

    pub fn code(&self) -> &Arc<FuncByteCode> {
        &self.code
    }

    pub fn links(&self) -> &LinkingTable {
        &self.links
    }

    pub fn value_link(&self, id: ValueLinkId) -> Option<&Constant> {
        self.links.values.get(id.0)
    }

    pub fn text_link(&self, id: TextLinkId) -> Option<&str> {
        self.links.text.get(id.0).map(AsRef::as_ref)
    }

    pub fn prototype_link(&self, id: ProtoLinkId) -> Option<&Arc<BytecodeFunction>> {
        self.links.prototypes.get(id.0)
    }

    pub fn shares_code_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.code, &other.code)
    }

    pub fn relink(&self) -> Self {
        self.relink_with(Clone::clone, |text| text.into(), Arc::clone)
    }

    pub fn relink_with(
        &self,
        mut value: impl FnMut(&Constant) -> Constant,
        mut text: impl FnMut(&str) -> Arc<str>,
        mut prototype: impl FnMut(&Arc<BytecodeFunction>) -> Arc<BytecodeFunction>,
    ) -> Self {
        Self {
            code: Arc::clone(&self.code),
            links: LinkingTable {
                values: self.links.values.iter().map(&mut value).collect(),
                external_values: self.links.external_values.clone(),
                text: self.links.text.iter().map(|item| text(item)).collect(),
                prototypes: self.links.prototypes.iter().map(&mut prototype).collect(),
            },
        }
    }

    pub fn name(&self) -> &str {
        &self.code.name
    }

    pub fn register_count(&self) -> usize {
        self.code.register_count
    }

    pub fn parameter_count(&self) -> usize {
        self.code.parameter_count
    }

    pub fn capture_count(&self) -> usize {
        self.code.capture_count
    }

    pub fn constants(&self) -> &[Constant] {
        &self.links.values
    }

    pub(crate) fn bind_external_value(&mut self, index: usize, key: impl Into<Arc<str>>) {
        self.links
            .external_values
            .resize(self.links.values.len(), None);
        self.links.external_values[index] = Some(key.into());
    }

    pub fn instructions(&self) -> &[Opcode] {
        &self.code.instructions
    }

    pub fn origin_at(&self, instruction: usize) -> Option<Origin> {
        self.code
            .debug_origins
            .iter()
            .find(|range| range.start <= instruction && instruction < range.end)
            .map(|range| range.origin)
    }

    pub fn debug_origins(&self) -> &[DebugOriginRange] {
        &self.code.debug_origins
    }
}

fn link_instruction(instruction: Instruction, links: &mut LinkingTable) -> Opcode {
    let text = |value: String, links: &mut LinkingTable| {
        if let Some(index) = links.text.iter().position(|candidate| **candidate == value) {
            return TextLinkId(index);
        }
        let id = TextLinkId(links.text.len());
        links.text.push(value.into());
        id
    };
    match instruction {
        Instruction::LoadConst { dst, constant } => Opcode::LoadConst {
            dst,
            value: ValueLinkId(constant),
        },
        Instruction::Move { dst, src } => Opcode::Move { dst, src },
        Instruction::OwnDeclared { dst, owner, value } => Opcode::OwnDeclared { dst, owner, value },
        Instruction::AllocFunc { dst, static_id } => Opcode::AllocFunc { dst, static_id },
        Instruction::SealFunc { target, source } => Opcode::SealFunc { target, source },
        Instruction::AllocTypeSlot { dst } => Opcode::AllocTypeSlot { dst },
        Instruction::ReadTypeSlot { dst, link } => Opcode::ReadTypeSlot { dst, link },
        Instruction::SealTypeSlot { link, src } => Opcode::SealTypeSlot { link, src },
        Instruction::AssertTypeSlotReady { link } => Opcode::AssertTypeSlotReady { link },
        Instruction::Add { dst, left, right } => Opcode::Add { dst, left, right },
        Instruction::Subtract { dst, left, right } => Opcode::Subtract { dst, left, right },
        Instruction::Multiply { dst, left, right } => Opcode::Multiply { dst, left, right },
        Instruction::Divide { dst, left, right } => Opcode::Divide { dst, left, right },
        Instruction::Remainder { dst, left, right } => Opcode::Remainder { dst, left, right },
        Instruction::Negate { dst, src } => Opcode::Negate { dst, src },
        Instruction::Not { dst, src } => Opcode::Not { dst, src },
        Instruction::LogicalNot { dst, src } => Opcode::LogicalNot { dst, src },
        Instruction::BitNot { dst, src } => Opcode::BitNot { dst, src },
        Instruction::BitAnd { dst, left, right } => Opcode::BitAnd { dst, left, right },
        Instruction::BitOr { dst, left, right } => Opcode::BitOr { dst, left, right },
        Instruction::BitXor { dst, left, right } => Opcode::BitXor { dst, left, right },
        Instruction::Equal { dst, left, right } => Opcode::Equal { dst, left, right },
        Instruction::NotEqual { dst, left, right } => Opcode::NotEqual { dst, left, right },
        Instruction::LessThan { dst, left, right } => Opcode::LessThan { dst, left, right },
        Instruction::LessThanOrEqual { dst, left, right } => {
            Opcode::LessThanOrEqual { dst, left, right }
        }
        Instruction::MakeArray { dst, items } => Opcode::MakeArray { dst, items },
        Instruction::ConcatArrays { dst, arrays } => Opcode::ConcatArrays { dst, arrays },
        Instruction::MakeTuple { dst, items } => Opcode::MakeTuple { dst, items },
        Instruction::InterpolateString { dst, parts } => Opcode::InterpolateString { dst, parts },
        Instruction::MakeDict { dst, fields } => Opcode::MakeDict {
            dst,
            fields: fields
                .into_iter()
                .map(|(field, register)| (text(field, links), register))
                .collect(),
        },
        Instruction::MergeDicts { dst, dicts } => Opcode::MergeDicts { dst, dicts },
        Instruction::GetField { dst, dict, field } => Opcode::GetField {
            dst,
            dict,
            field: text(field, links),
        },
        Instruction::GetArray { dst, array, index } => Opcode::GetArray { dst, array, index },
        Instruction::ProjectTuple { dst, tuple, index } => {
            Opcode::ProjectTuple { dst, tuple, index }
        }
        Instruction::FieldExists { dst, value, field } => Opcode::FieldExists {
            dst,
            value,
            field: text(field, links),
        },
        Instruction::IsDict { dst, value } => Opcode::IsDict { dst, value },
        Instruction::TupleLengthEquals { dst, value, length } => {
            Opcode::TupleLengthEquals { dst, value, length }
        }
        Instruction::GetTuple { dst, tuple, index } => Opcode::GetTuple { dst, tuple, index },
        Instruction::TaggedTagEquals { dst, value, tag } => {
            Opcode::TaggedTagEquals { dst, value, tag }
        }
        Instruction::GetTaggedPayload { dst, value } => Opcode::GetTaggedPayload { dst, value },
        Instruction::MakeClosure {
            dst,
            function,
            captures,
        } => {
            let prototype = ProtoLinkId(links.prototypes.len());
            links.prototypes.push(function);
            Opcode::MakeClosure {
                dst,
                prototype,
                captures,
            }
        }
        Instruction::Call {
            base,
            argument_count,
        } => Opcode::Call {
            base,
            argument_count,
        },
        Instruction::TailCall {
            base,
            argument_count,
        } => Opcode::TailCall {
            base,
            argument_count,
        },
        Instruction::Jump { target } => Opcode::Jump { target },
        Instruction::JumpIfFalse { condition, target } => Opcode::JumpIfFalse { condition, target },
        Instruction::Return { src } => Opcode::Return { src },
        Instruction::Fail { message } => Opcode::Fail { message },
        Instruction::Panic { message } => Opcode::Panic { message },
        Instruction::Raise { error } => Opcode::Raise { error },
        Instruction::Debug {
            value,
            module,
            line,
            name,
            message,
        } => Opcode::Debug {
            value,
            module,
            line,
            name,
            message,
        },
    }
}

#[cfg(test)]
#[path = "bytecode/tests/mod.rs"]
mod tests;
