//! Semantic vocabulary shared by syntax lowering and the solved graph.
//! These tags do not own syntax trees.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlameAction {
    Build,
    Raise,
    Warn,
    Fail,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeclaredInitializerKind {
    Struct,
    Newtype,
    Enum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingKind {
    Let,
    Decl,
    Def,
    Native,
    NativeType,
    Type,
    Trait,
    Impl,
    Import,
    Export,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    Negate,
    Not,
    LogicalNot,
    BitNot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Equal,
    NotEqual,
    BitAnd,
    StructUpdate,
    BitOr,
    BitXor,
    And,
    Or,
}
