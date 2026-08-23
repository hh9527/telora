use crate::ast::{
    BinaryOperator, Binding, BindingKind, Block, Expr, ExprKind, Pattern, Program, StringPartKind,
    TypeArgumentKind, UnaryOperator, located,
};
use crate::compiler::compile_expression_with_external_bindings;
use crate::heap::{Handle, Heap, PersistentValue, Val, publish_root};
use crate::hir::{HirDefinitionId, HirDefinitionKind, HirExpressionId, HirProgram, HirResolution};
use crate::json::{Provenance, ValuePath, ValuePathSegment};
use crate::lexer::{FrontendError, SourceLocation};
use crate::lir::RegisterId;
use crate::parser::parse_registered;
use crate::semantic::{
    Conflict, DiagnosticId, FactIdentity, FactState, IncomputableReason, SemanticFact,
    UnknownReason,
};
use crate::source::{Diagnostic, SourceDatabase};
use crate::type_store::{InternType, TypeId, TypeShape, TypeStore};
use crate::value::{
    Atom, CoreBuiltinTypeFunction, CoreDiagnosticFunction, CoreDynFunction, CoreModelFunction,
    NativeError, NativeFunction,
};
use crate::{
    BuiltinAtom, CallContext, DebugSink, DiscardDebugSink, Quota, QuotaAccount, ValueKind,
    ValueRef, Vm,
};
use hashbrown::raw::RawTable;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::Arc;

const DEFAULT_TOOL_FUEL: usize = 100_000;

include!("types/graph.rs");
include!("types/descriptor.rs");
include!("types/analysis.rs");
include!("types/dependency.rs");
include!("types/metadata.rs");
include!("types/tool.rs");
include!("types/prelude.rs");
include!("types/inference-state.rs");
include!("types/inference-context.rs");
include!("types/inference-unify.rs");
include!("types/inference-expression.rs");
include!("types/inference-utils.rs");
include!("types/expression.rs");
include!("types/relations.rs");

#[cfg(test)]
#[path = "types/tests/mod.rs"]
mod tests;
