use crate::ast::{
    BindingKind, DictFieldKind, Expr, ExprKind, Program, StringPartKind, TypeArgumentKind, located,
};
use crate::compiler::{
    compile_program_analyzed_in_module, compile_program_with_promoted_types_and_static_funcs,
    function_contract_arity, metadata_compilation_plan, type_family_link_key,
    type_family_template_link_key,
};
use crate::core::{
    DEFAULT_ENTRY_MODULE, EES_DEFAULT_ENTRY_MODULE, EES_SERVE_ENTRY_MODULE, FMT_CAPABILITY_BINDING,
    FMT_MODULE, PRELUDE_MODULE, SERVE_ENTRY_MODULE, default_entry_source, ees_default_entry_source,
    ees_serve_entry_source, module_specs, serve_entry_source,
};
use crate::heap::{DecodedValue, Heap, Object, PersistentValue, Val, semantic_value_type_id};
use crate::json::{
    Provenance, SemanticDataTarget, ValidatedDataPlan, materialize_data_plan,
    validate_json_registered,
};
use crate::module_id::{
    ModuleCName, ModuleCatalogEntry, ModuleFormat, ModuleId, ModuleResolver, ModuleVendor,
    ResolvedModule, immediate_value, is_public_builtin_name,
};
use crate::parser::parse_registered;
use crate::semantic::{
    SemanticImport, SemanticModuleInput, SemanticModuleInterface, WorkspaceModuleKind,
    WorkspaceModuleState, WorkspaceSnapshot,
};
use crate::source::{Diagnostic, SourceDatabase};
use crate::toml::validate_toml_registered;
use crate::type_store::TypeStore;
use crate::types::{
    Analysis, ModuleAnalysisContext, ModuleInterface, PartialAnalysisControl, TraitImplementation,
    TypeDescriptor, TypeFamilyTemplate, TypeScheme, analyze_partial_types_recovered_with_query,
    analyze_program_with_bindings_observed, program_references_name, recovered_reference_locations,
};
#[cfg(test)]
use crate::vm::ValueRef;
use crate::vm::WorkWorld;
use crate::yaml::validate_yaml_registered;
use crate::{
    BuiltinAtom, BytecodeFunction, DebugSink, DiscardDebugSink, Instruction, Quota, QuotaAccount,
    Register, Vm,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

include!("module/data.rs");
include!("module/artifact.rs");
include!("module/graph.rs");
include!("module/loaded.rs");
include!("module/host.rs");
include!("module/engine.rs");
include!("module/workspace.rs");
include!("module/entry.rs");
include!("module/loader.rs");

#[cfg(test)]
#[path = "module/tests/mod.rs"]
mod tests;
