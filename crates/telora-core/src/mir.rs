//! Session-owned, partially solved IR. No resolver, type checker or VM lives here.
use crate::ast::{
    BinaryOperator, BindingKind, BlameAction, DeclaredInitializerKind, UnaryOperator,
};
use crate::source::{Diagnostic, Location, SourceDatabase, SourceId};
use crate::syntax::telora::parser::CstData;
use std::fmt::Write;

#[path = "mir/lower.rs"]
pub(crate) mod lower;
#[path = "mir/seal.rs"]
mod seal;
pub use seal::SealedMir;

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
id!(ScopeId);
id!(TypeTermId, TypeConflictId);
id!(GenericInstanceId);

/// A statically instantiated declaration. The source HIR is shared; this
/// instance supplies its normalized types and reference edges without cloning
/// syntax or requiring downstream consumers to apply substitutions.
#[derive(Debug)]
pub struct GenericInstance {
    pub symbol: SymbolId,
    pub concrete: bool,
    pub arguments: Vec<(SymbolId, TypeId)>,
    pub signature: TypeId,
    pub types: Vec<(HirId, TypeId)>,
    pub references: Vec<(HirId, GenericInstanceId)>,
}

impl GenericInstance {
    pub fn ty(&self, node: HirId) -> Option<TypeId> {
        self.types.binary_search_by_key(&node, |(node, _)| *node)
            .ok().map(|index| self.types[index].1)
    }

    pub fn reference(&self, node: HirId) -> Option<GenericInstanceId> {
        self.references.binary_search_by_key(&node, |(node, _)| *node)
            .ok().map(|index| self.references[index].1)
    }
}

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
    Data {
        body: HirId,
    },
    Unavailable(String),
}

#[derive(Debug)]
pub struct Module {
    pub native: Option<NativeModule>,
    pub name: String,
    pub kind: ModuleKind,
    pub state: ModuleState,
    pub imports: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NativeTypeId {
    pub module: u32,
    pub slot: u32,
}
impl NativeTypeId {
    /// Native ABI identity used by diagnostic syntax and the source inventory.
    pub const BLAME_ERROR: Self = Self {
        module: 34,
        slot: 0,
    };
}
#[derive(Clone, Debug)]
pub enum NativeTypeRule {
    Primitive(TypeConstructor),
    Constructor(TypeFunction),
    Opaque,
}
#[derive(Clone, Debug)]
pub struct NativeModule {
    pub id: u32,
    pub types: Vec<(u32, NativeTypeRule)>,
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
    /// A type-directed member constraint, not an unperformed name lookup.
    Member {
        receiver: HirId,
        name: HirId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Declaration(BindingKind),
    Parameter,
    TypeParameter,
    Pattern,
    Import,
    Export,
    Namespace(ModuleId),
}
#[derive(Debug)]
pub struct Symbol {
    pub native_type: Option<NativeTypeId>,
    pub module: Option<ModuleId>,
    pub name: String,
    pub kind: SymbolKind,
    pub declarations: Vec<HirId>,
    pub scope: Option<ScopeId>,
    pub resolution: ResolveState,
}
#[derive(Clone, Copy, Debug)]
pub struct ScopeBinding {
    pub symbol: SymbolId,
    pub after: Option<HirId>,
}
#[derive(Debug)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub module: ModuleId,
    pub bindings: Vec<ScopeBinding>,
    pub open_imports: Vec<usize>,
}
#[derive(Debug)]
pub enum ResolveConflict {
    DuplicateDefinition {
        name: String,
        definitions: Vec<SymbolId>,
    },
    AmbiguousImport {
        name: String,
        candidates: Vec<SymbolId>,
    },
    ModuleCandidates {
        candidates: Vec<ModuleId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeState {
    Unknown,
    ProxyTo(TypeSlotId),
    /// A provisional constructor whose arguments may still be unsolved slots.
    Structure(TypeTermId),
    Known(TypeId),
    Conflicted(TypeConflictId),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypeConstructor {
    Int,
    Float,
    String,
    Bytes,
    Bool,
    Never,
    Type,
    TypeOf,
    Dyn,
    Option,
    Result,
    FoldControl,
    PropertyTarget,
    PropertyBound,
    Unchecked,
    TypeFunction(TypeFunction),
    Nominal(SymbolId),
    Native(NativeTypeId),
    Tuple,
    Array,
    ArrayLiteral,
    TypeList,
    Dict,
    Function,
    Record(Vec<String>),
    Meta,
    Namespace(ModuleId),
    Parameter(SymbolId),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypeFunction {
    Array,
    Dict,
    Option,
    Result,
    FoldControl,
    TypeOf,
    Unchecked,
    Tuple,
    Func,
    Property,
}

#[derive(Debug)]
pub struct TypeDefinition {
    pub symbol: SymbolId,
    pub operation: TypeOperation,
    pub parameters: Vec<SymbolId>,
    pub members: Vec<TypeMember>,
}
#[derive(Debug)]
pub struct TypeMember {
    pub name: String,
    pub syntax: HirId,
    pub payload: Option<TypeSlotId>,
}

#[derive(Clone, Copy, Debug)]
pub enum MemberSelection {
    TraitMember {
        index: u32,
        implementation: Option<SymbolId>,
    },
    PropertyTarget(u8),
    Boolean(bool),
    RecordField,
    EnumVariant {
        index: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PropertySite {
    Type,
    Field(u32),
    Variant(u32),
}

/// One static presence fact. Providers are retained in declaration/reduce order;
/// their code is executed only by the subsequent metadata stage.
#[derive(Debug)]
pub struct PropertyRecord {
    pub owner: TypeId,
    pub site: PropertySite,
    pub property: TypeId,
    pub providers: Vec<HirId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundState {
    Pending,
    Assumed(SymbolId),
    Property(usize),
    Implementation(SymbolId),
    Ambiguous,
    Unresolved,
    Rejected,
}

impl BoundState {
    pub fn is_proven(self) -> bool {
        matches!(
            self,
            Self::Assumed(_) | Self::Property(_) | Self::Implementation(_)
        )
    }
}

#[derive(Clone, Debug)]
pub struct TraitImplementation {
    pub symbol: SymbolId,
    /// A trait skeleton applied to its target, possibly with rigid parameters.
    pub trait_type: TypeId,
    pub requirements: Vec<(SymbolId, TypeId)>,
}

#[derive(Debug)]
pub struct BoundRequirement {
    pub subject: TypeSlotId,
    /// The instantiated type expression describing the bound.
    pub bound: TypeSlotId,
    pub reference: HirId,
    pub state: BoundState,
    pub evidence: Option<usize>,
}

/// A node in the session's static evidence graph, retained for code generation.
#[derive(Debug)]
pub struct EvidenceNode {
    pub subject: TypeId,
    pub bound: TypeId,
    pub state: BoundState,
    pub implementation: Option<SymbolId>,
    pub arguments: Vec<(SymbolId, TypeId)>,
    pub dependencies: Vec<usize>,
}
#[derive(Clone, Debug)]
pub struct TypeTerm {
    pub constructor: TypeConstructor,
    pub arguments: Vec<TypeSlotId>,
}
#[derive(Clone, Debug)]
pub struct ResolvedType {
    pub constructor: TypeConstructor,
    pub arguments: Vec<TypeId>,
}
#[derive(Debug)]
pub struct TypeConflict {
    pub left: TypeSlotId,
    pub right: TypeSlotId,
    pub location: Option<Location>,
    pub message: String,
    pub resolve_origin: Option<ConflictId>,
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
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub role: Role,
    pub node: HirId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeOperation {
    Function,
    Tuple,
    Unit,
    Struct,
    Newtype,
    Enum,
}

/// Each operation retains its syntax payload and explicitly labelled child
/// edges. No node owns another HIR node or contains a resolved type descriptor.
#[derive(Debug)]
pub enum HirKind {
    NativeTypeSlot(i64),
    TypeOperation(TypeOperation),
    TypeMember {
        name: String,
        nullary: bool,
    },
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
    pub symbols: Vec<Symbol>,
    pub scopes: Vec<Scope>,
    pub hir_scopes: Vec<Option<ScopeId>>,
    pub hir_symbols: Vec<Option<SymbolId>>,
    pub module_scopes: Vec<Option<ScopeId>>,
    pub exports: Vec<Vec<SymbolId>>,
    pub resolve_conflicts: Vec<ResolveConflict>,
    pub symbols_closed: bool,
    pub ty_slots: Vec<TypeState>,
    pub symbol_types: Vec<TypeSlotId>,
    pub symbol_generics: Vec<Vec<SymbolId>>,
    /// Per-reference generic substitutions produced by the type pass. Argument
    /// slots are normalized with the rest of the graph; consumers must use
    /// their solved outcome rather than matching signatures again. A generic
    /// body may refer to a rigid outer parameter here.
    pub type_instances: Vec<Vec<(SymbolId, TypeSlotId)>>,
    pub generic_instances: Vec<GenericInstance>,
    pub reference_instances: Vec<Option<GenericInstanceId>>,
    pub implementation_instances: Vec<Option<GenericInstanceId>>,
    pub type_terms: Vec<TypeTerm>,
    pub types: Vec<ResolvedType>,
    pub member_selections: Vec<Option<MemberSelection>>,
    pub type_definitions: Vec<TypeDefinition>,
    pub properties: Vec<PropertyRecord>,
    pub bound_requirements: Vec<BoundRequirement>,
    pub trait_implementations: Vec<TraitImplementation>,
    pub evidence: Vec<EvidenceNode>,
    pub type_conflicts: Vec<TypeConflict>,
    pub type_unknowns: Vec<TypeSlotId>,
    pub required_types: Vec<bool>,
    pub types_solved: bool,
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
        let resolution = matches!(
            kind,
            HirKind::Variable(_) | HirKind::PatternName(_) | HirKind::Field
        )
        .then(|| {
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
        writeln!(
            out,
            "mir modules={} hir={} resolve_slots={} ty_slots={} symbols_closed={} types_solved={}",
            self.modules.len(),
            self.hir.len(),
            self.resolve_slots.len(),
            self.ty_slots.len(),
            self.symbols_closed,
            self.types_solved
        )
        .unwrap();
        writeln!(out, "roots {:?}", self.roots).unwrap();
        for (id, module) in self.modules.iter().enumerate() {
            let state = match &module.state {
                ModuleState::Unloaded => "unloaded".into(),
                ModuleState::Data { body } => {
                    format!("data (static export: data: Value) contract={body:?}")
                }
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
        for (id, scope) in self.scopes.iter().enumerate() {
            writeln!(out, "scope {id} {scope:?}").unwrap();
        }
        for (id, symbol) in self.symbols.iter().enumerate() {
            writeln!(out, "symbol {id} {symbol:?}").unwrap();
        }
        for (id, conflict) in self.resolve_conflicts.iter().enumerate() {
            writeln!(out, "resolve-conflict {id} {conflict:?}").unwrap();
        }
        for (id, term) in self.type_terms.iter().enumerate() {
            writeln!(out, "type-term {id} {term:?}").unwrap();
        }
        for definition in &self.type_definitions {
            writeln!(out, "type-definition {definition:?}").unwrap();
        }
        for (id, property) in self.properties.iter().enumerate() {
            writeln!(out, "property {id} {property:?}").unwrap();
        }
        for (id, bound) in self.bound_requirements.iter().enumerate() {
            writeln!(out, "bound {id} {bound:?}").unwrap();
        }
        for implementation in &self.trait_implementations {
            writeln!(out, "trait-impl {implementation:?}").unwrap();
        }
        for (id, evidence) in self.evidence.iter().enumerate() {
            writeln!(out, "evidence {id} {evidence:?}").unwrap();
        }
        for (id, parameters) in self.symbol_generics.iter().enumerate() {
            if !parameters.is_empty() {
                writeln!(out, "symbol-generics {id} {parameters:?}").unwrap();
            }
        }
        for (id, arguments) in self.type_instances.iter().enumerate() {
            if !arguments.is_empty() {
                writeln!(out, "type-instance {id} {arguments:?}").unwrap();
            }
        }
        for (id, instance) in self.generic_instances.iter().enumerate() {
            writeln!(out, "generic-instance {id} {instance:?}").unwrap();
        }
        for (id, instance) in self.reference_instances.iter().enumerate() {
            if let Some(instance) = instance {
                writeln!(out, "reference-instance {id} {instance:?}").unwrap();
            }
        }
        for (id, instance) in self.implementation_instances.iter().enumerate() {
            if let Some(instance) = instance {
                writeln!(out, "implementation-instance {id} {instance:?}").unwrap();
            }
        }
        for (id, ty) in self.types.iter().enumerate() {
            writeln!(out, "type {id} {ty:?}").unwrap();
        }
        for (id, selection) in self.member_selections.iter().enumerate() {
            if let Some(selection) = selection {
                writeln!(out, "member-selection {id} {selection:?}").unwrap();
            }
        }
        for (id, conflict) in self.type_conflicts.iter().enumerate() {
            writeln!(out, "type-conflict {id} {conflict:?}").unwrap();
        }
        for (id, slot) in self.symbol_types.iter().enumerate() {
            writeln!(
                out,
                "symbol-type {id} {slot:?} => {:?}",
                self.ty_slots[slot.index()]
            )
            .unwrap();
        }
        writeln!(out, "type-unknowns {:?}", self.type_unknowns).unwrap();
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
