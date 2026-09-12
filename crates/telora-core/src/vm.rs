use crate::bytecode::{BytecodeFunction, Opcode, Register};
use crate::heap::{
    DecodedValue, Handle, Heap, HeapView, Object, PersistentValue, Val,
    publish_root, relocate_work_roots,
};
use crate::lir::RegisterId;
use crate::value::{
    BuiltinAtom, CoreArrayFunction, CoreCodecFunction,
    CoreDictFunction, CoreDynFunction, CoreEqFunction, CoreHashFunction,
    CoreJsonFunction, CorePathFunction, CoreRuntimeFunction,
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
include!("vm/solved-codec-encode.rs");
include!("vm/solved-codec-decode.rs");
include!("vm/solved-type-desc.rs");
include!("vm/solved-string-parse.rs");
include!("vm/solved-construction.rs");
include!("vm/array.rs");
include!("vm/string.rs");
include!("vm/path.rs");
include!("vm/hash.rs");
include!("vm/dict.rs");
include!("vm/dyn.rs");
include!("vm/solved-dyn.rs");
include!("vm/codec-entry.rs");
include!("vm/codec-value.rs");
include!("vm/codec-names.rs");
include!("vm/solved-schema.rs");
include!("vm/json.rs");
include!("vm/json-writer.rs");
include!("vm/solved-json.rs");
include!("vm/solved-parse.rs");
include!("vm/solved-eval.rs");
include!("vm/demand.rs");
include!("vm/solved-check.rs");
include!("vm/solved-tests.rs");
include!("vm/solved-run.rs");
include!("vm/solved-run-host.rs");
include!("vm/debug.rs");
include!("vm/helpers.rs");

#[cfg(test)]
#[path = "vm/tests/mod.rs"]
mod tests;
