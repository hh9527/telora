#![allow(
    clippy::chunks_exact_to_as_chunks,
    clippy::large_enum_variant,
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

pub mod ast;
pub mod bytecode;
pub mod codegen;
pub mod type_image;
pub mod candidate_layout;
#[cfg(feature = "experimental-layout-runtime")]
pub mod layout_runtime;
pub mod test_plan;
mod test_protocol;
pub use test_protocol::{TestContext, TestHost, TestLimits, TestSource};
pub mod execution_link;
pub mod execution_graph;
mod core;
pub mod document;
mod fmt;
mod heap;
pub mod json;
pub mod lexer;
pub mod lir;
pub mod mir;
pub mod mir_query;
pub mod static_sources;
#[path = "module-resolve.rs"]
pub mod module_resolve;
#[path = "symbol-resolve.rs"]
pub mod symbol_resolve;
#[path = "type-resolve.rs"]
pub mod type_resolve;
pub mod module_id;
pub mod package;
pub mod parser;
pub mod query;
mod regex;
pub mod runtime_host;
mod sha256;
pub mod source;
pub mod syntax;
pub mod toml;
mod type_id;
pub mod value;
pub mod vm;
pub mod yaml;

pub use bytecode::{
    BytecodeFunction, DebugOriginRange, FuncByteCode, Instruction, LinkingTable, Opcode,
    ProtoLinkId, Register, TextLinkId, ValueLinkId,
};
pub use document::{
    DocumentSnapshot, DocumentText, DocumentVersion, PositionEncoding, TextEdit, TextPosition,
};
pub use heap::TextRef;
pub use json::{
    JsonError, JsonParse, Provenance, SourcedValue, ValuePath, ValuePathSegment, parse_json,
    parse_json_registered, parse_json_with_provenance,
};
pub use lexer::{FrontendError, SourceLocation};
pub use module_id::{
    FIRST_DYNAMIC_MODULE_LOCAL, ModuleCName, ModuleCatalogEntry, ModuleCatalogOrigin,
    ModuleFormat, ModuleId, ModuleResolver, ModuleVendor, ModuleVisibility, ResolveModuleError,
    ResolvedModule, TraitId, TraitImplId, TypeConstructorId, resolve_root_module,
};
pub use package::{
    CONFIG_FILE, CRATE_FILE, CrateManifest, LOCK_FILE, LockedPackage, LockedSource,
    ModuleDeclaration, PackageError, RemoteSource, ResolvedWorkspace, UndeclaredModule,
    WorkspaceConfig, WorkspaceLock, WorkspaceSpec,
};
pub use query::{CancellationToken, QueryContext, QueryError, Revision, RevisionClock};
pub use runtime_host::{
    DataLimits, EesCall, EesReply, EntryDataSources, EvalContext, EvalSource, RunHost,
    RunHostFuture, RunOutcome, RunTermination, SystemCaps, SystemDataFormat, SystemDataSource,
    SystemEesModel, SystemEvent, SystemStdin, SystemTextSource,
};
pub use source::{
    Diagnostic, Label, Loc, Located, Location, Origin, SourceDatabase, SourceId, TextRange,
    WithOrigin,
};
pub use toml::{TomlParse, parse_toml_registered};
pub use type_id::TypeId;
pub use value::{Atom, BuiltinAtom, NativeError, NativeFunction, NativeType, OpaqueValue};
pub use vm::{
    CallContext, DataWorld, DebugEvent, DebugSink, DiscardDebugSink, ExecutionWorld, Quota,
    QuotaAccount, RuntimeError, RuntimeErrorKind, RuntimeFrame, ValueKind, ValueRef, Vm,
};
pub use yaml::{YamlParse, parse_yaml_registered};
