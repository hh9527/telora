#![allow(
    clippy::chunks_exact_to_as_chunks,
    clippy::large_enum_variant,
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

pub mod ast;
pub mod entry_plan;
pub mod type_image;
pub mod candidate_layout;
#[cfg(feature = "experimental-layout-runtime")]
pub mod layout_runtime;
pub mod test_plan;
mod test_protocol;
pub use test_protocol::{TestContext, TestHost, TestLimits, TestSource};
pub mod document;
pub mod json;
pub mod data_plan;
pub mod lexer;
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
pub mod runtime_host;
pub mod source;
pub mod syntax;
pub mod toml;
pub mod yaml;

pub use document::{
    DocumentSnapshot, DocumentText, DocumentVersion, PositionEncoding, TextEdit, TextPosition,
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
    DataLimits, EesCall, EesReply, EntryDataSources, EvalSource, RunHost,
    RunHostFuture, SystemCaps, SystemDataFormat, SystemDataSource,
    SystemEesModel, SystemEvent, SystemStdin, SystemTextSource,
};
pub use source::{
    Diagnostic, Label, Loc, Located, Location, Origin, SourceDatabase, SourceId, TextRange,
    WithOrigin,
};
#[cfg(test)]
mod test_graph;
#[cfg(test)]
mod data_plan_test;
