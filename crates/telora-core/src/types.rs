use crate::ast::{
    BinaryOperator, Binding, BindingKind, Block, Expr, ExprKind, Pattern, Program, StringPartKind,
    TypeArgumentKind, UnaryOperator, located,
};
use crate::compiler::prepare_expression_with_external_bindings;
use crate::heap::{
    Handle, Heap, HeapView, PersistentValue, PropertyKey, Val, publish_root,
    publish_type_properties,
};
use crate::hir::{HirDefinitionId, HirDefinitionKind, HirExpressionId, HirProgram, HirResolution};
use crate::json::{Provenance, ValuePath, ValuePathSegment};
use crate::lexer::{FrontendError, SourceLocation};
use crate::lir::RegisterId;
use crate::parser::parse_registered;
mod facts;
pub use facts::{
    Conflict, DiagnosticId, FactIdentity, FactState, IncomputableReason, SemanticFact,
    UnknownReason,
};
use crate::source::{Diagnostic, SourceDatabase};
use crate::type_store::{InternType, TypeId, TypeShape, TypeStore};
use crate::value::{
    Atom, CoreBuiltinTypeFunction, CoreDynFunction, CoreModelFunction,
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
include!("types/environment.rs");
include!("types/traits.rs");
include!("types/analysis.rs");
include!("types/partial-solver.rs");
include!("types/program-solver.rs");
include!("types/type-check.rs");
include!("types/dependency.rs");
include!("types/type-boundary.rs");
include!("types/static-contract.rs");
include!("types/static-family.rs");
include!("types/static-origin.rs");
include!("types/static-constraints.rs");
include!("types/dependency-plan.rs");
include!("types/solved-module.rs");
include!("types/metadata.rs");
include!("types/host-contract.rs");
include!("types/tool.rs");
include!("types/tool-plan.rs");
include!("types/declaration-plan.rs");
include!("types/owner-plan.rs");
include!("types/annotations.rs");
include!("types/tool-bindings.rs");
include!("types/properties.rs");
include!("types/construction.rs");
include!("types/prelude.rs");
include!("types/inference-state.rs");
#[cfg(feature = "inference-profile")]
include!("types/inference-profile.rs");
include!("types/inference-variables.rs");
include!("types/inference-table.rs");
include!("types/inference-inputs.rs");
include!("types/inference-query.rs");
include!("types/inference-structures.rs");
include!("types/inference-publication.rs");
include!("types/inference-context.rs");
include!("types/inference-enum.rs");
include!("types/inference-unify.rs");
include!("types/inference-expression.rs");
include!("types/inference-utils.rs");
include!("types/expression.rs");
include!("types/relations.rs");

#[cfg(test)]
#[path = "types/tests/mod.rs"]
mod tests;
