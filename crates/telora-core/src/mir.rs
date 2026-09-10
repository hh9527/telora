//! Session-owned, partially solved IR. No resolver, type checker or VM lives here.
use crate::ast::{
    BinaryOperator, BindingKind, BlameAction, DeclaredInitializerKind, UnaryOperator,
};
use crate::source::{Diagnostic, Location, SourceDatabase, SourceId};
use crate::syntax::telora::parser::CstData;
use std::fmt::Write;

#[path = "mir/lower.rs"]
pub(crate) mod lower;

macro_rules! id {
    ($($name:ident),*) => {$(
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub(crate) u32);
        impl $name { pub fn index(self) -> usize { self.0 as usize } }
    )*};
}
id!(
    ModuleId,
    HirId,
    ResolveSlotId,
    SymbolId,
    TypeSlotId,
    TypeId,
    ConflictId
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleTarget {
    Bound(ModuleId),
    Unresolved(String),
    Conflicted(Vec<ModuleId>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    Source,
    Data,
}

#[derive(Debug)]
pub enum ModuleState {
    Unloaded,
    Source {
        source: SourceId,
        cst: CstData,
        body: HirId,
    },
    Data,
    Unavailable(String),
}

#[derive(Debug)]
pub struct Module {
    pub name: String,
    pub kind: ModuleKind,
    pub state: ModuleState,
    pub imports: Vec<usize>,
}

#[derive(Debug)]
pub struct Import {
    pub owner: ModuleId,
    /// None for a configured implicit dependency, such as the prelude.
    pub syntax: Option<HirId>,
    pub request: String,
    pub target: ModuleTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveState {
    Pending,
    Bound(SymbolId),
    Unresolved,
    Conflicted(ConflictId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeState {
    Unknown,
    ProxyTo(TypeSlotId),
    Known(TypeId),
    Conflicted(ConflictId),
}

/// Syntax-owned type slots use the HIR node's index; additional inference slots
/// may be appended later without changing those identities.
#[derive(Debug)]
pub struct HirNode {
    pub module: ModuleId,
    pub location: Location,
    pub kind: HirKind,
    pub children: Vec<Edge>,
    pub resolution: Option<ResolveSlotId>,
}
impl HirId {
    pub fn ty(self) -> TypeSlotId {
        TypeSlotId(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Binding,
    Result,
    Name,
    TypeParameter,
    Bound,
    Annotation,
    Value,
    Decorator,
    Callee,
    Argument,
    Operand,
    Left,
    Right,
    Receiver,
    Target,
    Namespace,
    Index,
    Parameter,
    ReturnType,
    Body,
    Condition,
    Then,
    Else,
    Pattern,
    Guard,
    Arm,
    Field,
    Item,
    Subject,
    Elaboration,
    Part,
}
#[derive(Debug)]
pub struct Edge {
    pub role: Role,
    pub node: HirId,
}

/// Each operation retains its syntax payload and explicitly labelled child
/// edges. No node owns another HIR node or contains a resolved type descriptor.
#[derive(Debug)]
pub enum HirKind {
    Block,
    Binding {
        kind: BindingKind,
        initializer: Option<DeclaredInitializerKind>,
        imported: Option<String>,
    },
    Name(String),
    Parameter,
    TypeParameter,
    ReturnType,
    Decorator {
        configured: bool,
    },
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Atom(String),
    Variable(String),
    InterpolatedString,
    Text(String),
    Array,
    Tuple,
    Spread,
    TypeSyntax,
    TypeMetadata,
    Dict,
    DictField,
    Unary(UnaryOperator),
    Propagate,
    Return,
    Panic,
    Raise(BlameAction),
    Debug {
        message: Option<String>,
        expression: String,
    },
    TypeAscription,
    CheckedCast,
    DynProject,
    Binary(BinaryOperator),
    Field,
    FieldProjection,
    Index,
    TupleProjection(usize),
    Call,
    TypeApply,
    InferredTypeArgument,
    Interpreter,
    Closure,
    If,
    IfLet,
    LetElse,
    Match,
    MatchArm {
        irrefutable: bool,
    },
    Wildcard,
    PatternName(String),
    TaggedPattern(String),
    ConstructorPattern,
    TuplePattern,
    StructPattern,
    PatternField,
}

#[derive(Default)]
pub struct Mir {
    pub sources: SourceDatabase,
    pub modules: Vec<Module>,
    pub roots: Vec<ModuleTarget>,
    pub imports: Vec<Import>,
    pub hir: Vec<HirNode>,
    pub resolve_slots: Vec<ResolveState>,
    pub ty_slots: Vec<TypeState>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Mir {
    pub(crate) fn node(
        &mut self,
        module: ModuleId,
        location: Location,
        kind: HirKind,
        children: Vec<Edge>,
    ) -> HirId {
        assert_eq!(
            self.hir.len(),
            self.ty_slots.len(),
            "syntax slots precede inference-only slots"
        );
        let resolution =
            matches!(kind, HirKind::Variable(_) | HirKind::PatternName(_)).then(|| {
                let id = ResolveSlotId(
                    self.resolve_slots
                        .len()
                        .try_into()
                        .expect("resolve slot capacity"),
                );
                self.resolve_slots.push(ResolveState::Pending);
                id
            });
        let id = HirId(self.hir.len().try_into().expect("HIR capacity"));
        self.hir.push(HirNode {
            module,
            location,
            kind,
            children,
            resolution,
        });
        self.ty_slots.push(TypeState::Unknown);
        id
    }

    /// A read-only dump: never resolves a name, shortens a proxy or evaluates code.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        writeln!(out, "roots {:?}", self.roots).unwrap();
        for (id, module) in self.modules.iter().enumerate() {
            let state = match &module.state {
                ModuleState::Unloaded => "unloaded".into(),
                ModuleState::Data => "data (static export: data: Value)".into(),
                ModuleState::Unavailable(message) => format!("unavailable {message:?}"),
                ModuleState::Source { source, body, .. } => {
                    format!("source {source:?} CST attached body={body:?}")
                }
            };
            writeln!(out, "module {id} {:?}: {state}", module.name).unwrap();
        }
        for (id, edge) in self.imports.iter().enumerate() {
            writeln!(
                out,
                "import {id} {:?} {:?} => {:?} syntax={:?}",
                edge.owner, edge.request, edge.target, edge.syntax
            )
            .unwrap();
        }
        for (id, node) in self.hir.iter().enumerate() {
            writeln!(
                out,
                "hir {id} {:?} {:?} {:?} edges={:?} resolve={:?} ty={:?}",
                node.module,
                node.location,
                node.kind,
                node.children,
                node.resolution
                    .map(|slot| (slot, &self.resolve_slots[slot.index()])),
                self.ty_slots[id]
            )
            .unwrap();
        }
        for diagnostic in &self.diagnostics {
            writeln!(out, "diagnostic {diagnostic:?}").unwrap();
        }
        out
    }
}
