use crate::bytecode::{BytecodeFunction, Opcode, Register};
use crate::heap::{
    DecodedValue, Handle, Heap, HeapView, Object, PersistentValue, PropertyKey, Val,
    publish_module_root, publish_root, relocate_work_roots, semantic_value_unwrap_bytes,
    semantic_value_wrapper_bytes, unwrap_semantic_value, wrap_semantic_value,
};
use crate::lir::RegisterId;
use crate::value::{
    BuiltinAtom, CoreArrayFunction, CoreBuiltinTypeFunction, CoreCodecFunction,
    CoreDiagnosticFunction, CoreDictFunction, CoreDynFunction, CoreEqFunction, CoreHashFunction,
    CoreJsonFunction, CoreModelFunction, CorePathFunction, CoreResultFunction, CoreRuntimeFunction,
    CoreStringFunction, CoreTypeDescFunction, NativeError, NativeKind, NativeLimit,
};
use crate::{Diagnostic, Origin, SourceDatabase};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fmt::Write;
use std::sync::Arc;

include!("vm/public.rs");
include!("vm/call-context.rs");
include!("vm/error.rs");
include!("vm/runtime.rs");
include!("vm/diagnostic-scope.rs");
include!("vm/execute.rs");
include!("vm/dispatch.rs");
include!("vm/array.rs");
include!("vm/string.rs");
include!("vm/path.rs");
include!("vm/hash.rs");
include!("vm/dict.rs");
include!("vm/model.rs");
include!("vm/type-desc.rs");
include!("vm/dyn.rs");
include!("vm/model-type.rs");
include!("vm/codec-entry.rs");
include!("vm/codec-encode.rs");
include!("vm/codec-type.rs");
include!("vm/codec-transform.rs");
include!("vm/codec-plan.rs");
include!("vm/codec-enum.rs");
include!("vm/codec-schema.rs");
include!("vm/result.rs");
include!("vm/json.rs");
include!("vm/json-writer.rs");
include!("vm/debug.rs");
include!("vm/helpers.rs");

#[cfg(test)]
#[path = "vm/tests/mod.rs"]
mod tests;
