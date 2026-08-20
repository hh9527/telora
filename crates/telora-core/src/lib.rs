pub mod ast;
pub mod bytecode;
pub mod compiler;
mod core;
pub mod document;
mod elaboration;
mod evaluation;
mod fmt;
mod heap;
pub mod hir;
pub mod json;
pub mod lexer;
pub mod lir;
pub mod module;
pub mod module_id;
pub mod parser;
mod pattern;
pub mod query;
mod regex;
pub mod semantic;
mod sha256;
pub mod source;
pub mod syntax;
pub mod toml;
mod type_store;
pub mod types;
pub mod value;
pub mod vm;
pub mod workspace;
pub mod yaml;

pub use bytecode::{
    BytecodeFunction, DebugOriginRange, FuncByteCode, Instruction, LinkingTable, Opcode,
    ProtoLinkId, Register, TextLinkId, ValueLinkId,
};
pub use compiler::{ExecutionError, compile_source, run_source};
pub use document::{
    DocumentSnapshot, DocumentText, DocumentVersion, PositionEncoding, TextEdit, TextPosition,
};
pub use heap::TextRef;
pub use hir::{
    HirDefinition, HirDefinitionId, HirDefinitionKind, HirExpression, HirExpressionId, HirProgram,
    HirReference, HirReferenceId, HirResolution, HirTypeParameter,
};
pub use json::{
    JsonError, JsonParse, Provenance, SourcedValue, ValuePath, ValuePathSegment, parse_json,
    parse_json_registered, parse_json_with_provenance,
};
pub use lexer::{FrontendError, SourceLocation};
pub use module::{
    ChildExit, ChildOptions, ChildOutputMode, ChildSpawnResult, ChildStdinMode, ChildStdio,
    ChildText, Engine, EngineBuilder, EngineConfig, InstantiatedModule, LoadedModule,
    LoadedOptionAction, ModuleError, NativeModuleSpec, PendingModule, RunHost, RunHostFuture,
    RunOutcome, RunTermination, SpawnStdioChild, SystemEvent, evaluate_expression_module,
    evaluate_expression_module_with_quota, evaluate_expression_module_with_quota_and_debug_sink,
};
pub use module_id::{
    FIRST_DYNAMIC_MODULE_LOCAL, FuncId, ModuleAuthority, ModuleCName, ModuleCatalogEntry,
    ModuleCatalogOrigin, ModuleFormat, ModuleId, ModuleResolver, ModuleVisibility,
    ResolveModuleError, ResolvedModule, TypeConstructorId, resolve_root_module,
};
pub use query::{CancellationToken, QueryContext, QueryError, Revision, RevisionClock};
pub use semantic::{
    CompletionCandidate, CompletionKind, CompletionResult, Conflict, Definition, DefinitionId,
    DefinitionKind, DiagnosticId, FactIdentity, FactState, IncomputableReason, Reference,
    ReferenceId, SemanticFact, UnknownReason, WorkspaceExport, WorkspaceExpression,
    WorkspaceExpressionId, WorkspaceModule, WorkspaceModuleId, WorkspaceModuleKind,
    WorkspaceModuleState, WorkspaceSnapshot, WorkspaceTypeGraph, WorkspaceTypeId,
    WorkspaceTypeNode,
};
pub use source::{
    Diagnostic, Label, Loc, Located, Location, Origin, SourceDatabase, SourceId, TextRange,
    WithOrigin,
};
pub use toml::{TomlParse, parse_toml_registered};
pub use type_store::TypeId;
pub use types::{
    Analysis, AnalysisTypeId, DeclaredTypeDescriptor, ModuleInterface, PartialAnalysis,
    SemanticDependencyGraph, SemanticDependencyNode, TypeGraph, TypeNode, TypeParameter,
    TypeParameterId, TypeScheme, analyze_partial_types, analyze_partial_types_with_bindings,
    analyze_source, analyze_source_with_fuel, analyze_source_with_quota,
};
pub use value::{Atom, BuiltinAtom, NativeError, NativeFunction, NativeType, OpaqueValue};
pub use vm::{
    CallContext, DataWorld, DebugEvent, DebugSink, DiscardDebugSink, ExecutionWorld, Quota,
    QuotaAccount, RuntimeError, RuntimeErrorKind, RuntimeFrame, ValueKind, ValueRef, Vm,
};
pub use workspace::{Workspace, WorkspaceError};
pub use yaml::{YamlParse, parse_yaml_registered};
