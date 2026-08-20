use crate::bytecode::{BytecodeFunction, Opcode, Register};
use crate::heap::{
    DecodedValue, Handle, Heap, HeapView, Object, PersistentValue, Val, publish_module_root,
    publish_root, relocate_work_roots, semantic_value_unwrap_bytes, semantic_value_wrapper_bytes,
    unwrap_semantic_value, wrap_semantic_value,
};
use crate::lir::RegisterId;
use crate::value::{
    BuiltinAtom, CoreArrayFunction, CoreAttributesFunction, CoreBuiltinTypeFunction,
    CoreCodecFunction, CoreDiagnosticFunction, CoreDictFunction, CoreDynFunction, CoreEqFunction,
    CoreHashFunction, CoreJsonFunction, CoreModelFunction, CorePathFunction, CoreResultFunction,
    CoreStringFunction, CoreTypeDescFunction, NativeError, NativeKind, NativeLimit,
};
use crate::{Diagnostic, Origin, SourceDatabase};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fmt::Write;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugEvent {
    pub name: String,
    pub repr: String,
    pub module: String,
    pub line: u32,
    pub message: Option<String>,
}

pub trait DebugSink: Send + Sync {
    fn emit(&self, event: DebugEvent);
}

#[derive(Debug, Default)]
pub struct DiscardDebugSink;

impl DebugSink for DiscardDebugSink {
    fn emit(&self, _event: DebugEvent) {}
}

const MAX_CALL_DEPTH: usize = 1_024;
const MAX_STACK_SLOTS: usize = 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Quota {
    pub fuel: usize,
    pub stack_slots: usize,
    pub allocation_bytes: u64,
}

impl Quota {
    pub const fn new(fuel: usize, stack_slots: usize, allocation_bytes: u64) -> Self {
        Self {
            fuel,
            stack_slots,
            allocation_bytes,
        }
    }

    pub const fn with_fuel(fuel: usize) -> Self {
        Self::new(fuel, MAX_STACK_SLOTS, u64::MAX)
    }
}

#[derive(Debug)]
pub struct QuotaAccount {
    quota: Quota,
    remaining_fuel: usize,
    requested_allocation_bytes: u64,
    query: Option<crate::query::QueryContext>,
    diagnostics: Vec<Diagnostic>,
}

impl QuotaAccount {
    pub fn new(quota: Quota) -> Self {
        Self {
            remaining_fuel: quota.fuel,
            quota,
            requested_allocation_bytes: 0,
            query: None,
            diagnostics: Vec::new(),
        }
    }

    pub fn with_query(mut self, query: crate::query::QueryContext) -> Self {
        self.query = Some(query);
        self
    }

    pub const fn quota(&self) -> Quota {
        self.quota
    }

    pub const fn remaining_fuel(&self) -> usize {
        self.remaining_fuel
    }

    pub const fn requested_allocation_bytes(&self) -> u64 {
        self.requested_allocation_bytes
    }

    pub(crate) fn query_context(&self) -> Option<crate::query::QueryContext> {
        self.query.clone()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    fn stack_limit(&self) -> usize {
        self.quota.stack_slots.min(MAX_STACK_SLOTS)
    }

    fn charge_allocation(&mut self, bytes: u64) -> Result<(), ()> {
        let requested = self
            .requested_allocation_bytes
            .checked_add(bytes)
            .ok_or(())?;
        if requested > self.quota.allocation_bytes {
            return Err(());
        }
        self.requested_allocation_bytes = requested;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    Int,
    Float,
    String,
    Bytes,
    Type,
    Opaque,
    Dict,
    Array,
    Atom,
    Tagged,
    Tuple,
    Func,
    Dyn,
    Module,
}

#[derive(Clone, Copy)]
pub struct ValueRef<'a> {
    value: Val,
    view: HeapView<'a>,
}

pub struct ExecutionWorld {
    main: Arc<Heap>,
    work: WorkWorld,
}

#[derive(Clone)]
pub struct DataWorld {
    heap: Arc<Heap>,
    root: Val,
}

impl DataWorld {
    pub(crate) fn new(heap: Heap, root: Val) -> Self {
        Self {
            heap: Arc::new(heap),
            root,
        }
    }

    pub fn int(value: i64) -> Self {
        Self::new(Heap::work(), Val::unknown(DecodedValue::Int(value)))
    }

    pub fn float(value: f64) -> Result<Self, &'static str> {
        value
            .is_finite()
            .then(|| Self::new(Heap::work(), Val::unknown(DecodedValue::Float(value))))
            .ok_or("Telora Float must be finite")
    }

    pub fn string(value: &str) -> Self {
        let mut heap = Heap::work();
        let root = Val::unknown(heap.string(None, value));
        Self::new(heap, root)
    }

    pub fn value(&self) -> ValueRef<'_> {
        ValueRef {
            value: self.root,
            view: HeapView {
                current: &self.heap,
                background: None,
            },
        }
    }

    pub(crate) fn publish(
        &self,
        main: &mut Heap,
    ) -> Result<PersistentValue, crate::heap::HeapError> {
        publish_root(main, &self.heap, self.root)
    }

    pub(crate) fn relocate_into(
        &self,
        target: &mut Heap,
        main: &Heap,
    ) -> Result<Val, crate::heap::HeapError> {
        relocate_work_roots(target, main, &self.heap, &[self.root]).map(|roots| roots[0])
    }
}

impl fmt::Debug for DataWorld {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DataWorld")
            .field("value", &self.value().to_string())
            .finish()
    }
}

impl fmt::Display for DataWorld {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value().fmt(formatter)
    }
}

impl ExecutionWorld {
    pub(crate) fn new(main: Arc<Heap>, work: WorkWorld) -> Self {
        Self { main, work }
    }

    pub fn value(&self) -> ValueRef<'_> {
        ValueRef::work(self.work.root, &self.work.heap, &self.main)
    }

    pub fn select(mut self, field: &str) -> Result<Self, String> {
        let selected = self
            .value()
            .dict_get(field)
            .or_else(|| self.value().module_get(field))
            .ok_or_else(|| format!("value has no field {field:?}"))?
            .value;
        self.work.root = selected;
        Ok(self)
    }

    pub(crate) fn into_parts(self) -> (Arc<Heap>, WorkWorld) {
        (self.main, self.work)
    }

    pub fn format(&self) -> Result<String, String> {
        DebugValueFormatter::new(HeapView {
            current: &self.work.heap,
            background: Some(&self.main),
        })
        .format(self.work.root)
        .map_err(|error| error.to_string())
    }
}

impl fmt::Debug for ExecutionWorld {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutionWorld")
            .field("value", &self.format())
            .finish_non_exhaustive()
    }
}

impl fmt::Display for ExecutionWorld {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.format() {
            Ok(value) => formatter.write_str(&value),
            Err(error) => write!(formatter, "<invalid value: {error}>"),
        }
    }
}

impl<'a> ValueRef<'a> {
    pub(crate) fn runtime(self) -> Val {
        self.value
    }

    pub(crate) fn persistent(value: PersistentValue, heap: &'a Heap) -> Self {
        Self {
            value: value.runtime(),
            view: HeapView {
                current: heap,
                background: None,
            },
        }
    }

    pub(crate) fn work(value: Val, work: &'a Heap, main: &'a Heap) -> Self {
        Self {
            value,
            view: HeapView {
                current: work,
                background: Some(main),
            },
        }
    }

    pub(crate) fn hidden_type_slot_handle(self) -> Option<Handle> {
        let DecodedValue::TypeSlot(handle) = self.value.value() else {
            return None;
        };
        Some(handle)
    }

    pub(crate) fn object_handle(self) -> Option<Handle> {
        match self.value.value() {
            DecodedValue::Bytes(handle)
            | DecodedValue::Opaque(handle)
            | DecodedValue::DeclaredType(handle)
            | DecodedValue::SymbolicType(handle)
            | DecodedValue::Array(handle)
            | DecodedValue::Tagged(handle)
            | DecodedValue::Tuple(handle)
            | DecodedValue::Dict(handle)
            | DecodedValue::Func(handle)
            | DecodedValue::Dyn(handle) => Some(handle),
            _ => None,
        }
    }

    pub(crate) fn is_hidden_type_slot(self) -> bool {
        matches!(self.value.value(), DecodedValue::TypeSlot(_))
    }

    pub(crate) fn resolve_hidden_type_slot(self) -> Result<Self, String> {
        let DecodedValue::TypeSlot(handle) = self.value.value() else {
            return Ok(self);
        };
        let value = self
            .view
            .type_slot(handle)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "recursive type link is not initialized".to_owned())?;
        Ok(Self {
            value,
            view: self.view,
        })
    }

    pub fn kind(self) -> ValueKind {
        match self.value.value() {
            DecodedValue::Failed(_) => {
                unreachable!("failed nodes are private best-effort values")
            }
            DecodedValue::Int(_) => ValueKind::Int,
            DecodedValue::Float(_) => ValueKind::Float,
            DecodedValue::InlineString(_) | DecodedValue::ShortString(_) => ValueKind::String,
            DecodedValue::Bytes(_) => ValueKind::Bytes,
            DecodedValue::NativeType(_) => ValueKind::Type,
            DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_) => ValueKind::Type,
            DecodedValue::Opaque(_) => ValueKind::Opaque,
            DecodedValue::Dict(_) => ValueKind::Dict,
            DecodedValue::Array(_) => ValueKind::Array,
            DecodedValue::BuiltinAtom(_) | DecodedValue::InlineAtom(_) | DecodedValue::Atom(_) => {
                ValueKind::Atom
            }
            DecodedValue::Tagged(_) => ValueKind::Tagged,
            DecodedValue::Tuple(_) => ValueKind::Tuple,
            DecodedValue::Func(_) => ValueKind::Func,
            DecodedValue::FuncRef(_) => ValueKind::Func,
            DecodedValue::Dyn(_) => ValueKind::Dyn,
            DecodedValue::Module(_) => ValueKind::Module,
            DecodedValue::TypeSlot(_) => {
                unreachable!("up-links are private VM values")
            }
        }
    }

    pub fn as_atom(self) -> Option<crate::TextRef<'a>> {
        match self.value.value() {
            DecodedValue::BuiltinAtom(atom) => Some(crate::heap::TextRef::borrowed(atom.name())),
            DecodedValue::InlineAtom(text) => Some(crate::heap::TextRef::inline(text)),
            DecodedValue::Atom(id) => self.view.text(id).ok().map(crate::heap::TextRef::borrowed),
            _ => None,
        }
    }

    pub fn as_int(self) -> Option<i64> {
        match self.value.value() {
            DecodedValue::Int(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_float(self) -> Option<f64> {
        match self.value.value() {
            DecodedValue::Float(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_str(self) -> Option<crate::TextRef<'a>> {
        match self.value.value() {
            DecodedValue::InlineString(text) => Some(crate::heap::TextRef::inline(text)),
            DecodedValue::ShortString(id) => {
                self.view.text(id).ok().map(crate::heap::TextRef::borrowed)
            }
            _ => None,
        }
    }

    pub fn as_bytes(self) -> Option<&'a [u8]> {
        let DecodedValue::Bytes(handle) = self.value.value() else {
            return None;
        };
        match self.view.object(handle).ok()? {
            Object::Bytes(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_native_type(self) -> Option<&'a crate::NativeType> {
        let DecodedValue::NativeType(id) = self.value.value() else {
            return None;
        };
        self.view.native_type(id).ok()
    }

    pub(crate) fn declared_type_parts(
        self,
    ) -> Option<(&'a crate::value::DeclaredTypeId, &'a str, ValueRef<'a>)> {
        let handle = match self.value.value() {
            DecodedValue::DeclaredType(handle) | DecodedValue::SymbolicType(handle) => handle,
            _ => return None,
        };
        let (id, name, body) = match self.view.object(handle).ok()? {
            Object::DeclaredType { id, name, body, .. } => (id, name, *body),
            Object::SymbolicType { id, name, body, .. } => (id, name, *body),
            _ => return None,
        };
        Some((
            id,
            name,
            ValueRef {
                value: body,
                view: self.view,
            },
        ))
    }

    pub(crate) fn declared_type_body(self) -> Option<ValueRef<'a>> {
        self.declared_type_parts().map(|(_, _, body)| body)
    }

    pub(crate) fn unwrap_declared(self) -> Option<ValueRef<'a>> {
        let value = self.view.unwrap_declared(self.value).ok()?;
        Some(ValueRef {
            value,
            view: self.view,
        })
    }

    pub(crate) fn declared_value_parts(self) -> Option<(ValueRef<'a>, ValueRef<'a>)> {
        let owner = self.view.type_witness(self.value).ok()??;
        Some((
            ValueRef {
                value: owner,
                view: self.view,
            },
            ValueRef {
                value: self.value.without_type_id(),
                view: self.view,
            },
        ))
    }

    pub fn as_opaque<T: std::any::Any>(self, expected_type: &crate::NativeType) -> Option<&'a T> {
        let DecodedValue::Opaque(handle) = self.value.value() else {
            return None;
        };
        match self.view.object(handle).ok()? {
            Object::Opaque(value) => value.downcast_ref(expected_type),
            _ => None,
        }
    }

    pub(crate) fn opaque_native_type(self) -> Option<&'a crate::NativeType> {
        let DecodedValue::Opaque(handle) = self.value.value() else {
            return None;
        };
        match self.view.object(handle).ok()? {
            Object::Opaque(value) => Some(value.native_type()),
            _ => None,
        }
    }

    pub fn sequence_len(self) -> Option<usize> {
        match self.value.value() {
            DecodedValue::Array(handle) => self.view.sequence(handle, false).ok().map(<[_]>::len),
            DecodedValue::Tuple(handle) => self.view.sequence(handle, true).ok().map(<[_]>::len),
            _ => None,
        }
    }

    pub fn sequence_get(self, index: usize) -> Option<ValueRef<'a>> {
        let values = match self.value.value() {
            DecodedValue::Array(handle) => self.view.sequence(handle, false).ok()?,
            DecodedValue::Tuple(handle) => self.view.sequence(handle, true).ok()?,
            _ => return None,
        };
        values.get(index).copied().map(|value| ValueRef {
            value,
            view: self.view,
        })
    }

    pub fn tagged_parts(self) -> Option<(ValueRef<'a>, ValueRef<'a>)> {
        let DecodedValue::Tagged(handle) = self.value.value() else {
            return None;
        };
        let (tag, payload) = self.view.tagged(handle).ok()?;
        Some((
            ValueRef {
                value: tag,
                view: self.view,
            },
            ValueRef {
                value: payload,
                view: self.view,
            },
        ))
    }

    pub fn dict_fields(self) -> Option<Vec<&'a str>> {
        match self.value.value() {
            DecodedValue::Dict(handle) => self.view.dict_fields(handle).ok(),
            _ => None,
        }
    }

    pub fn dict_get(self, field: &str) -> Option<ValueRef<'a>> {
        let DecodedValue::Dict(handle) = self.value.value() else {
            return None;
        };
        self.view
            .dict_get_text(handle, field)
            .ok()
            .flatten()
            .map(|value| ValueRef {
                value,
                view: self.view,
            })
    }

    pub fn get(self, field: &str) -> Option<ValueRef<'a>> {
        self.dict_get(field)
    }

    pub fn dict_values(self) -> Option<Vec<ValueRef<'a>>> {
        let DecodedValue::Dict(handle) = self.value.value() else {
            return None;
        };
        let (_, values) = self.view.dict_parts(handle).ok()?;
        Some(
            values
                .iter()
                .copied()
                .map(|value| ValueRef {
                    value,
                    view: self.view,
                })
                .collect(),
        )
    }

    pub fn is_declared(self) -> bool {
        self.view
            .type_witness(self.value)
            .is_ok_and(|owner| owner.is_some())
    }

    pub fn declared_body(self) -> Option<ValueRef<'a>> {
        self.declared_type_body()
    }

    pub(crate) fn module_fields(self) -> Option<Vec<&'a str>> {
        let DecodedValue::Module(handle) = self.value.value() else {
            return None;
        };
        self.view.module_fields(handle).ok()
    }

    pub(crate) fn module_get(self, field: &str) -> Option<ValueRef<'a>> {
        let DecodedValue::Module(handle) = self.value.value() else {
            return None;
        };
        self.view
            .module_get_text(handle, field)
            .ok()
            .flatten()
            .map(|value| ValueRef {
                value,
                view: self.view,
            })
    }

    pub fn function_arity(self) -> Option<usize> {
        self.view.resolved_function_arity(self.value).ok().flatten()
    }
}

impl fmt::Display for ValueRef<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match DebugValueFormatter::new(self.view).format(self.value) {
            Ok(value) => formatter.write_str(&value),
            Err(error) => write!(formatter, "<invalid value: {error}>"),
        }
    }
}

pub struct CallContext<'vm, 'stack> {
    current: &'vm mut Heap,
    background: Option<&'vm Heap>,
    stack: &'stack mut Vec<Option<Val>>,
    account: &'stack mut QuotaAccount,
    base: usize,
    argument_count: usize,
    upvalue_base: usize,
    upvalue_count: usize,
    result: RegisterId,
    call_site: Option<crate::Loc>,
}

impl<'vm, 'stack> CallContext<'vm, 'stack> {
    fn new(
        current: &'vm mut Heap,
        background: Option<&'vm Heap>,
        stack: &'stack mut Vec<Option<Val>>,
        account: &'stack mut QuotaAccount,
        arguments: Vec<Val>,
        upvalues: &[Val],
        call_site: Option<crate::Loc>,
    ) -> Result<Self, NativeError> {
        let base = stack.len();
        let argument_count = arguments.len();
        let window_size = argument_count
            .checked_add(upvalues.len())
            .and_then(|size| size.checked_add(1))
            .ok_or_else(|| NativeError::stack_limit("native stack window is too large"))?;
        let end = base
            .checked_add(window_size)
            .ok_or_else(|| NativeError::stack_limit("Telora stack size overflowed"))?;
        if end > account.stack_limit() || window_size > u32::MAX as usize {
            return Err(NativeError::stack_limit(
                "native call exceeds the Telora stack-slot limit",
            ));
        }
        stack.extend(arguments.into_iter().map(Some));
        let upvalue_base = argument_count;
        stack.extend(upvalues.iter().cloned().map(Some));
        let upvalue_count = upvalues.len();
        let result_index = argument_count + upvalue_count;
        stack.push(None);
        Ok(Self {
            current,
            background,
            stack,
            account,
            base,
            argument_count,
            upvalue_base,
            upvalue_count,
            result: RegisterId(
                u32::try_from(result_index)
                    .map_err(|_| NativeError::stack_limit("native register count exceeds u32"))?,
            ),
            call_site,
        })
    }

    pub fn argument(&self, index: usize) -> Result<RegisterId, NativeError> {
        if index >= self.argument_count {
            return Err(NativeError::new(format!(
                "argument {index} is out of bounds"
            )));
        }
        Ok(RegisterId(u32::try_from(index).map_err(|_| {
            NativeError::stack_limit("argument register exceeds u32")
        })?))
    }

    pub const fn argument_count(&self) -> usize {
        self.argument_count
    }

    pub const fn result(&self) -> RegisterId {
        self.result
    }

    pub fn upvalue(&self, index: usize) -> Result<RegisterId, NativeError> {
        if index >= self.upvalue_count {
            return Err(NativeError::new(format!(
                "upvalue {index} is out of bounds"
            )));
        }
        Ok(RegisterId(
            u32::try_from(self.upvalue_base + index)
                .map_err(|_| NativeError::stack_limit("upvalue register exceeds u32"))?,
        ))
    }

    pub fn value(&self, register: RegisterId) -> Result<ValueRef<'_>, NativeError> {
        let index = usize::try_from(register.0)
            .map_err(|_| NativeError::new("register does not fit this platform"))?;
        self.stack
            .get(self.base + index)
            .and_then(Option::as_ref)
            .copied()
            .map(|value| ValueRef {
                value,
                view: HeapView {
                    current: self.current,
                    background: self.background,
                },
            })
            .ok_or_else(|| NativeError::new(format!("register {} is not initialized", register.0)))
    }

    pub fn scratch(&mut self) -> Result<RegisterId, NativeError> {
        if self.stack.len() >= self.account.stack_limit() {
            return Err(NativeError::stack_limit(
                "native scratch register exceeds the Telora stack-slot limit",
            ));
        }
        let register = RegisterId(
            u32::try_from(self.stack.len() - self.base)
                .map_err(|_| NativeError::stack_limit("native scratch register exceeds u32"))?,
        );
        self.stack.push(None);
        Ok(register)
    }

    pub fn set_atom(&mut self, destination: RegisterId, name: &str) -> Result<(), NativeError> {
        let value = self.current.atom(self.background, name);
        self.set(destination, value.into())
    }

    pub fn set_int(&mut self, destination: RegisterId, value: i64) -> Result<(), NativeError> {
        self.set(destination, DecodedValue::Int(value).into())
    }

    pub fn set_float(&mut self, destination: RegisterId, value: f64) -> Result<(), NativeError> {
        if !value.is_finite() {
            let value_count = self
                .argument_count
                .checked_add(3)
                .ok_or_else(|| NativeError::allocation_limit("allocation item count overflowed"))?;
            let bytes = logical_value_bytes(value_count)?
                .checked_add(15) // "data", "message", and "rule"
                .ok_or_else(|| NativeError::allocation_limit("allocation size overflowed"))?;
            self.account
                .charge_allocation(bytes)
                .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))?;
            return Err(NativeError::non_finite_float());
        }
        self.set(destination, DecodedValue::Float(value).into())
    }

    pub fn set_none(&mut self, destination: RegisterId) -> Result<(), NativeError> {
        self.set(
            destination,
            DecodedValue::BuiltinAtom(BuiltinAtom::None).into(),
        )
    }

    pub fn set_string(
        &mut self,
        destination: RegisterId,
        value: impl Into<String>,
    ) -> Result<(), NativeError> {
        let value = value.into();
        self.charge_allocation(value.len())?;
        let value = self.current.string(self.background, &value).into();
        self.set(destination, value)
    }

    pub fn set_bytes(
        &mut self,
        destination: RegisterId,
        value: impl Into<Box<[u8]>>,
    ) -> Result<(), NativeError> {
        let value = value.into();
        self.charge_allocation(value.len())?;
        let handle = self.current.allocate(Object::Bytes(value));
        self.set(destination, DecodedValue::Bytes(handle).into())
    }

    pub fn set_opaque<T>(
        &mut self,
        destination: RegisterId,
        native_type: crate::NativeType,
        payload: T,
    ) -> Result<(), NativeError>
    where
        T: std::any::Any + Eq + Send + Sync,
    {
        self.charge_sequence(1)?;
        let value = crate::OpaqueValue::new(native_type, payload);
        let handle = self.current.allocate(Object::Opaque(value));
        self.set(destination, DecodedValue::Opaque(handle).into())
    }

    pub fn set_identity_opaque<T>(
        &mut self,
        destination: RegisterId,
        native_type: crate::NativeType,
        payload: T,
    ) -> Result<(), NativeError>
    where
        T: std::any::Any + Send + Sync,
    {
        self.charge_sequence(1)?;
        let value = crate::OpaqueValue::new_identity(native_type, payload);
        let handle = self.current.allocate(Object::Opaque(value));
        self.set(destination, DecodedValue::Opaque(handle).into())
    }

    pub(crate) fn mark_at_call_site(&mut self, register: RegisterId) -> Result<(), NativeError> {
        let value = self.owned(register)?.with_loc(self.call_site);
        self.set(register, value)
    }

    pub(crate) fn instantiate_type_family(
        &mut self,
        destination: RegisterId,
        template: RegisterId,
        arguments: &[RegisterId],
        argument_descriptors: &[crate::types::TypeDescriptor],
    ) -> Result<(), NativeError> {
        let template = self.owned(template)?;
        let arguments = arguments
            .iter()
            .map(|argument| self.owned(*argument))
            .collect::<Result<Vec<_>, _>>()?;
        let (value, allocations) = crate::heap::instantiate_type_family(
            self.current,
            self.background,
            template,
            &arguments,
            argument_descriptors,
        )
        .map_err(|error| NativeError::new(error.to_string()))?;
        self.charge_sequence(allocations)?;
        self.set(destination, value)
    }

    pub(crate) fn make_declared_type_application(
        &mut self,
        destination: RegisterId,
        id: crate::value::DeclaredTypeId,
        name: impl Into<Arc<str>>,
        body: RegisterId,
        arguments: &[RegisterId],
    ) -> Result<(), NativeError> {
        let body = self.owned(body)?;
        let arguments = arguments
            .iter()
            .map(|argument| self.owned(*argument))
            .collect::<Result<Box<[_]>, _>>()?;
        self.charge_sequence(arguments.len().saturating_add(1))?;
        let name = name.into();
        let value = if id
            .arguments()
            .iter()
            .any(crate::types::type_identity_is_symbolic)
        {
            let handle = self.current.allocate(Object::SymbolicType {
                id,
                name,
                body,
                sealed: true,
                application_arguments: Some(arguments),
            });
            DecodedValue::SymbolicType(handle)
        } else {
            let type_id = self
                .current
                .canonical_declared_type_id(&id)
                .map_err(|error| NativeError::new(error.to_string()))?;
            let handle = self.current.allocate_declared_type(Object::DeclaredType {
                type_id,
                id,
                name,
                body,
                sealed: true,
                application_arguments: Some(arguments),
            });
            DecodedValue::DeclaredType(handle)
        };
        self.set(destination, Val::unknown(value))
    }

    pub(crate) fn make_declared_value(
        &mut self,
        destination: RegisterId,
        owner: RegisterId,
        payload: RegisterId,
    ) -> Result<(), NativeError> {
        let owner = self.owned(owner)?;
        if matches!(owner.value(), DecodedValue::SymbolicType(_)) {
            return Err(NativeError::new(
                "symbolic type metadata cannot own a runtime value",
            ));
        }
        if !matches!(owner.value(), DecodedValue::DeclaredType(_)) {
            return Err(NativeError::new(
                "declared value owner is not a declared Type",
            ));
        }
        let payload = self.owned(payload)?;
        let type_id = HeapView {
            current: self.current,
            background: self.background,
        }
        .declared_type_id(owner)
        .map_err(|error| NativeError::new(error.to_string()))?;
        self.set(destination, payload.with_type_id(type_id))
    }

    pub fn copy(&mut self, destination: RegisterId, source: RegisterId) -> Result<(), NativeError> {
        let value = self.owned(source)?;
        self.set(destination, value)
    }

    pub fn copy_field(
        &mut self,
        destination: RegisterId,
        source: RegisterId,
        field: &str,
    ) -> Result<(), NativeError> {
        let value = self.owned(source)?;
        let DecodedValue::Dict(handle) = value.value() else {
            return Err(NativeError::new("native field source must be a Dict"));
        };
        let value = HeapView {
            current: self.current,
            background: self.background,
        }
        .dict_get_text(handle, field)
        .map_err(|error| NativeError::new(error.to_string()))?
        .ok_or_else(|| NativeError::new(format!("native field source has no field {field:?}")))?;
        self.set(destination, value)
    }

    pub fn copy_sequence_item(
        &mut self,
        destination: RegisterId,
        source: RegisterId,
        index: usize,
    ) -> Result<(), NativeError> {
        let value = self.owned(source)?;
        let DecodedValue::Array(handle) = value.value() else {
            return Err(NativeError::new("native sequence source must be an Array"));
        };
        let value = HeapView {
            current: self.current,
            background: self.background,
        }
        .sequence(handle, false)
        .map_err(|error| NativeError::new(error.to_string()))?
        .get(index)
        .copied()
        .ok_or_else(|| NativeError::new(format!("native sequence has no item {index}")))?;
        self.set(destination, value)
    }

    pub fn copy_tagged_payload(
        &mut self,
        destination: RegisterId,
        source: RegisterId,
    ) -> Result<(), NativeError> {
        let value = self.owned(source)?;
        let DecodedValue::Tagged(handle) = value.value() else {
            return Err(NativeError::new("native tagged source must have a payload"));
        };
        let (_, payload) = HeapView {
            current: self.current,
            background: self.background,
        }
        .tagged(handle)
        .map_err(|error| NativeError::new(error.to_string()))?;
        self.set(destination, payload)
    }

    pub fn set_semantic_value(
        &mut self,
        destination: RegisterId,
        source: &DataWorld,
        owner: RegisterId,
        allocation_hint: usize,
    ) -> Result<(), NativeError> {
        let background = self
            .background
            .ok_or_else(|| NativeError::new("semantic Value requires a Main world"))?;
        let owner = self.owned(owner)?;
        self.charge_allocation(allocation_hint)?;
        let raw = source
            .relocate_into(self.current, background)
            .map_err(|error| NativeError::new(error.to_string()))?;
        let wrapper_bytes = semantic_value_wrapper_bytes(self.current, Some(background), raw)
            .map_err(|error| NativeError::new(error.to_string()))?;
        self.account
            .charge_allocation(wrapper_bytes)
            .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))?;
        let value = wrap_semantic_value(self.current, Some(background), raw, owner)
            .map_err(|error| NativeError::new(error.to_string()))?;
        self.set(destination, value)
    }

    pub fn make_array(
        &mut self,
        destination: RegisterId,
        items: &[RegisterId],
    ) -> Result<(), NativeError> {
        let values = items
            .iter()
            .map(|item| self.owned(*item))
            .collect::<Result<Vec<_>, _>>()?;
        self.charge_sequence(values.len())?;
        let value = Val::unknown(DecodedValue::Array(
            self.current.allocate(Object::Array(values.into())),
        ));
        self.set(destination, value)
    }

    pub fn make_tuple(
        &mut self,
        destination: RegisterId,
        items: &[RegisterId],
    ) -> Result<(), NativeError> {
        let values = items
            .iter()
            .map(|item| self.owned(*item))
            .collect::<Result<Vec<_>, _>>()?;
        self.charge_sequence(values.len())?;
        let value = Val::unknown(DecodedValue::Tuple(
            self.current.allocate(Object::Tuple(values.into())),
        ));
        self.set(destination, value)
    }

    pub fn make_tagged(
        &mut self,
        destination: RegisterId,
        tag: RegisterId,
        payload: RegisterId,
    ) -> Result<(), NativeError> {
        let tag = self.owned(tag)?;
        let payload = self.owned(payload)?;
        self.charge_sequence(2)?;
        let value = Val::unknown(DecodedValue::Tagged(
            self.current.allocate(Object::Tagged { tag, payload }),
        ));
        self.set(destination, value)
    }

    pub fn make_dict(
        &mut self,
        destination: RegisterId,
        fields: &[(String, RegisterId)],
    ) -> Result<(), NativeError> {
        let mut entries = fields
            .iter()
            .map(|(name, register)| Ok((name.as_str(), self.owned(*register)?)))
            .collect::<Result<Vec<_>, NativeError>>()?;
        self.charge_dict(&entries)?;
        entries.sort_by(|left, right| left.0.cmp(right.0));
        if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(NativeError::new("Dict contains a duplicate field"));
        }
        let (fields, values): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .map(|(field, value)| (self.current.intern(field), value))
            .unzip();
        let shape = self.current.intern_shape(fields);
        let value = Val::unknown(DecodedValue::Dict(self.current.allocate(Object::Dict {
            shape,
            values: values.into(),
        })));
        self.set(destination, value)
    }

    fn charge_sequence(&mut self, count: usize) -> Result<(), NativeError> {
        let bytes = logical_value_bytes(count)?;
        self.account
            .charge_allocation(bytes)
            .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))
    }

    fn charge_dict(&mut self, entries: &[(&str, Val)]) -> Result<(), NativeError> {
        let field_bytes = entries.iter().try_fold(0u64, |total, (field, _)| {
            total.checked_add(field.len() as u64).ok_or_else(|| {
                NativeError::allocation_limit("native Dict allocation size overflowed")
            })
        })?;
        let value_bytes = logical_value_bytes(entries.len())?;
        let bytes = field_bytes.checked_add(value_bytes).ok_or_else(|| {
            NativeError::allocation_limit("native Dict allocation size overflowed")
        })?;
        self.account
            .charge_allocation(bytes)
            .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))
    }

    fn charge_allocation(&mut self, bytes: usize) -> Result<(), NativeError> {
        let bytes = u64::try_from(bytes)
            .map_err(|_| NativeError::allocation_limit("native allocation size overflowed"))?;
        self.account
            .charge_allocation(bytes)
            .map_err(|()| NativeError::allocation_limit("native allocation quota exceeded"))
    }

    fn owned(&self, register: RegisterId) -> Result<Val, NativeError> {
        let index = usize::try_from(register.0)
            .map_err(|_| NativeError::new("register does not fit this platform"))?;
        self.stack
            .get(self.base + index)
            .and_then(Option::as_ref)
            .copied()
            .ok_or_else(|| NativeError::new(format!("register {} is not initialized", register.0)))
    }

    fn set(&mut self, register: RegisterId, value: Val) -> Result<(), NativeError> {
        let index = usize::try_from(register.0)
            .map_err(|_| NativeError::new("register does not fit this platform"))?;
        let slot = self
            .stack
            .get_mut(self.base + index)
            .ok_or_else(|| NativeError::new(format!("register {} is out of bounds", register.0)))?;
        *slot = Some(value);
        Ok(())
    }

    fn take_result(self) -> Result<Val, NativeError> {
        let index = usize::try_from(self.result.0)
            .map_err(|_| NativeError::stack_limit("result register does not fit usize"))?;
        let slot = self
            .base
            .checked_add(index)
            .and_then(|slot| self.stack.get_mut(slot))
            .ok_or_else(|| NativeError::new("native result register is out of bounds"))?;
        let result = slot
            .take()
            .ok_or_else(|| NativeError::new("native function did not write its result register"));
        self.stack.truncate(self.base);
        result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeErrorKind {
    Cancelled,
    FuelExhausted,
    AllocationQuotaExceeded,
    CallDepthExceeded,
    DivisionByZero,
    IntegerOverflow,
    InvalidBytecode,
    MissingField,
    NoPatternMatched,
    Panic,
    ReportedDiagnostic,
    RaisedBlame,
    StackLimitExceeded,
    TypeMismatch,
    UninitializedDefinition,
    DuplicateDefinition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
    pub function: String,
    pub instruction: usize,
    pub trace: Vec<RuntimeFrame>,
    locations: Option<Box<RuntimeLocations>>,
    rendered: Option<Box<str>>,
    trace_includes_active_frame: bool,
    propagated_failure: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RuntimeLocations {
    data: Option<crate::Loc>,
    rule: Option<crate::Loc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeFrame {
    pub function: String,
    pub instruction: usize,
    pub origin: Option<Origin>,
}

impl RuntimeError {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn failure_class(&self) -> crate::evaluation::FailureClass {
        use crate::evaluation::FailureClass;
        match self.kind {
            RuntimeErrorKind::DivisionByZero
            | RuntimeErrorKind::IntegerOverflow
            | RuntimeErrorKind::MissingField
            | RuntimeErrorKind::NoPatternMatched
            | RuntimeErrorKind::Panic
            | RuntimeErrorKind::ReportedDiagnostic
            | RuntimeErrorKind::RaisedBlame
            | RuntimeErrorKind::TypeMismatch
            | RuntimeErrorKind::UninitializedDefinition
            | RuntimeErrorKind::DuplicateDefinition => FailureClass::Recoverable,
            RuntimeErrorKind::Cancelled
            | RuntimeErrorKind::FuelExhausted
            | RuntimeErrorKind::AllocationQuotaExceeded
            | RuntimeErrorKind::CallDepthExceeded
            | RuntimeErrorKind::InvalidBytecode
            | RuntimeErrorKind::StackLimitExceeded => FailureClass::Terminal,
        }
    }

    pub(crate) fn from_heap_error(
        function: &BytecodeFunction,
        heap_error: crate::heap::HeapError,
    ) -> Self {
        error(
            RuntimeErrorKind::InvalidBytecode,
            heap_error.to_string(),
            function,
            0,
        )
    }

    pub fn origin(&self) -> Option<Origin> {
        self.trace.first().and_then(|frame| frame.origin)
    }

    pub fn data_location(&self) -> Option<crate::Loc> {
        self.locations
            .as_deref()
            .and_then(|locations| locations.data)
    }

    pub fn rule_location(&self) -> Option<crate::Loc> {
        self.locations
            .as_deref()
            .and_then(|locations| locations.rule)
    }

    pub(crate) const fn propagated_failure(&self) -> Option<u32> {
        self.propagated_failure
    }

    fn set_locations(&mut self, data: Option<crate::Loc>, rule: Option<crate::Loc>) {
        self.locations =
            (data.is_some() || rule.is_some()).then(|| Box::new(RuntimeLocations { data, rule }));
    }

    fn set_data_location(&mut self, data: Option<crate::Loc>) {
        self.set_locations(data, self.rule_location());
    }

    pub(crate) fn diagnostic(&self) -> Option<Diagnostic> {
        if self.propagated_failure.is_some() {
            return None;
        }
        let operation_location = self.origin().and_then(|origin| match origin {
            Origin::Source(location) => Some(location),
            Origin::Synthetic { derived_from } => derived_from,
        });
        let rule_location = self.rule_location().or(operation_location);
        let secondary_message = if self.rule_location().is_some() {
            "contract rule declared here"
        } else {
            "operation originated here"
        };
        match (self.data_location(), rule_location) {
            (Some(data), Some(rule)) if data != rule => Some(
                Diagnostic::error(self.message.clone(), data)
                    .with_secondary(secondary_message, rule),
            ),
            (Some(data), _) => Some(Diagnostic::error(self.message.clone(), data)),
            (None, Some(rule)) => Some(Diagnostic::error(self.message.clone(), rule)),
            (None, None) => None,
        }
    }

    pub fn with_sources(mut self, sources: &SourceDatabase) -> Self {
        if let Some(diagnostic) = self.diagnostic() {
            self.rendered = Some(sources.render(&diagnostic).into_boxed_str());
        }
        self
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(rendered) = &self.rendered {
            return formatter.write_str(rendered);
        }
        write!(
            formatter,
            "{} at {}:{}",
            self.message, self.function, self.instruction
        )
    }
}

impl std::error::Error for RuntimeError {}

fn fail_on_reported_error(
    account: &QuotaAccount,
    start: usize,
    function: &BytecodeFunction,
) -> Result<(), RuntimeError> {
    let Some(diagnostic) = account.diagnostics[start..]
        .iter()
        .find(|diagnostic| diagnostic.severity == crate::source::Severity::Error)
    else {
        return Ok(());
    };
    let mut runtime = error(
        RuntimeErrorKind::ReportedDiagnostic,
        diagnostic.message.clone(),
        function,
        0,
    );
    let primary = diagnostic
        .labels
        .iter()
        .find(|label| label.primary)
        .map(|label| label.location);
    let rule = diagnostic
        .labels
        .iter()
        .rev()
        .find(|label| !label.primary)
        .map(|label| label.location);
    runtime.set_locations(primary, rule);
    Err(runtime)
}

pub struct Vm {
    debug_sink: Arc<dyn DebugSink>,
}

impl Default for Vm {
    fn default() -> Self {
        Self {
            debug_sink: Arc::new(DiscardDebugSink),
        }
    }
}

struct ExecutionFrame {
    function: Arc<BytecodeFunction>,
    prototype: Handle,
    base: usize,
    pc: usize,
    return_target: ReturnTarget,
}

#[derive(Debug)]
enum ReturnTarget {
    Root,
    Register {
        destination: Register,
        call_site: Option<crate::Loc>,
    },
    Native(Box<dyn NativeContinuation>),
}

trait NativeContinuation: fmt::Debug {
    fn return_target(&self) -> &ReturnTarget;
    fn trace_frame(&self) -> &RuntimeFrame;

    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError>;

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError>;
}

#[derive(Debug)]
struct ArrayContinuation {
    function: CoreArrayFunction,
    source: Val,
    callback: Val,
    next_index: usize,
    accumulator: Option<Val>,
    output: Vec<Val>,
    failed: Option<Val>,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    trace_frame: RuntimeFrame,
}

#[derive(Debug)]
struct DictContinuation {
    function: CoreDictFunction,
    entries: Vec<(String, Val)>,
    callback: Val,
    next_index: usize,
    accumulator: Option<Val>,
    output: Vec<(String, Val)>,
    failed: Option<Val>,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    trace_frame: RuntimeFrame,
}

enum VmAction {
    Call {
        callee: Val,
        arguments: Vec<Val>,
        return_target: ReturnTarget,
        call_function: Arc<BytecodeFunction>,
        call_pc: usize,
    },
    Return {
        value: Val,
        return_target: ReturnTarget,
    },
}

enum DriveOutcome {
    Pending,
    Root(Val),
}

pub(crate) struct WorkWorld {
    heap: Heap,
    root: Val,
}

pub(crate) struct VmExecution {
    pub(crate) world: WorkWorld,
    pub(crate) failures: Vec<RuntimeError>,
}

pub(crate) struct VmExecutionFailure {
    heap: Heap,
    pub(crate) error: RuntimeError,
    pub(crate) failures: Vec<RuntimeError>,
}

#[derive(Clone, Copy)]
struct WorkView<'a> {
    main: &'a Heap,
    work: &'a Heap,
}

impl<'a> WorkView<'a> {
    fn heap_view(self) -> HeapView<'a> {
        HeapView {
            current: self.work,
            background: Some(self.main),
        }
    }
}

impl WorkWorld {
    pub(crate) fn root_ref<'a>(&'a self, world: &'a Heap) -> ValueRef<'a> {
        self.value_ref(world, self.root)
    }

    pub(crate) fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    pub(crate) fn heap(&self) -> &Heap {
        &self.heap
    }

    pub(crate) fn value_ref<'a>(&'a self, world: &'a Heap, value: Val) -> ValueRef<'a> {
        ValueRef::work(value, &self.heap, world)
    }

    pub(crate) fn import_world_root(
        mut self,
        background: &Heap,
        source: &WorkWorld,
    ) -> Result<(Self, Val), crate::heap::HeapError> {
        let roots = relocate_work_roots(&mut self.heap, background, &source.heap, &[source.root])?;
        Ok((self, roots[0]))
    }

    pub(crate) fn wrap_root_dyn(
        mut self,
        background: &Heap,
        type_descriptor: &crate::types::TypeDescriptor,
        origin: impl Into<Arc<str>>,
    ) -> Result<Self, crate::heap::HeapError> {
        let descriptor = self
            .heap
            .type_descriptor_value(Some(background), type_descriptor)?;
        self.root = self
            .root
            .with_value(DecodedValue::Dyn(self.heap.allocate(Object::Dyn {
                identity: Arc::new(()),
                descriptor,
                value: self.root,
                scheme: Some(crate::TypeScheme {
                    parameters: Vec::new(),
                    body: type_descriptor.clone(),
                }),
                origin: Some(origin.into()),
            })));
        Ok(self)
    }

    fn module_member(
        &self,
        world: &Heap,
        name: &str,
    ) -> Result<Option<Val>, crate::heap::HeapError> {
        let view = WorkView {
            main: world,
            work: &self.heap,
        }
        .heap_view();
        let DecodedValue::Module(handle) = self.root.value() else {
            return Err(crate::heap::HeapError::new(
                "execution root is not a Module",
            ));
        };
        let Some(field) = self.heap.find_text(name).or_else(|| world.find_text(name)) else {
            return Ok(None);
        };
        view.exports_get(handle, field)
    }

    pub(crate) fn module_member_ref<'a>(
        &'a self,
        world: &'a Heap,
        name: &str,
    ) -> Result<Option<ValueRef<'a>>, crate::heap::HeapError> {
        self.module_member(world, name)
            .map(|value| value.map(|value| self.value_ref(world, value)))
    }

    pub(crate) fn member_function_arity(
        &self,
        world: &Heap,
        name: &str,
    ) -> Result<Option<usize>, crate::heap::HeapError> {
        let Some(value) = self.module_member(world, name)? else {
            return Ok(None);
        };
        WorkView {
            main: world,
            work: &self.heap,
        }
        .heap_view()
        .resolved_function_arity(value)
    }

    pub(crate) fn seal_module(mut self) -> Result<Self, crate::heap::HeapError> {
        self.root = self.heap.seal_module(self.root)?;
        Ok(self)
    }

    pub(crate) fn module_fields(
        &self,
        world: &Heap,
    ) -> Result<Vec<String>, crate::heap::HeapError> {
        let view = WorkView {
            main: world,
            work: &self.heap,
        }
        .heap_view();
        let DecodedValue::Module(handle) = self.root.value() else {
            return Err(crate::heap::HeapError::new(
                "execution root is not a Module",
            ));
        };
        view.exports_fields(handle)
            .map(|fields| fields.into_iter().map(str::to_owned).collect())
    }

    pub(crate) fn publish(
        self,
        world: &mut Heap,
    ) -> Result<PersistentValue, crate::heap::HeapError> {
        publish_module_root(world, &self.heap, self.root)
    }

    pub(crate) fn publish_module(
        mut self,
        world: &mut Heap,
    ) -> Result<PersistentValue, crate::heap::HeapError> {
        self.root = self.heap.seal_module(self.root)?;
        publish_module_root(world, &self.heap, self.root)
    }

    pub(crate) fn into_reducer_transition(
        mut self,
        world: &Heap,
    ) -> Result<(Self, Vec<Val>), crate::heap::HeapError> {
        let view = HeapView {
            current: &self.heap,
            background: Some(world),
        };
        let DecodedValue::Tuple(handle) = self.root.value() else {
            return Err(crate::heap::HeapError::new(
                "Entry reducer must return Tuple([State, Array(SystemEffect)])",
            ));
        };
        let values = view.sequence(handle, true)?;
        let [state, effects] = values else {
            return Err(crate::heap::HeapError::new(
                "Entry reducer transition must contain exactly State and effects",
            ));
        };
        let DecodedValue::Array(effects) = effects.value() else {
            return Err(crate::heap::HeapError::new(
                "Entry reducer effects must be an Array",
            ));
        };
        let effects = view.sequence(effects, false)?.to_vec();
        // Audit the complete batch before the Host observes or executes the
        // first effect. A later failed payload must not permit earlier effects
        // to escape and make the transition partially visible.
        for effect in &effects {
            if view.first_data_failure(*effect)?.is_some() {
                return Err(crate::heap::HeapError::new(
                    "failed evaluation node cannot cross the SystemEffect boundary",
                ));
            }
        }
        self.root = *state;
        Ok((self, effects))
    }

    pub(crate) fn into_runtime_pair(
        mut self,
        world: &Heap,
        root_error: &'static str,
        length_error: &'static str,
    ) -> Result<(Self, Val), crate::heap::HeapError> {
        let view = HeapView {
            current: &self.heap,
            background: Some(world),
        };
        let DecodedValue::Tuple(handle) = self.root.value() else {
            return Err(crate::heap::HeapError::new(root_error));
        };
        let values = view.sequence(handle, true)?;
        let [state, value] = values else {
            return Err(crate::heap::HeapError::new(length_error));
        };
        let state = *state;
        let value = *value;
        self.root = state;
        Ok((self, value))
    }

    pub(crate) fn runtime_function_arity(
        &self,
        world: &Heap,
        value: Val,
    ) -> Result<Option<usize>, crate::heap::HeapError> {
        HeapView {
            current: &self.heap,
            background: Some(world),
        }
        .resolved_function_arity(value)
    }
}

impl Vm {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_debug_sink(mut self, sink: Arc<dyn DebugSink>) -> Self {
        self.debug_sink = sink;
        self
    }

    pub fn execute(
        &mut self,
        function: &BytecodeFunction,
        evaluation_fuel: usize,
    ) -> Result<ExecutionWorld, RuntimeError> {
        self.execute_with_args(function, &[], evaluation_fuel)
    }

    pub fn execute_with_args(
        &mut self,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        evaluation_fuel: usize,
    ) -> Result<ExecutionWorld, RuntimeError> {
        self.execute_with_quota_and_args(function, arguments, Quota::with_fuel(evaluation_fuel))
    }

    pub fn execute_with_quota(
        &mut self,
        function: &BytecodeFunction,
        quota: Quota,
    ) -> Result<ExecutionWorld, RuntimeError> {
        self.execute_with_quota_and_args(function, &[], quota)
    }

    pub fn execute_with_quota_and_args(
        &mut self,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        quota: Quota,
    ) -> Result<ExecutionWorld, RuntimeError> {
        let mut account = QuotaAccount::new(quota);
        self.execute_with_account(function, arguments, &mut account)
    }

    pub(crate) fn execute_with_account(
        &mut self,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        account: &mut QuotaAccount,
    ) -> Result<ExecutionWorld, RuntimeError> {
        let diagnostic_start = account.diagnostics.len();
        let background = Arc::new(Heap::main());
        let arena = self.execute_frame(
            &background,
            &HashMap::new(),
            function,
            None,
            None,
            &[],
            arguments,
            &[],
            account,
        )?;
        fail_on_reported_error(account, diagnostic_start, function)?;
        Ok(ExecutionWorld::new(background, arena))
    }

    pub(crate) fn execute_in_work(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        account: &mut QuotaAccount,
    ) -> Result<WorkWorld, RuntimeError> {
        let diagnostic_start = account.diagnostics.len();
        let arena = self.execute_frame(
            background,
            externals,
            function,
            None,
            None,
            &[],
            arguments,
            &[],
            account,
        )?;
        fail_on_reported_error(account, diagnostic_start, function)?;
        Ok(arena)
    }

    pub(crate) fn execute_in_work_best_effort_with_failures(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        account: &mut QuotaAccount,
        inherited_failure_count: usize,
    ) -> Result<VmExecution, VmExecutionFailure> {
        self.execute_frame_with_policy(
            background,
            externals,
            function,
            None,
            None,
            &[],
            arguments,
            &[],
            account,
            true,
            inherited_failure_count,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_in_existing_world_with_runtime_args(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        world: WorkWorld,
        runtime_arguments: &[Val],
        arguments: &[crate::DataWorld],
        account: &mut QuotaAccount,
    ) -> Result<WorkWorld, RuntimeError> {
        let diagnostic_start = account.diagnostics.len();
        let WorkWorld { heap, root } = world;
        let mut existing_arguments = Vec::with_capacity(runtime_arguments.len() + 1);
        existing_arguments.push(root);
        existing_arguments.extend_from_slice(runtime_arguments);
        let arena = self.execute_frame(
            background,
            externals,
            function,
            Some(heap),
            None,
            &existing_arguments,
            arguments,
            &[],
            account,
        )?;
        fail_on_reported_error(account, diagnostic_start, function)?;
        Ok(arena)
    }

    #[allow(clippy::needless_borrow, clippy::too_many_arguments)]
    fn execute_frame(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        initial_work: Option<Heap>,
        work_state: Option<WorkWorld>,
        existing_arguments: &[Val],
        arguments: &[crate::DataWorld],
        captures: &[crate::DataWorld],
        account: &mut QuotaAccount,
    ) -> Result<WorkWorld, RuntimeError> {
        self.execute_frame_with_policy(
            background,
            externals,
            function,
            initial_work,
            work_state,
            existing_arguments,
            arguments,
            captures,
            account,
            false,
            0,
        )
        .map(|execution| execution.world)
        .map_err(|failure| failure.error)
    }

    #[allow(clippy::needless_borrow, clippy::too_many_arguments)]
    fn execute_frame_with_policy(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        initial_work: Option<Heap>,
        work_state: Option<WorkWorld>,
        existing_arguments: &[Val],
        arguments: &[crate::DataWorld],
        captures: &[crate::DataWorld],
        account: &mut QuotaAccount,
        best_effort: bool,
        inherited_failure_count: usize,
    ) -> Result<VmExecution, VmExecutionFailure> {
        // Linking recursively walks the immutable prototype graph. Keep that host
        // recursion off callers' often-small test or embedding threads; VM calls
        // themselves use the explicit frame stack below.
        let mut current = initial_work.unwrap_or_else(|| Heap::work_for(background));
        let linked = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("telora-bytecode-linker".into())
                .stack_size(16 * 1024 * 1024)
                .spawn_scoped(scope, || {
                    current.link_bytecode_resolved(Some(background), function, externals)
                })
                .map_err(|_| crate::heap::HeapError::new("failed to start bytecode linker"))
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    )
                })?
                .join()
                .map_err(|_| crate::heap::HeapError::new("bytecode linker panicked"))
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    )
                })
        });
        let prototype = match linked {
            Ok(prototype) => prototype,
            Err(error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error,
                    failures: Vec::new(),
                });
            }
        };
        let prototype = match prototype {
            Ok(prototype) => prototype,
            Err(heap_error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error: error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    ),
                    failures: Vec::new(),
                });
            }
        };
        let mut runtime_arguments = Vec::with_capacity(
            existing_arguments.len() + arguments.len() + usize::from(work_state.is_some()),
        );
        runtime_arguments.extend_from_slice(existing_arguments);
        if let Some(WorkWorld { heap, root }) = work_state {
            let relocated = match relocate_work_roots(&mut current, background, &heap, &[root]) {
                Ok(relocated) => relocated,
                Err(heap_error) => {
                    return Err(VmExecutionFailure {
                        heap: current,
                        error: error(
                            RuntimeErrorKind::InvalidBytecode,
                            heap_error.to_string(),
                            function,
                            0,
                        ),
                        failures: Vec::new(),
                    });
                }
            };
            runtime_arguments.extend(relocated);
        }
        let imported_arguments = arguments
            .iter()
            .map(|value| value.relocate_into(&mut current, background))
            .collect::<Result<Vec<_>, _>>();
        let imported_arguments = match imported_arguments {
            Ok(arguments) => arguments,
            Err(heap_error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error: error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    ),
                    failures: Vec::new(),
                });
            }
        };
        runtime_arguments.extend(imported_arguments);
        let captures = captures
            .iter()
            .map(|value| value.relocate_into(&mut current, background))
            .collect::<Result<Vec<_>, _>>();
        let captures = match captures {
            Ok(captures) => captures,
            Err(heap_error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error: error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    ),
                    failures: Vec::new(),
                });
            }
        };
        let mut stack: Vec<Option<Val>> = Vec::new();
        let root_frame = make_execution_frame(
            Arc::new(function.clone()),
            prototype,
            &runtime_arguments,
            &captures,
            ReturnTarget::Root,
            &mut stack,
            account.stack_limit(),
        );
        let root_frame = match root_frame {
            Ok(frame) => frame,
            Err(error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error,
                    failures: Vec::new(),
                });
            }
        };
        let mut frames = vec![root_frame];
        let debug_sink = Arc::clone(&self.debug_sink);

        // A failed node may arrive through an imported Main-world Module. Its
        // id is below the stable prefix length owned by that Main world; only
        // newly created roots need to be retained by this execution.
        let mut failures = Vec::new();
        let mut result = (|| -> Result<Val, RuntimeError> {
            loop {
                let attempt = (|| -> Result<Val, RuntimeError> {
                    loop {
                        let function_arc = frames
                            .last()
                            .expect("execution has at least one frame")
                            .function
                            .clone();
                        let function = function_arc.as_ref();
                        let pc = frames.last().expect("execution frame").pc;
                        let instruction = function.instructions().get(pc).ok_or_else(|| {
                            error(
                                RuntimeErrorKind::InvalidBytecode,
                                "instruction pointer is out of bounds",
                                function,
                                pc,
                            )
                        })?;
                        let frame = frames.last().expect("execution frame");
                        let base = frame.base;
                        let end = base + frame.function.register_count();
                        let mut registers = &mut stack[base..end];
                        let view = WorkView {
                            main: background,
                            work: &current,
                        }
                        .heap_view();

                        match instruction {
                            Opcode::LoadConst { dst, value } => {
                                let (_, values, _, _) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let value = values.get(value.0).copied().ok_or_else(|| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        format!("value link {} is out of bounds", value.0),
                                        function,
                                        pc,
                                    )
                                })?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    value.with_loc(
                                        value.loc().or(instruction_location(function, pc)),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::Move { dst, src } => {
                                let value = *read_register(&registers, *src, function, pc)?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::OwnDeclared { dst, owner, value } => {
                                let owner = *read_register(&registers, *owner, function, pc)?;
                                let value = *read_register(&registers, *value, function, pc)?;
                                let type_id =
                                    view.declared_type_id(owner).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    value.with_type_id(type_id),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::AllocFunc { dst, static_id } => {
                                let value = if let Some(id) = static_id {
                                    Val::new(
                                        DecodedValue::FuncRef(*id),
                                        instruction_location(function, pc),
                                    )
                                } else {
                                    charge_allocation(
                                        account,
                                        logical_value_bytes(1).map_err(|native_error| {
                                            allocation_error(native_error.message, function, pc)
                                        })?,
                                        function,
                                        pc,
                                    )?;
                                    Val::new(
                                        DecodedValue::Func(
                                            current.allocate(crate::heap::Object::OpenFunc),
                                        ),
                                        instruction_location(function, pc),
                                    )
                                };
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::SealFunc { target, source } => {
                                let target = *read_register(&registers, *target, function, pc)?;
                                let source = *read_register(&registers, *source, function, pc)?;
                                if view
                                    .resolve_func(source)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .is_none()
                                {
                                    return Err(error(
                                        RuntimeErrorKind::TypeMismatch,
                                        "function definition did not produce a FuncRef",
                                        function,
                                        pc,
                                    ));
                                }
                                match target.value() {
                                    DecodedValue::Func(target) => {
                                        let DecodedValue::Func(source) = source.value() else {
                                            return Err(error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                "dynamic function slot cannot retain a static reference",
                                                function,
                                                pc,
                                            ));
                                        };
                                        current.seal_local_func(target, source).map_err(
                                            |heap_error| {
                                                error(
                                                    RuntimeErrorKind::DuplicateDefinition,
                                                    heap_error.to_string(),
                                                    function,
                                                    pc,
                                                )
                                            },
                                        )?;
                                    }
                                    DecodedValue::FuncRef(id) => current
                                        .seal_static_func(id, source)
                                        .map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::DuplicateDefinition,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?,
                                    _ => {
                                        return Err(error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            "function ref target is not a FuncRef",
                                            function,
                                            pc,
                                        ));
                                    }
                                }
                            }
                            Opcode::AllocTypeSlot { dst } => {
                                charge_allocation(
                                    account,
                                    logical_value_bytes(1).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?,
                                    function,
                                    pc,
                                )?;
                                let link =
                                    Val::new(
                                        DecodedValue::TypeSlot(current.allocate(
                                            crate::heap::Object::TypeSlot { value: None },
                                        )),
                                        instruction_location(function, pc),
                                    );
                                write_register(&mut registers, *dst, link, function, pc)?;
                            }
                            Opcode::ReadTypeSlot { dst, link } => {
                                let DecodedValue::TypeSlot(handle) =
                                    read_register(&registers, *link, function, pc)?.value()
                                else {
                                    return Err(error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        "up-link read operand is not an up-link",
                                        function,
                                        pc,
                                    ));
                                };
                                let value = view
                                    .type_slot(handle)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .ok_or_else(|| {
                                        error(
                                            RuntimeErrorKind::UninitializedDefinition,
                                            "definition was read before initialization",
                                            function,
                                            pc,
                                        )
                                    })?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::SealTypeSlot { link, src } => {
                                let DecodedValue::TypeSlot(handle) =
                                    read_register(&registers, *link, function, pc)?.value()
                                else {
                                    return Err(error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        "up-link initialization operand is not an up-link",
                                        function,
                                        pc,
                                    ));
                                };
                                if view
                                    .type_slot(handle)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .is_some()
                                {
                                    return Err(error(
                                        RuntimeErrorKind::DuplicateDefinition,
                                        "definition was initialized more than once",
                                        function,
                                        pc,
                                    ));
                                }
                                let value = *read_register(&registers, *src, function, pc)?;
                                current.initialize_type_slot(handle, value).map_err(
                                    |heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    },
                                )?;
                            }
                            Opcode::AssertTypeSlotReady { link } => {
                                let DecodedValue::TypeSlot(handle) =
                                    read_register(&registers, *link, function, pc)?.value()
                                else {
                                    return Err(error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        "up-link assertion operand is not an up-link",
                                        function,
                                        pc,
                                    ));
                                };
                                if view
                                    .type_slot(handle)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .is_none()
                                {
                                    return Err(error(
                                        RuntimeErrorKind::UninitializedDefinition,
                                        "declaration was not initialized before block completion",
                                        function,
                                        pc,
                                    ));
                                }
                            }
                            Opcode::Add { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Add,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    value.with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::Subtract { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Subtract,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    value.with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::Multiply { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Multiply,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::Divide { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Divide,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::Remainder { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Remainder,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::Negate { dst, src } => {
                                let input = *read_register(&registers, *src, function, pc)?;
                                let value = match input.value() {
                                    DecodedValue::Int(value) => {
                                        DecodedValue::Int(value.checked_neg().ok_or_else(|| {
                                            error(
                                                RuntimeErrorKind::IntegerOverflow,
                                                "integer negation overflowed",
                                                function,
                                                pc,
                                            )
                                        })?)
                                    }
                                    DecodedValue::Float(value) => DecodedValue::Float(-value),
                                    _ => {
                                        return Err(runtime_type_error(
                                            "numeric value",
                                            &input,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    }
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(value, instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::Not { dst, src } => {
                                let input = *read_register(&registers, *src, function, pc)?;
                                let value = match input.value() {
                                    DecodedValue::Int(value) => DecodedValue::Int(!value),
                                    DecodedValue::BuiltinAtom(BuiltinAtom::True) => {
                                        DecodedValue::BuiltinAtom(BuiltinAtom::False)
                                    }
                                    DecodedValue::BuiltinAtom(BuiltinAtom::False) => {
                                        DecodedValue::BuiltinAtom(BuiltinAtom::True)
                                    }
                                    _ => {
                                        return Err(runtime_type_error(
                                            "Int or Bool",
                                            &input,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    }
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(value, instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::LogicalNot { dst, src } => {
                                let input = *read_register(&registers, *src, function, pc)?;
                                let value = match input.value() {
                                    DecodedValue::BuiltinAtom(BuiltinAtom::True) => {
                                        DecodedValue::BuiltinAtom(BuiltinAtom::False)
                                    }
                                    DecodedValue::BuiltinAtom(BuiltinAtom::False) => {
                                        DecodedValue::BuiltinAtom(BuiltinAtom::True)
                                    }
                                    _ => {
                                        return Err(runtime_type_error(
                                            "Bool", &input, &view, function, pc,
                                        ));
                                    }
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(value, instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::BitNot { dst, src } => {
                                let input = *read_register(&registers, *src, function, pc)?;
                                let DecodedValue::Int(value) = input.value() else {
                                    return Err(runtime_type_error(
                                        "Int", &input, &view, function, pc,
                                    ));
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(
                                        DecodedValue::Int(!value),
                                        instruction_location(function, pc),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::BitAnd { dst, left, right }
                            | Opcode::BitOr { dst, left, right }
                            | Opcode::BitXor { dst, left, right } => {
                                let operation = match instruction {
                                    Opcode::BitAnd { .. } => BitwiseOperation::And,
                                    Opcode::BitOr { .. } => BitwiseOperation::Or,
                                    Opcode::BitXor { .. } => BitwiseOperation::Xor,
                                    _ => unreachable!(),
                                };
                                let value = bitwise_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    operation,
                                    &view,
                                    function,
                                    pc,
                                )?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::Equal { dst, left, right } => {
                                let left = *read_register(&registers, *left, function, pc)?;
                                let right = *read_register(&registers, *right, function, pc)?;
                                propagate_data_failures(&[left, right], &view, function, pc)?;
                                let equal =
                                    view.values_equal(left, right).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(equal)
                                        .with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::NotEqual { dst, left, right } => {
                                let left = *read_register(&registers, *left, function, pc)?;
                                let right = *read_register(&registers, *right, function, pc)?;
                                propagate_data_failures(&[left, right], &view, function, pc)?;
                                let not_equal =
                                    !view.values_equal(left, right).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(not_equal)
                                        .with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::LessThan { dst, left, right } => {
                                let left = read_register(&registers, *left, function, pc)?;
                                let right = read_register(&registers, *right, function, pc)?;
                                let less =
                                    ordered_comparison(left, right, false, &view, function, pc)?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(less).with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::LessThanOrEqual { dst, left, right } => {
                                let left = read_register(&registers, *left, function, pc)?;
                                let right = read_register(&registers, *right, function, pc)?;
                                let less_or_equal =
                                    ordered_comparison(left, right, true, &view, function, pc)?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(less_or_equal)
                                        .with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::MakeArray { dst, items } => {
                                let values = read_many(&registers, items, function, pc)?;
                                let bytes =
                                    logical_value_bytes(values.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(
                                        DecodedValue::Array(
                                            current.allocate(crate::heap::Object::Array(
                                                values.into(),
                                            )),
                                        ),
                                        instruction_location(function, pc),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::ConcatArrays { dst, arrays } => {
                                let arrays = read_many(&registers, arrays, function, pc)?;
                                let mut values = Vec::new();
                                for array in arrays {
                                    let DecodedValue::Array(handle) = array.value() else {
                                        return Err(runtime_type_error(
                                            "Array spread operand",
                                            &array,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    };
                                    values.extend_from_slice(
                                        view.sequence(handle, false).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?,
                                    );
                                }
                                let bytes =
                                    logical_value_bytes(values.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(
                                        DecodedValue::Array(
                                            current.allocate(crate::heap::Object::Array(
                                                values.into(),
                                            )),
                                        ),
                                        instruction_location(function, pc),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::MakeTuple { dst, items } => {
                                let values = read_many(&registers, items, function, pc)?;
                                let bytes =
                                    logical_value_bytes(values.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(
                                        DecodedValue::Tuple(
                                            current.allocate(crate::heap::Object::Tuple(
                                                values.into(),
                                            )),
                                        ),
                                        instruction_location(function, pc),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::InterpolateString { dst, parts } => {
                                let values = read_many(&registers, parts, function, pc)?
                                    .into_iter()
                                    .map(|value| {
                                        view.unwrap_declared(value).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                let mut length = 0usize;
                                for value in &values {
                                    length += if let DecodedValue::Int(value) = value.value() {
                                        decimal_length(value)
                                    } else if let DecodedValue::Float(value) = value.value() {
                                        value.to_string().len()
                                    } else if let Some(value) =
                                        view.string_text(*value).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?
                                    {
                                        value.len()
                                    } else if let Some(value) =
                                        view.atom_text(*value).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?
                                    {
                                        value.len()
                                    } else {
                                        return Err(runtime_shallow_type_error(
                                            "String, Int, Float, or Atom interpolation value",
                                            *value,
                                            function,
                                            pc,
                                        ));
                                    };
                                }
                                let bytes = u64::try_from(length).map_err(|_| {
                                    allocation_error(
                                        "String allocation size overflowed",
                                        function,
                                        pc,
                                    )
                                })?;
                                charge_allocation(account, bytes, function, pc)?;
                                let mut output = String::with_capacity(length);
                                for value in &values {
                                    if let DecodedValue::Int(value) = value.value() {
                                        write!(output, "{value}")
                                            .expect("writing to String cannot fail");
                                    } else if let DecodedValue::Float(value) = value.value() {
                                        write!(output, "{value}")
                                            .expect("writing to String cannot fail");
                                    } else if let Some(value) =
                                        view.string_text(*value).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?
                                    {
                                        output.push_str(value.as_str());
                                    } else if let Some(value) =
                                        view.atom_text(*value).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?
                                    {
                                        output.push_str(value.as_str());
                                    } else {
                                        unreachable!("interpolation values were validated");
                                    }
                                }
                                let value = Val::new(
                                    current.string(Some(background), &output),
                                    instruction_location(function, pc),
                                );
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::MakeDict { dst, fields } => {
                                let (_, _, text_links, _) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let mut entries = fields
                                    .iter()
                                    .map(|(field, register)| {
                                        let field =
                                            text_links.get(field.0).copied().ok_or_else(|| {
                                                error(
                                                    RuntimeErrorKind::InvalidBytecode,
                                                    format!(
                                                        "text link {} is out of bounds",
                                                        field.0
                                                    ),
                                                    function,
                                                    pc,
                                                )
                                            })?;
                                        Ok((
                                            field,
                                            *read_register(&registers, *register, function, pc)?,
                                        ))
                                    })
                                    .collect::<Result<Vec<_>, RuntimeError>>()?;
                                entries.sort_by(|left, right| {
                                    view.text(left.0)
                                        .unwrap_or("")
                                        .cmp(view.text(right.0).unwrap_or(""))
                                });
                                if entries.windows(2).any(|pair| {
                                    view.text(pair[0].0).ok() == view.text(pair[1].0).ok()
                                }) {
                                    return Err(error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        "Dict contains a duplicate field",
                                        function,
                                        pc,
                                    ));
                                }
                                let (fields, values): (Vec<_>, Vec<_>) =
                                    entries.into_iter().unzip();
                                let field_bytes =
                                    fields.iter().try_fold(0u64, |total, field| {
                                        let length = view
                                            .text(*field)
                                            .map_err(|heap_error| {
                                                error(
                                                    RuntimeErrorKind::InvalidBytecode,
                                                    heap_error.to_string(),
                                                    function,
                                                    pc,
                                                )
                                            })?
                                            .len();
                                        total.checked_add(length as u64).ok_or_else(|| {
                                            allocation_error(
                                                "Dict allocation size overflowed",
                                                function,
                                                pc,
                                            )
                                        })
                                    })?;
                                let value_bytes =
                                    logical_value_bytes(values.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                let bytes =
                                    field_bytes.checked_add(value_bytes).ok_or_else(|| {
                                        allocation_error(
                                            "Dict allocation size overflowed",
                                            function,
                                            pc,
                                        )
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                let shape = current.intern_shape(fields);
                                let dict = Val::new(
                                    DecodedValue::Dict(current.allocate(
                                        crate::heap::Object::Dict {
                                            shape,
                                            values: values.into(),
                                        },
                                    )),
                                    instruction_location(function, pc),
                                );
                                write_register(&mut registers, *dst, dict, function, pc)?;
                            }
                            Opcode::MergeDicts { dst, dicts } => {
                                let dicts = read_many(&registers, dicts, function, pc)?;
                                let mut merged = BTreeMap::new();
                                for dict in dicts {
                                    let DecodedValue::Dict(handle) = dict.value() else {
                                        return Err(runtime_type_error(
                                            "Dict spread operand",
                                            &dict,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    };
                                    let (fields, values) =
                                        view.dict_parts(handle).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?;
                                    for (field, value) in fields.iter().zip(values) {
                                        let field = view.text(*field).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?;
                                        merged.insert(field.to_owned(), *value);
                                    }
                                }
                                let field_bytes =
                                    merged.keys().try_fold(0u64, |total, field| {
                                        total.checked_add(field.len() as u64).ok_or_else(|| {
                                            allocation_error(
                                                "Dict allocation size overflowed",
                                                function,
                                                pc,
                                            )
                                        })
                                    })?;
                                let value_bytes =
                                    logical_value_bytes(merged.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                let bytes =
                                    field_bytes.checked_add(value_bytes).ok_or_else(|| {
                                        allocation_error(
                                            "Dict allocation size overflowed",
                                            function,
                                            pc,
                                        )
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                let (fields, values): (Vec<_>, Vec<_>) = merged
                                    .into_iter()
                                    .map(|(field, value)| (current.intern(&field), value))
                                    .unzip();
                                let shape = current.intern_shape(fields);
                                let dict = Val::new(
                                    DecodedValue::Dict(current.allocate(
                                        crate::heap::Object::Dict {
                                            shape,
                                            values: values.into(),
                                        },
                                    )),
                                    instruction_location(function, pc),
                                );
                                write_register(&mut registers, *dst, dict, function, pc)?;
                            }
                            Opcode::GetField { dst, dict, field } => {
                                let (_, _, text_links, _) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let field = text_links.get(field.0).copied().ok_or_else(|| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        format!("text link {} is out of bounds", field.0),
                                        function,
                                        pc,
                                    )
                                })?;
                                let dict = read_register(&registers, *dict, function, pc)?;
                                let dict = view.unwrap_declared(*dict).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let value = match dict.value() {
                                    DecodedValue::Dict(handle) => view.dict_get(handle, field),
                                    DecodedValue::Module(handle) => view.exports_get(handle, field),
                                    _ => {
                                        return Err(runtime_type_error(
                                            "Dict or Module",
                                            &dict,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    }
                                }
                                .map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?
                                .ok_or_else(|| {
                                    error(
                                        RuntimeErrorKind::MissingField,
                                        format!(
                                            "value has no field {:?}",
                                            view.text(field).unwrap_or("<invalid>")
                                        ),
                                        function,
                                        pc,
                                    )
                                })?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::GetArray { dst, array, index } => {
                                let array = *read_register(&registers, *array, function, pc)?;
                                let DecodedValue::Array(handle) = array.value() else {
                                    return Err(runtime_type_error(
                                        "Array", &array, &view, function, pc,
                                    ));
                                };
                                let index = *read_register(&registers, *index, function, pc)?;
                                let DecodedValue::Int(index_value) = index.value() else {
                                    return Err(runtime_type_error(
                                        "Int", &index, &view, function, pc,
                                    ));
                                };
                                let items = view.sequence(handle, false).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let value = usize::try_from(index_value)
                                    .ok()
                                    .and_then(|index| items.get(index).copied());
                                let Some(value) = value else {
                                    return Err(out_of_range_error(account, function, pc));
                                };
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::ProjectTuple { dst, tuple, index } => {
                                let tuple = *read_register(&registers, *tuple, function, pc)?;
                                let DecodedValue::Tuple(handle) = tuple.value() else {
                                    return Err(runtime_type_error(
                                        "Tuple", &tuple, &view, function, pc,
                                    ));
                                };
                                let value = view
                                    .sequence(handle, true)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .get(*index)
                                    .copied();
                                let Some(value) = value else {
                                    return Err(out_of_range_error(account, function, pc));
                                };
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::FieldExists { dst, value, field } => {
                                let (_, _, text_links, _) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let field = text_links.get(field.0).copied().ok_or_else(|| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        format!("text link {} is out of bounds", field.0),
                                        function,
                                        pc,
                                    )
                                })?;
                                let value = read_register(&registers, *value, function, pc)?;
                                propagate_direct_failure(value, function, pc)?;
                                let value = view.unwrap_declared(*value).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let exists = match value.value() {
                                    DecodedValue::Dict(handle) => view
                                        .dict_get(handle, field)
                                        .map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?
                                        .is_some(),
                                    _ => false,
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(exists),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::IsDict { dst, value } => {
                                let value = read_register(&registers, *value, function, pc)?;
                                propagate_direct_failure(value, function, pc)?;
                                let value = view.unwrap_declared(*value).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let matches = matches!(value.value(), DecodedValue::Dict(_));
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(matches),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::TupleLengthEquals { dst, value, length } => {
                                let value = read_register(&registers, *value, function, pc)?;
                                propagate_direct_failure(value, function, pc)?;
                                let value = view.unwrap_declared(*value).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let matches = matches!(
                                    value.value(),
                                    DecodedValue::Tuple(handle) if view.sequence(handle, true).is_ok_and(|items| items.len() == *length)
                                );
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(matches),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::GetTuple { dst, tuple, index } => {
                                let tuple = read_register(&registers, *tuple, function, pc)?;
                                let DecodedValue::Tuple(handle) = tuple.value() else {
                                    return Err(runtime_type_error(
                                        "Tuple", tuple, &view, function, pc,
                                    ));
                                };
                                let value = view
                                    .sequence(handle, true)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .get(*index)
                                    .copied()
                                    .ok_or_else(|| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            format!("tuple index {index} is out of bounds"),
                                            function,
                                            pc,
                                        )
                                    })?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::TaggedTagEquals { dst, value, tag } => {
                                let value = read_register(&registers, *value, function, pc)?;
                                propagate_direct_failure(value, function, pc)?;
                                let value = view.unwrap_declared(*value).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let expected = read_register(&registers, *tag, function, pc)?;
                                let actual = match value.value() {
                                    DecodedValue::Tagged(handle) => {
                                        let (actual, _) =
                                            view.tagged(handle).map_err(|heap_error| {
                                                error(
                                                    RuntimeErrorKind::InvalidBytecode,
                                                    heap_error.to_string(),
                                                    function,
                                                    pc,
                                                )
                                            })?;
                                        Some(actual)
                                    }
                                    DecodedValue::BuiltinAtom(_)
                                    | DecodedValue::InlineAtom(_)
                                    | DecodedValue::Atom(_) => Some(value),
                                    _ => None,
                                };
                                let matches = if let Some(actual) = actual {
                                    view.values_equal(
                                        actual.without_type_id(),
                                        expected.without_type_id(),
                                    )
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                } else {
                                    false
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(matches),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::GetTaggedPayload { dst, value } => {
                                let tagged = read_register(&registers, *value, function, pc)?;
                                let tagged =
                                    view.unwrap_declared(*tagged).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let DecodedValue::Tagged(handle) = tagged.value() else {
                                    return Err(runtime_type_error(
                                        "Tagged", &tagged, &view, function, pc,
                                    ));
                                };
                                let (_, payload) = view.tagged(handle).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                write_register(&mut registers, *dst, payload, function, pc)?;
                            }
                            Opcode::MakeClosure {
                                dst,
                                prototype,
                                captures,
                            } => {
                                let (_, _, _, prototypes) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let closure_prototype =
                                    prototypes.get(prototype.0).copied().ok_or_else(|| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            format!(
                                                "prototype link {} is out of bounds",
                                                prototype.0
                                            ),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let captures = read_many(&registers, captures, function, pc)?;
                                let bytes = logical_value_bytes(captures.len()).map_err(
                                    |native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    },
                                )?;
                                charge_allocation(account, bytes, function, pc)?;
                                let closure = Val::new(
                                    DecodedValue::Func(current.allocate(
                                        crate::heap::Object::Closure {
                                            identity: Arc::new(()),
                                            prototype: closure_prototype,
                                            upvalues: captures.into(),
                                        },
                                    )),
                                    instruction_location(function, pc),
                                );
                                write_register(&mut registers, *dst, closure, function, pc)?;
                            }
                            Opcode::Call {
                                base: call_base,
                                argument_count,
                            } => {
                                let callee = *read_register(&registers, *call_base, function, pc)?;
                                let arguments = read_call_arguments(
                                    &registers,
                                    *call_base,
                                    *argument_count,
                                    function,
                                    pc,
                                )?;
                                frames.last_mut().expect("caller frame").pc += 1;
                                let _ = registers;
                                match drive_vm_action(
                                    VmAction::Call {
                                        callee,
                                        arguments,
                                        return_target: ReturnTarget::Register {
                                            destination: *call_base,
                                            call_site: instruction_location(function, pc),
                                        },
                                        call_function: function_arc,
                                        call_pc: pc,
                                    },
                                    &mut frames,
                                    &mut stack,
                                    &mut current,
                                    background,
                                    account,
                                )? {
                                    DriveOutcome::Pending => continue,
                                    DriveOutcome::Root(value) => return Ok(value),
                                }
                            }
                            Opcode::TailCall {
                                base: call_base,
                                argument_count,
                            } => {
                                let callee = *read_register(&registers, *call_base, function, pc)?;
                                let arguments = read_call_arguments(
                                    &registers,
                                    *call_base,
                                    *argument_count,
                                    function,
                                    pc,
                                )?;
                                let completed = frames.pop().expect("tail caller frame");
                                let _ = registers;
                                stack.truncate(completed.base);
                                match drive_vm_action(
                                    VmAction::Call {
                                        callee,
                                        arguments,
                                        return_target: completed.return_target,
                                        call_function: function_arc,
                                        call_pc: pc,
                                    },
                                    &mut frames,
                                    &mut stack,
                                    &mut current,
                                    background,
                                    account,
                                )? {
                                    DriveOutcome::Pending => continue,
                                    DriveOutcome::Root(value) => return Ok(value),
                                }
                            }
                            Opcode::Jump { target } => {
                                validate_jump(*target, function, pc)?;
                                if *target <= pc {
                                    consume_fuel(account, function, pc)?;
                                }
                                frames.last_mut().expect("execution frame").pc = *target;
                                continue;
                            }
                            Opcode::JumpIfFalse { condition, target } => {
                                let condition =
                                    read_register(&registers, *condition, function, pc)?;
                                match condition.value() {
                                    DecodedValue::BuiltinAtom(BuiltinAtom::True) => {}
                                    DecodedValue::BuiltinAtom(BuiltinAtom::False) => {
                                        validate_jump(*target, function, pc)?;
                                        if *target <= pc {
                                            consume_fuel(account, function, pc)?;
                                        }
                                        frames.last_mut().expect("execution frame").pc = *target;
                                        continue;
                                    }
                                    _ => {
                                        return Err(runtime_type_error(
                                            "'True or 'False",
                                            condition,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    }
                                }
                            }
                            Opcode::Return { src } => {
                                let value = *read_register(&registers, *src, function, pc)?;
                                let completed = frames.pop().expect("execution frame");
                                let _ = registers;
                                stack.truncate(completed.base);
                                match drive_vm_action(
                                    VmAction::Return {
                                        value,
                                        return_target: completed.return_target,
                                    },
                                    &mut frames,
                                    &mut stack,
                                    &mut current,
                                    background,
                                    account,
                                )? {
                                    DriveOutcome::Pending => continue,
                                    DriveOutcome::Root(value) => return Ok(value),
                                }
                            }
                            Opcode::Fail { message } => {
                                return Err(error(
                                    RuntimeErrorKind::NoPatternMatched,
                                    message,
                                    function,
                                    pc,
                                ));
                            }
                            Opcode::Panic { message } => {
                                let message = *read_register(&registers, *message, function, pc)?;
                                let text = view
                                    .string_text(message)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .ok_or_else(|| {
                                        runtime_type_error("String", &message, &view, function, pc)
                                    })?
                                    .as_str()
                                    .to_owned();
                                return Err(error(RuntimeErrorKind::Panic, text, function, pc));
                            }
                            Opcode::Raise {
                                error: error_register,
                            } => {
                                let structured =
                                    *read_register(&registers, *error_register, function, pc)?;
                                let DecodedValue::Dict(handle) = structured.value() else {
                                    return Err(runtime_type_error(
                                        "BlameError",
                                        &structured,
                                        &view,
                                        function,
                                        pc,
                                    ));
                                };
                                let fields = view.dict_fields(handle).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                if fields.as_slice() != ["data", "message", "rule"] {
                                    return Err(runtime_type_error(
                                        "BlameError",
                                        &structured,
                                        &view,
                                        function,
                                        pc,
                                    ));
                                }
                                let get_field = |name| {
                                    view.dict_get_text(handle, name)
                                        .map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?
                                        .ok_or_else(|| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                format!("BlameError is missing {name}"),
                                                function,
                                                pc,
                                            )
                                        })
                                };
                                let data = get_field("data")?;
                                let message = get_field("message")?;
                                let rule = get_field("rule")?;
                                let text = view.string_text(message).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let Some(text) = text else {
                                    return Err(runtime_type_error(
                                        "String", &message, &view, function, pc,
                                    ));
                                };
                                let mut runtime =
                                    error(RuntimeErrorKind::RaisedBlame, text, function, pc);
                                runtime.set_locations(data.loc(), rule.loc());
                                return Err(runtime);
                            }
                            Opcode::Debug {
                                value,
                                module,
                                line,
                                name,
                                message,
                            } => {
                                let value = *read_register(&registers, *value, function, pc)?;
                                if let Ok(value_text) = DebugValueFormatter::new(view).format(value)
                                {
                                    debug_sink.emit(DebugEvent {
                                        name: name.clone(),
                                        repr: value_text,
                                        module: module.clone(),
                                        line: *line,
                                        message: message.clone(),
                                    });
                                }
                            }
                        }
                        frames.last_mut().expect("execution frame").pc += 1;
                    }
                })();
                match attempt {
                    Err(mut runtime_error)
                        if best_effort
                            && runtime_error.failure_class()
                                == crate::evaluation::FailureClass::Recoverable =>
                    {
                        let failure_location = runtime_error.data_location();
                        let failed_instruction = runtime_error.instruction;
                        let frame_index = frames.iter().rposition(|frame| {
                            matches!(
                                frame.return_target,
                                ReturnTarget::Native(_) | ReturnTarget::Register { .. }
                            )
                        });
                        let current_destination = if frame_index.is_none() {
                            frames.last().and_then(|frame| {
                                (frame.function.name() == runtime_error.function)
                                    .then(|| {
                                        frame
                                            .function
                                            .instructions()
                                            .get(runtime_error.instruction)
                                            .and_then(recoverable_instruction_destination)
                                    })
                                    .flatten()
                            })
                        } else {
                            None
                        };
                        if frame_index.is_none() && current_destination.is_none() {
                            break Err(runtime_error);
                        }
                        let failure_id = if let Some(failure_id) = runtime_error.propagated_failure
                        {
                            if failure_id as usize
                                >= inherited_failure_count.saturating_add(failures.len())
                            {
                                break Err(error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "failed evaluation node references an unknown root",
                                    function,
                                    0,
                                ));
                            }
                            failure_id
                        } else {
                            append_runtime_trace(&mut runtime_error, &frames);
                            let failure_id = u32::try_from(
                                inherited_failure_count.saturating_add(failures.len()),
                            )
                            .map_err(|_| {
                                error(
                                    RuntimeErrorKind::AllocationQuotaExceeded,
                                    "best-effort failure arena is full",
                                    function,
                                    0,
                                )
                            })?;
                            failures.push(runtime_error);
                            failure_id
                        };
                        let failure = Val::new(DecodedValue::Failed(failure_id), failure_location);
                        if let Some(frame_index) = frame_index {
                            let stack_base = frames[frame_index].base;
                            let completed =
                                frames.drain(frame_index..).next().expect("failed frame");
                            stack.truncate(stack_base);
                            match completed.return_target {
                                ReturnTarget::Register {
                                    destination,
                                    call_site,
                                } => {
                                    let caller =
                                        frames.last().expect("register return has a caller");
                                    let end = caller.base + caller.function.register_count();
                                    write_register(
                                        &mut stack[caller.base..end],
                                        destination,
                                        failure.rebase_generated(call_site),
                                        &caller.function,
                                        caller.pc.saturating_sub(1),
                                    )?;
                                    continue;
                                }
                                ReturnTarget::Native(continuation) => {
                                    let action = continuation.resume_failed(
                                        failure,
                                        &mut current,
                                        background,
                                        account,
                                    )?;
                                    match drive_vm_action(
                                        action,
                                        &mut frames,
                                        &mut stack,
                                        &mut current,
                                        background,
                                        account,
                                    )? {
                                        DriveOutcome::Pending => continue,
                                        DriveOutcome::Root(root) => break Ok(root),
                                    }
                                }
                                ReturnTarget::Root => unreachable!("root frame is not recoverable"),
                            }
                        } else {
                            let destination = current_destination.expect("checked above");
                            let frame = frames.last_mut().expect("execution frame");
                            let end = frame.base + frame.function.register_count();
                            write_register(
                                &mut stack[frame.base..end],
                                destination,
                                failure,
                                &frame.function,
                                failed_instruction,
                            )?;
                            frame.pc = frame.pc.max(failed_instruction.saturating_add(1));
                            continue;
                        }
                    }
                    outcome => break outcome,
                }
            }
        })();
        if let Err(runtime_error) = &mut result {
            append_runtime_trace(runtime_error, &frames);
        }
        match result {
            Ok(root) => Ok(VmExecution {
                world: WorkWorld {
                    heap: current,
                    root,
                },
                failures,
            }),
            Err(error) => Err(VmExecutionFailure {
                heap: current,
                error,
                failures,
            }),
        }
    }

    pub(crate) fn execute_in_existing_work(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        work: Heap,
        account: &mut QuotaAccount,
    ) -> Result<(Heap, Val), (Heap, RuntimeError)> {
        let diagnostic_start = account.diagnostics.len();
        let execution = self
            .execute_frame_with_policy(
                background,
                externals,
                function,
                Some(work),
                None,
                &[],
                &[],
                &[],
                account,
                false,
                0,
            )
            .map_err(|failure| (failure.heap, failure.error))?;
        let world = execution.world;
        if let Err(error) = fail_on_reported_error(account, diagnostic_start, function) {
            return Err((world.heap, error));
        }
        Ok((world.heap, world.root))
    }
}

fn recoverable_instruction_destination(instruction: &Opcode) -> Option<Register> {
    match instruction {
        Opcode::LoadConst { dst, .. }
        | Opcode::Move { dst, .. }
        | Opcode::OwnDeclared { dst, .. }
        | Opcode::AllocFunc { dst, .. }
        | Opcode::AllocTypeSlot { dst }
        | Opcode::ReadTypeSlot { dst, .. }
        | Opcode::Add { dst, .. }
        | Opcode::Subtract { dst, .. }
        | Opcode::Multiply { dst, .. }
        | Opcode::Divide { dst, .. }
        | Opcode::Remainder { dst, .. }
        | Opcode::Negate { dst, .. }
        | Opcode::Not { dst, .. }
        | Opcode::LogicalNot { dst, .. }
        | Opcode::BitNot { dst, .. }
        | Opcode::BitAnd { dst, .. }
        | Opcode::BitOr { dst, .. }
        | Opcode::BitXor { dst, .. }
        | Opcode::Equal { dst, .. }
        | Opcode::NotEqual { dst, .. }
        | Opcode::LessThan { dst, .. }
        | Opcode::LessThanOrEqual { dst, .. }
        | Opcode::MakeArray { dst, .. }
        | Opcode::ConcatArrays { dst, .. }
        | Opcode::MakeTuple { dst, .. }
        | Opcode::InterpolateString { dst, .. }
        | Opcode::MakeDict { dst, .. }
        | Opcode::MergeDicts { dst, .. }
        | Opcode::GetField { dst, .. }
        | Opcode::GetArray { dst, .. }
        | Opcode::ProjectTuple { dst, .. }
        | Opcode::FieldExists { dst, .. }
        | Opcode::IsDict { dst, .. }
        | Opcode::TupleLengthEquals { dst, .. }
        | Opcode::GetTuple { dst, .. }
        | Opcode::TaggedTagEquals { dst, .. }
        | Opcode::GetTaggedPayload { dst, .. }
        | Opcode::MakeClosure { dst, .. } => Some(*dst),
        Opcode::Call { base, .. } => Some(*base),
        Opcode::Panic { message } => Some(*message),
        Opcode::Raise { error } => Some(*error),
        Opcode::SealFunc { .. }
        | Opcode::SealTypeSlot { .. }
        | Opcode::AssertTypeSlotReady { .. }
        | Opcode::TailCall { .. }
        | Opcode::Jump { .. }
        | Opcode::JumpIfFalse { .. }
        | Opcode::Return { .. }
        | Opcode::Fail { .. }
        | Opcode::Debug { .. } => None,
    }
}

fn append_runtime_trace(runtime_error: &mut RuntimeError, frames: &[ExecutionFrame]) {
    for (index, frame) in frames.iter().rev().enumerate() {
        if index != 0 || !runtime_error.trace_includes_active_frame {
            let instruction = frame.pc.saturating_sub(1);
            runtime_error.trace.push(RuntimeFrame {
                function: frame.function.name().to_owned(),
                instruction,
                origin: frame.function.origin_at(instruction),
            });
        }
        frame
            .return_target
            .append_native_trace(&mut runtime_error.trace);
    }
    runtime_error.trace_includes_active_frame = false;
}

fn make_execution_frame(
    function: Arc<BytecodeFunction>,
    prototype: Handle,
    arguments: &[Val],
    captures: &[Val],
    return_target: ReturnTarget,
    stack: &mut Vec<Option<Val>>,
    stack_limit: usize,
) -> Result<ExecutionFrame, RuntimeError> {
    if arguments.len() != function.parameter_count() {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            format!(
                "expected {} arguments, got {}",
                function.parameter_count(),
                arguments.len()
            ),
            &function,
            0,
        ));
    }
    if captures.len() != function.capture_count() {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "closure capture count does not match function signature",
            &function,
            0,
        ));
    }
    let base = stack.len();
    let end = base.checked_add(function.register_count()).ok_or_else(|| {
        error(
            RuntimeErrorKind::StackLimitExceeded,
            "Telora stack size overflowed",
            &function,
            0,
        )
    })?;
    if end > stack_limit {
        return Err(error(
            RuntimeErrorKind::StackLimitExceeded,
            format!("Telora stack exceeds the limit of {stack_limit} slots"),
            &function,
            0,
        ));
    }
    stack.resize(end, None);
    for (index, value) in arguments.iter().chain(captures).enumerate() {
        let Some(register) = stack.get_mut(base + index) else {
            return Err(error(
                RuntimeErrorKind::InvalidBytecode,
                "function signature exceeds its register count",
                &function,
                0,
            ));
        };
        *register = Some(*value);
    }
    Ok(ExecutionFrame {
        function,
        prototype,
        base,
        pc: 0,
        return_target,
    })
}

fn drive_vm_action(
    mut action: VmAction,
    frames: &mut Vec<ExecutionFrame>,
    stack: &mut Vec<Option<Val>>,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<DriveOutcome, RuntimeError> {
    loop {
        action = match action {
            VmAction::Return {
                value,
                return_target,
            } => match return_target {
                ReturnTarget::Root => return Ok(DriveOutcome::Root(value)),
                ReturnTarget::Register {
                    destination,
                    call_site,
                } => {
                    let caller = frames.last().ok_or_else(|| RuntimeError {
                        kind: RuntimeErrorKind::InvalidBytecode,
                        message: "return register has no caller".into(),
                        function: "<vm>".into(),
                        instruction: 0,
                        trace: Vec::new(),
                        locations: None,
                        rendered: None,
                        trace_includes_active_frame: false,
                        propagated_failure: None,
                    })?;
                    let caller_function = caller.function.clone();
                    let caller_end = caller.base + caller.function.register_count();
                    write_register(
                        &mut stack[caller.base..caller_end],
                        destination,
                        value.rebase_generated(call_site),
                        &caller_function,
                        caller.pc.saturating_sub(1),
                    )?;
                    return Ok(DriveOutcome::Pending);
                }
                ReturnTarget::Native(continuation) => {
                    let trace_frame = continuation.trace_frame().clone();
                    let resumed = if matches!(value.value(), DecodedValue::Failed(_)) {
                        continuation.resume_failed(value, current, background, account)
                    } else {
                        continuation.resume(value, current, background, account)
                    };
                    resumed.map_err(|mut runtime_error| {
                        runtime_error.trace.push(trace_frame);
                        runtime_error
                    })?
                }
            },
            VmAction::Call {
                callee,
                arguments,
                return_target,
                call_function,
                call_pc,
            } => {
                consume_fuel(account, &call_function, call_pc).map_err(|mut runtime_error| {
                    return_target.append_native_trace(&mut runtime_error.trace);
                    runtime_error
                })?;
                let logical_depth = frames.len()
                    + frames
                        .iter()
                        .map(|frame| frame.return_target.native_depth())
                        .sum::<usize>()
                    + return_target.native_depth();
                if logical_depth >= MAX_CALL_DEPTH {
                    return Err(error(
                        RuntimeErrorKind::CallDepthExceeded,
                        format!("call depth exceeds the limit of {MAX_CALL_DEPTH} frames"),
                        &call_function,
                        call_pc,
                    ));
                }
                if matches!(
                    callee.value(),
                    DecodedValue::BuiltinAtom(_)
                        | DecodedValue::InlineAtom(_)
                        | DecodedValue::Atom(_)
                ) {
                    if arguments.len() != 1 {
                        return Err(error(
                            RuntimeErrorKind::TypeMismatch,
                            format!(
                                "tag constructor expects 1 argument, got {}",
                                arguments.len()
                            ),
                            &call_function,
                            call_pc,
                        ));
                    }
                    charge_allocation(
                        account,
                        (std::mem::size_of::<Val>() * 2) as u64,
                        &call_function,
                        call_pc,
                    )?;
                    let value = Val::new(
                        DecodedValue::Tagged(current.allocate(Object::Tagged {
                            tag: callee,
                            payload: arguments[0],
                        })),
                        callee.loc(),
                    );
                    VmAction::Return {
                        value,
                        return_target,
                    }
                } else {
                    let view = HeapView {
                        current,
                        background: Some(background),
                    };
                    let Some(closure_handle) = view.resolve_func(callee).map_err(|heap_error| {
                        error(
                            RuntimeErrorKind::UninitializedDefinition,
                            heap_error.to_string(),
                            &call_function,
                            call_pc,
                        )
                    })?
                    else {
                        return Err(runtime_type_error(
                            "Func",
                            &callee,
                            &view,
                            &call_function,
                            call_pc,
                        ));
                    };
                    if let Some(failure) = arguments.iter().find_map(|argument| {
                        if let DecodedValue::Failed(failure) = argument.value() {
                            Some((failure, argument.loc()))
                        } else {
                            None
                        }
                    }) {
                        return Err(propagated_failure_error(
                            failure.0,
                            failure.1,
                            &call_function,
                            call_pc,
                        ));
                    }
                    let (runtime_prototype, upvalues) =
                        view.closure(closure_handle).map_err(|heap_error| {
                            error(
                                RuntimeErrorKind::InvalidBytecode,
                                heap_error.to_string(),
                                &call_function,
                                call_pc,
                            )
                        })?;
                    let upvalues = upvalues.to_vec();
                    let expected_arity = match runtime_prototype {
                        crate::heap::RuntimePrototype::Bytecode(prototype) => view
                            .bytecode(prototype)
                            .map_err(|heap_error| {
                                error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    heap_error.to_string(),
                                    &call_function,
                                    call_pc,
                                )
                            })?
                            .0
                            .parameter_count(),
                        crate::heap::RuntimePrototype::Native(native) => native.arity(),
                    };
                    if arguments.len() != expected_arity {
                        return Err(error(
                            RuntimeErrorKind::TypeMismatch,
                            format!(
                                "expected {expected_arity} arguments, got {}",
                                arguments.len()
                            ),
                            &call_function,
                            call_pc,
                        ));
                    }
                    match runtime_prototype {
                        crate::heap::RuntimePrototype::Bytecode(prototype) => {
                            let (code, _, _, _) =
                                view.bytecode(prototype).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        &call_function,
                                        call_pc,
                                    )
                                })?;
                            let callee_function =
                                Arc::new(BytecodeFunction::from_linked_code(Arc::clone(code)));
                            let next = make_execution_frame(
                                callee_function,
                                prototype,
                                &arguments,
                                &upvalues,
                                return_target,
                                stack,
                                account.stack_limit(),
                            )
                            .map_err(|runtime_error| {
                                error(
                                    runtime_error.kind,
                                    runtime_error.message,
                                    &call_function,
                                    call_pc,
                                )
                            })?;
                            frames.push(next);
                            return Ok(DriveOutcome::Pending);
                        }
                        crate::heap::RuntimePrototype::Native(native) => match native.kind() {
                            NativeKind::Synchronous => {
                                let mut context = CallContext::new(
                                    current,
                                    Some(background),
                                    stack,
                                    account,
                                    arguments,
                                    &upvalues,
                                    instruction_location(&call_function, call_pc),
                                )
                                .map_err(|native_error| {
                                    native_runtime_error(
                                        native,
                                        native_error,
                                        &call_function,
                                        call_pc,
                                    )
                                })?;
                                (native.callback())(&mut context).map_err(|native_error| {
                                    native_runtime_error(
                                        native,
                                        native_error,
                                        &call_function,
                                        call_pc,
                                    )
                                })?;
                                let value = context.take_result().map_err(|native_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        format!("{}: {}", native.name(), native_error.message),
                                        &call_function,
                                        call_pc,
                                    )
                                })?;
                                VmAction::Return {
                                    value: value.with_loc(
                                        value
                                            .loc()
                                            .or(instruction_location(&call_function, call_pc)),
                                    ),
                                    return_target,
                                }
                            }
                            NativeKind::CoreArray(function) => start_array_continuation(
                                function,
                                arguments,
                                return_target,
                                call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreAttributes(function) => run_core_attributes(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreModel(function) => run_core_model(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreBuiltinType(function) => run_core_builtin_type(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreDict(function) => {
                                if matches!(
                                    function,
                                    CoreDictFunction::MapValues
                                        | CoreDictFunction::Filter
                                        | CoreDictFunction::Fold
                                ) {
                                    start_dict_continuation(
                                        function,
                                        arguments,
                                        return_target,
                                        call_function,
                                        call_pc,
                                        current,
                                        background,
                                        account,
                                    )?
                                } else {
                                    run_core_dict(
                                        function,
                                        &arguments,
                                        return_target,
                                        &call_function,
                                        call_pc,
                                        current,
                                        background,
                                        account,
                                    )?
                                }
                            }
                            NativeKind::CoreString(function) => run_core_string(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CorePath(function) => run_core_path(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreDiagnostic(CoreDiagnosticFunction::Warn) => {
                                run_core_diagnostic(
                                    &arguments,
                                    return_target,
                                    &call_function,
                                    call_pc,
                                    current,
                                    background,
                                    account,
                                )?
                            }
                            NativeKind::CoreHash(function) => run_core_hash(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreCodec(operation) => run_core_codec(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreTypeDesc(operation) => run_core_type_desc(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreDyn(operation) => run_core_dyn(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreEq(operation) => run_core_eq(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                            )?,
                            NativeKind::CoreResult(operation) => run_core_result(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                            )?,
                            NativeKind::CoreJson(operation) => run_core_json(
                                operation,
                                &arguments,
                                &upvalues,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                        },
                    }
                }
            }
        };
    }
}

impl ReturnTarget {
    fn native_depth(&self) -> usize {
        match self {
            Self::Root | Self::Register { .. } => 0,
            Self::Native(continuation) => 1 + continuation.return_target().native_depth(),
        }
    }

    fn append_native_trace(&self, trace: &mut Vec<RuntimeFrame>) {
        if let Self::Native(continuation) = self {
            trace.push(continuation.trace_frame().clone());
            continuation.return_target().append_native_trace(trace);
        }
    }
}

impl NativeContinuation for ArrayContinuation {
    fn return_target(&self) -> &ReturnTarget {
        &self.return_target
    }

    fn trace_frame(&self) -> &RuntimeFrame {
        &self.trace_frame
    }

    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_array_continuation(*self, value, current, background, account)
    }

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_array_failure(*self, failure, current, background, account)
    }
}

impl NativeContinuation for DictContinuation {
    fn return_target(&self) -> &ReturnTarget {
        &self.return_target
    }

    fn trace_frame(&self) -> &RuntimeFrame {
        &self.trace_frame
    }

    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_dict_continuation(*self, value, current, background, account)
    }

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_dict_failure(*self, failure, current, background, account)
    }
}

fn native_runtime_error(
    native: crate::NativeFunction,
    native_error: NativeError,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    if native_error.is_non_finite_float() {
        let location = instruction_location(function, pc);
        let mut runtime = error(
            RuntimeErrorKind::RaisedBlame,
            "NonFiniteFloat",
            function,
            pc,
        );
        runtime.set_locations(location, location);
        return runtime;
    }
    error(
        match native_error.limit() {
            Some(NativeLimit::Stack) => RuntimeErrorKind::StackLimitExceeded,
            Some(NativeLimit::Allocation) => RuntimeErrorKind::AllocationQuotaExceeded,
            None => RuntimeErrorKind::TypeMismatch,
        },
        format!("{}: {}", native.name(), native_error.message),
        function,
        pc,
    )
}

#[allow(clippy::too_many_arguments)]
fn start_array_continuation(
    function: CoreArrayFunction,
    arguments: Vec<Val>,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let source = arguments[0];
    let DecodedValue::Array(source_handle) = source.value() else {
        let view = HeapView {
            current,
            background: Some(background),
        };
        return Err(runtime_type_error(
            "Array",
            &source,
            &view,
            &call_function,
            call_pc,
        ));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let length = view
        .sequence(source_handle, false)
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                &call_function,
                call_pc,
            )
        })?
        .len();
    if function == CoreArrayFunction::Length {
        let length = i64::try_from(length).map_err(|_| {
            error(
                RuntimeErrorKind::IntegerOverflow,
                "Array length does not fit Int",
                &call_function,
                call_pc,
            )
        })?;
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Int(length),
                instruction_location(&call_function, call_pc),
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Get {
        let DecodedValue::Int(index) = arguments[1].value() else {
            return Err(runtime_type_error(
                "Int",
                &arguments[1],
                &view,
                &call_function,
                call_pc,
            ));
        };
        let values = view
            .sequence(source_handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?;
        let value = usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index).copied());
        let Some(payload) = value else {
            return Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::None),
                    instruction_location(&call_function, call_pc),
                ),
                return_target,
            });
        };
        if matches!(payload.value(), DecodedValue::Failed(_)) {
            return Ok(VmAction::Return {
                value: payload,
                return_target,
            });
        }
        charge_allocation(
            account,
            logical_value_bytes(2)
                .map_err(|error| allocation_error(error.message, &call_function, call_pc))?,
            &call_function,
            call_pc,
        )?;
        let location = instruction_location(&call_function, call_pc);
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), location),
                    payload,
                })),
                location,
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Enumerate {
        let values = view
            .sequence(source_handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?
            .to_vec();
        i64::try_from(values.len()).map_err(|_| {
            error(
                RuntimeErrorKind::IntegerOverflow,
                "Array enumeration index does not fit Int",
                &call_function,
                call_pc,
            )
        })?;
        let output_slots = values.len().checked_mul(3).ok_or_else(|| {
            allocation_error("Array enumeration size overflowed", &call_function, call_pc)
        })?;
        charge_allocation(
            account,
            logical_value_bytes(output_slots)
                .map_err(|error| allocation_error(error.message, &call_function, call_pc))?,
            &call_function,
            call_pc,
        )?;
        let location = instruction_location(&call_function, call_pc);
        let output = values
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let index = i64::try_from(index).expect("enumeration length checked above");
                Val::new(
                    DecodedValue::Tuple(current.allocate(Object::Tuple(
                        vec![Val::new(DecodedValue::Int(index), location), value].into(),
                    ))),
                    location,
                )
            })
            .collect();
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Array(current.allocate(Object::Array(output))),
                location,
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Push {
        let source_values = view.sequence(source_handle, false).map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                &call_function,
                call_pc,
            )
        })?;
        let output_len = source_values.len().checked_add(1).ok_or_else(|| {
            allocation_error("Array push length overflowed", &call_function, call_pc)
        })?;
        let bytes = logical_value_bytes(output_len).map_err(|native_error| {
            allocation_error(native_error.message, &call_function, call_pc)
        })?;
        charge_allocation(account, bytes, &call_function, call_pc)?;
        let mut output = Vec::with_capacity(output_len);
        output.extend_from_slice(source_values);
        output.push(arguments[1]);
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Array(current.allocate(Object::Array(output.into()))),
                instruction_location(&call_function, call_pc),
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Concat {
        let arrays = view.sequence(source_handle, false).map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                &call_function,
                call_pc,
            )
        })?;
        let mut output = Vec::new();
        for (index, array) in arrays.iter().copied().enumerate() {
            let DecodedValue::Array(handle) = array.value() else {
                if let DecodedValue::Failed(failure) = array.value() {
                    return Err(propagated_failure_error(
                        failure,
                        array.loc(),
                        &call_function,
                        call_pc,
                    ));
                }
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    format!("std/array.concat item {index} must be an Array"),
                    &call_function,
                    call_pc,
                ));
            };
            output.extend_from_slice(view.sequence(handle, false).map_err(|heap_error| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    heap_error.to_string(),
                    &call_function,
                    call_pc,
                )
            })?);
        }
        let bytes = logical_value_bytes(output.len()).map_err(|native_error| {
            allocation_error(native_error.message, &call_function, call_pc)
        })?;
        charge_allocation(account, bytes, &call_function, call_pc)?;
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Array(current.allocate(Object::Array(output.into()))),
                instruction_location(&call_function, call_pc),
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Zip {
        let DecodedValue::Array(right_handle) = arguments[1].value() else {
            return Err(runtime_type_error(
                "Array",
                &arguments[1],
                &view,
                &call_function,
                call_pc,
            ));
        };
        let left = view
            .sequence(source_handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?;
        let right = view
            .sequence(right_handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?;
        if left.len() != right.len() {
            return Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::None),
                    instruction_location(&call_function, call_pc),
                ),
                return_target,
            });
        }
        let pairs = left
            .iter()
            .copied()
            .zip(right.iter().copied())
            .collect::<Vec<_>>();
        charge_allocation(
            account,
            logical_value_bytes(2 + pairs.len() * 3)
                .map_err(|error| allocation_error(error.message, &call_function, call_pc))?,
            &call_function,
            call_pc,
        )?;
        let pairs = pairs
            .into_iter()
            .map(|(left, right)| {
                Val::new(
                    DecodedValue::Tuple(current.allocate(Object::Tuple(vec![left, right].into()))),
                    left.loc(),
                )
            })
            .collect();
        let pairs = Val::new(
            DecodedValue::Array(current.allocate(Object::Array(pairs))),
            source.loc(),
        );
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), source.loc()),
                    payload: pairs,
                })),
                source.loc(),
            ),
            return_target,
        });
    }

    let controlled_fold = function == CoreArrayFunction::FoldControl;
    let callback_index = if function == CoreArrayFunction::Fold || controlled_fold {
        2
    } else {
        1
    };
    let callback = arguments[callback_index];
    let Some(actual_callback_arity) = view
        .resolved_function_arity(callback)
        .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?
    else {
        return Err(runtime_type_error(
            "Func",
            &callback,
            &view,
            &call_function,
            call_pc,
        ));
    };
    let expected_callback_arity = if function == CoreArrayFunction::Fold || controlled_fold {
        2
    } else {
        1
    };
    if actual_callback_arity != expected_callback_arity {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            format!(
                "{} callback must accept {expected_callback_arity} arguments, got {actual_callback_arity}",
                core_array_name(function)
            ),
            &call_function,
            call_pc,
        ));
    }

    let accumulator =
        (function == CoreArrayFunction::Fold || controlled_fold).then_some(arguments[1]);
    let continuation = ArrayContinuation {
        function,
        source,
        callback,
        next_index: 0,
        accumulator,
        output: Vec::new(),
        failed: None,
        return_target,
        trace_frame: RuntimeFrame {
            function: core_array_name(function).into(),
            instruction: 0,
            origin: call_function.origin_at(call_pc),
        },
        call_function,
        call_pc,
    };
    next_array_action(continuation, current, background, account)
}

fn resume_array_continuation(
    mut continuation: ArrayContinuation,
    value: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    match continuation.function {
        CoreArrayFunction::Length
        | CoreArrayFunction::Get
        | CoreArrayFunction::Enumerate
        | CoreArrayFunction::Push
        | CoreArrayFunction::Concat
        | CoreArrayFunction::Zip => {
            unreachable!("non-callback array operation does not suspend")
        }
        CoreArrayFunction::Map => {
            charge_array_output(&continuation, account, 1)?;
            continuation.output.push(value);
        }
        CoreArrayFunction::Filter => match value.value() {
            DecodedValue::BuiltinAtom(BuiltinAtom::True) => {
                let item = array_item(
                    continuation.source,
                    continuation.next_index - 1,
                    current,
                    background,
                    &continuation.call_function,
                    continuation.call_pc,
                )?;
                charge_array_output(&continuation, account, 1)?;
                continuation.output.push(item);
            }
            DecodedValue::BuiltinAtom(BuiltinAtom::False) => {}
            _ => {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/array.filter predicate must return 'True or 'False",
                    &continuation.call_function,
                    continuation.call_pc,
                ));
            }
        },
        CoreArrayFunction::FlatMap => {
            let DecodedValue::Array(handle) = value.value() else {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/array.flat_map callback must return an Array",
                    &continuation.call_function,
                    continuation.call_pc,
                ));
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let values = view
                .sequence(handle, false)
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        &continuation.call_function,
                        continuation.call_pc,
                    )
                })?
                .to_vec();
            charge_array_output(&continuation, account, values.len())?;
            continuation.output.extend(values);
        }
        CoreArrayFunction::Fold => continuation.accumulator = Some(value),
        CoreArrayFunction::FoldControl => {
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/array.fold_control callback must return 'Continue(value) or 'Break(value)",
                    &continuation.call_function,
                    continuation.call_pc,
                ));
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let (tag, payload) = view.tagged(handle).map_err(|heap_error| {
                core_dict_heap_error(
                    heap_error,
                    &continuation.call_function,
                    continuation.call_pc,
                )
            })?;
            let tag = view.atom_text(tag).map_err(|heap_error| {
                core_dict_heap_error(
                    heap_error,
                    &continuation.call_function,
                    continuation.call_pc,
                )
            })?;
            match tag.as_ref().map(crate::TextRef::as_str) {
                Some("Continue") => continuation.accumulator = Some(payload),
                Some("Break") => {
                    return Ok(VmAction::Return {
                        value,
                        return_target: continuation.return_target,
                    });
                }
                _ => {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        "std/array.fold_control callback must return 'Continue(value) or 'Break(value)",
                        &continuation.call_function,
                        continuation.call_pc,
                    ));
                }
            }
        }
        CoreArrayFunction::Any | CoreArrayFunction::All | CoreArrayFunction::Find => {
            let matched = match value.value() {
                DecodedValue::BuiltinAtom(BuiltinAtom::True) => true,
                DecodedValue::BuiltinAtom(BuiltinAtom::False) => false,
                _ => {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!(
                            "{} predicate must return 'True or 'False",
                            continuation.function.name()
                        ),
                        &continuation.call_function,
                        continuation.call_pc,
                    ));
                }
            };
            if continuation.function == CoreArrayFunction::Any && matched {
                return Ok(VmAction::Return {
                    value: Val::new(
                        DecodedValue::BuiltinAtom(BuiltinAtom::True),
                        instruction_location(&continuation.call_function, continuation.call_pc),
                    ),
                    return_target: continuation.return_target,
                });
            }
            if continuation.function == CoreArrayFunction::All && !matched {
                return Ok(VmAction::Return {
                    value: Val::new(
                        DecodedValue::BuiltinAtom(BuiltinAtom::False),
                        instruction_location(&continuation.call_function, continuation.call_pc),
                    ),
                    return_target: continuation.return_target,
                });
            }
            if continuation.function == CoreArrayFunction::Find && matched {
                if let Some(failure) = continuation.failed {
                    return Ok(VmAction::Return {
                        value: failure,
                        return_target: continuation.return_target,
                    });
                }
                let item = array_item(
                    continuation.source,
                    continuation.next_index - 1,
                    current,
                    background,
                    &continuation.call_function,
                    continuation.call_pc,
                )?;
                charge_array_output(&continuation, account, 2)?;
                return Ok(VmAction::Return {
                    value: Val::new(
                        DecodedValue::Tagged(current.allocate(Object::Tagged {
                            tag: Val::new(
                                DecodedValue::BuiltinAtom(BuiltinAtom::Some),
                                instruction_location(
                                    &continuation.call_function,
                                    continuation.call_pc,
                                ),
                            ),
                            payload: item,
                        })),
                        instruction_location(&continuation.call_function, continuation.call_pc),
                    ),
                    return_target: continuation.return_target,
                });
            }
        }
    }
    next_array_action(continuation, current, background, account)
}

fn resume_array_failure(
    mut continuation: ArrayContinuation,
    failure: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    match continuation.function {
        CoreArrayFunction::Map => continuation.output.push(failure),
        CoreArrayFunction::Fold | CoreArrayFunction::FoldControl => {
            return Ok(VmAction::Return {
                value: failure,
                return_target: continuation.return_target,
            });
        }
        CoreArrayFunction::Filter
        | CoreArrayFunction::FlatMap
        | CoreArrayFunction::Any
        | CoreArrayFunction::All
        | CoreArrayFunction::Find => {
            continuation.failed.get_or_insert(failure);
        }
        CoreArrayFunction::Length
        | CoreArrayFunction::Get
        | CoreArrayFunction::Enumerate
        | CoreArrayFunction::Push
        | CoreArrayFunction::Concat
        | CoreArrayFunction::Zip => unreachable!("non-callback operation cannot resume"),
    }
    next_array_action(continuation, current, background, account)
}

fn next_array_action(
    mut continuation: ArrayContinuation,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let DecodedValue::Array(handle) = continuation.source.value() else {
        unreachable!("validated Array continuation source")
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let length = view
        .sequence(handle, false)
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                &continuation.call_function,
                continuation.call_pc,
            )
        })?
        .len();
    if continuation.next_index >= length {
        if let Some(failure) = continuation.failed {
            return Ok(VmAction::Return {
                value: failure,
                return_target: continuation.return_target,
            });
        }
        let value = match continuation.function {
            CoreArrayFunction::Fold => continuation
                .accumulator
                .expect("fold continuation has an accumulator"),
            CoreArrayFunction::FoldControl => {
                let accumulator = continuation
                    .accumulator
                    .expect("controlled fold continuation has an accumulator");
                charge_allocation(
                    account,
                    logical_value_bytes(2).map_err(|error| {
                        allocation_error(
                            error.message,
                            &continuation.call_function,
                            continuation.call_pc,
                        )
                    })?,
                    &continuation.call_function,
                    continuation.call_pc,
                )?;
                let continue_tag = current.intern("Continue");
                Val::new(
                    DecodedValue::Tagged(current.allocate(Object::Tagged {
                        tag: Val::new(
                            DecodedValue::Atom(continue_tag),
                            instruction_location(&continuation.call_function, continuation.call_pc),
                        ),
                        payload: accumulator,
                    })),
                    instruction_location(&continuation.call_function, continuation.call_pc),
                )
            }
            CoreArrayFunction::Any => Val::new(
                DecodedValue::BuiltinAtom(BuiltinAtom::False),
                instruction_location(&continuation.call_function, continuation.call_pc),
            ),
            CoreArrayFunction::All => Val::new(
                DecodedValue::BuiltinAtom(BuiltinAtom::True),
                instruction_location(&continuation.call_function, continuation.call_pc),
            ),
            CoreArrayFunction::Find => Val::new(
                DecodedValue::BuiltinAtom(BuiltinAtom::None),
                instruction_location(&continuation.call_function, continuation.call_pc),
            ),
            CoreArrayFunction::Map | CoreArrayFunction::Filter | CoreArrayFunction::FlatMap => {
                Val::new(
                    DecodedValue::Array(
                        current.allocate(Object::Array(continuation.output.into())),
                    ),
                    instruction_location(&continuation.call_function, continuation.call_pc),
                )
            }
            CoreArrayFunction::Length
            | CoreArrayFunction::Get
            | CoreArrayFunction::Enumerate
            | CoreArrayFunction::Push
            | CoreArrayFunction::Concat
            | CoreArrayFunction::Zip => {
                unreachable!()
            }
        };
        return Ok(VmAction::Return {
            value,
            return_target: continuation.return_target,
        });
    }

    let item = array_item(
        continuation.source,
        continuation.next_index,
        current,
        background,
        &continuation.call_function,
        continuation.call_pc,
    )?;
    continuation.next_index += 1;
    if matches!(item.value(), DecodedValue::Failed(_)) {
        return resume_array_failure(continuation, item, current, background, account);
    }
    let arguments = if matches!(
        continuation.function,
        CoreArrayFunction::Fold | CoreArrayFunction::FoldControl
    ) {
        vec![
            continuation
                .accumulator
                .expect("fold continuation has an accumulator"),
            item,
        ]
    } else {
        vec![item]
    };
    let callee = continuation.callback;
    let call_function = Arc::clone(&continuation.call_function);
    let call_pc = continuation.call_pc;
    Ok(VmAction::Call {
        callee,
        arguments,
        return_target: ReturnTarget::Native(Box::new(continuation)),
        call_function,
        call_pc,
    })
}

fn array_item(
    source: Val,
    index: usize,
    current: &Heap,
    background: &Heap,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    let DecodedValue::Array(handle) = source.value() else {
        unreachable!("validated Array source")
    };
    HeapView {
        current,
        background: Some(background),
    }
    .sequence(handle, false)
    .map_err(|heap_error| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            heap_error.to_string(),
            function,
            pc,
        )
    })?
    .get(index)
    .copied()
    .ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "Array continuation index is out of bounds",
            function,
            pc,
        )
    })
}

fn charge_array_output(
    continuation: &ArrayContinuation,
    account: &mut QuotaAccount,
    count: usize,
) -> Result<(), RuntimeError> {
    let bytes = logical_value_bytes(count).map_err(|native_error| {
        allocation_error(
            native_error.message,
            &continuation.call_function,
            continuation.call_pc,
        )
    })?;
    charge_allocation(
        account,
        bytes,
        &continuation.call_function,
        continuation.call_pc,
    )
}

const fn core_array_name(function: CoreArrayFunction) -> &'static str {
    function.name()
}

#[allow(clippy::too_many_arguments)]
fn run_core_string(
    operation: CoreStringFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let argument = |index: usize| -> Result<String, RuntimeError> {
        let view = HeapView {
            current,
            background: Some(background),
        };
        view.string_text(arguments[index])
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            .map(|text| text.as_str().to_owned())
            .ok_or_else(|| runtime_type_error("String", &arguments[index], &view, function, pc))
    };
    let call_loc = instruction_location(function, pc);
    let value = match operation {
        CoreStringFunction::Length => {
            let length = i64::try_from(argument(0)?.chars().count()).map_err(|_| {
                error(
                    RuntimeErrorKind::IntegerOverflow,
                    "String length does not fit Int",
                    function,
                    pc,
                )
            })?;
            Val::new(DecodedValue::Int(length), call_loc)
        }
        CoreStringFunction::Join | CoreStringFunction::JoinLines => {
            let DecodedValue::Array(handle) = arguments[0].value() else {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                return Err(runtime_type_error(
                    "Array(String)",
                    &arguments[0],
                    &view,
                    function,
                    pc,
                ));
            };
            let separator = if operation == CoreStringFunction::Join {
                argument(1)?
            } else {
                "\n".to_owned()
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let items = view
                .sequence(handle, false)
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
            let mut strings = Vec::with_capacity(items.len());
            for (index, item) in items.iter().copied().enumerate() {
                propagate_direct_failure(&item, function, pc)?;
                let text = view
                    .string_text(item)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            format!("{} item {index} must be a String", operation.name()),
                            function,
                            pc,
                        )
                    })?;
                strings.push(text.as_str().to_owned());
            }
            let output = strings.join(&separator);
            charge_allocation(account, output.len() as u64, function, pc)?;
            Val::new(current.string(Some(background), &output), call_loc)
        }
        CoreStringFunction::Split | CoreStringFunction::Lines => {
            let source = argument(0)?;
            let pieces = if operation == CoreStringFunction::Lines {
                source
                    .split('\n')
                    .map(|line| line.strip_suffix('\r').unwrap_or(line))
                    .collect::<Vec<_>>()
            } else {
                let separator = argument(1)?;
                source.split(&separator).collect::<Vec<_>>()
            };
            let text_bytes = pieces.iter().try_fold(0u64, |total, piece| {
                total.checked_add(piece.len() as u64).ok_or_else(|| {
                    allocation_error("String split allocation size overflowed", function, pc)
                })
            })?;
            let slot_bytes = logical_value_bytes(pieces.len())
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
            charge_allocation(
                account,
                text_bytes.checked_add(slot_bytes).ok_or_else(|| {
                    allocation_error("String split allocation size overflowed", function, pc)
                })?,
                function,
                pc,
            )?;
            let values = pieces
                .into_iter()
                .map(|piece| Val::new(current.string(Some(background), piece), call_loc))
                .collect::<Box<[_]>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(values))),
                call_loc,
            )
        }
        CoreStringFunction::StartsWith
        | CoreStringFunction::EndsWith
        | CoreStringFunction::Contains => {
            let source = argument(0)?;
            let needle = argument(1)?;
            let result = match operation {
                CoreStringFunction::StartsWith => source.starts_with(&needle),
                CoreStringFunction::EndsWith => source.ends_with(&needle),
                CoreStringFunction::Contains => source.contains(&needle),
                _ => unreachable!(),
            };
            Val::new(
                DecodedValue::BuiltinAtom(if result {
                    BuiltinAtom::True
                } else {
                    BuiltinAtom::False
                }),
                call_loc,
            )
        }
        CoreStringFunction::Replace => {
            let output = argument(0)?.replace(&argument(1)?, &argument(2)?);
            charge_allocation(account, output.len() as u64, function, pc)?;
            Val::new(current.string(Some(background), &output), call_loc)
        }
        CoreStringFunction::Indent => {
            let source = argument(0)?;
            let DecodedValue::Int(width) = arguments[1].value() else {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                return Err(runtime_type_error(
                    "Int",
                    &arguments[1],
                    &view,
                    function,
                    pc,
                ));
            };
            let width = usize::try_from(width).map_err(|_| {
                error(
                    RuntimeErrorKind::TypeMismatch,
                    "String indentation width must be non-negative",
                    function,
                    pc,
                )
            })?;
            let indented_lines = source
                .split_inclusive('\n')
                .filter(|line| !line.trim_matches(['\r', '\n']).is_empty())
                .count();
            let output_bytes = width
                .checked_mul(indented_lines)
                .and_then(|added| source.len().checked_add(added))
                .and_then(|bytes| u64::try_from(bytes).ok())
                .ok_or_else(|| {
                    allocation_error("String indentation size overflowed", function, pc)
                })?;
            charge_allocation(account, output_bytes, function, pc)?;
            let prefix = " ".repeat(width);
            let mut output = String::with_capacity(output_bytes as usize);
            for line in source.split_inclusive('\n') {
                if !line.trim_matches(['\r', '\n']).is_empty() {
                    output.push_str(&prefix);
                }
                output.push_str(line);
            }
            Val::new(current.string(Some(background), &output), call_loc)
        }
        CoreStringFunction::EnsureTrailingNewline => {
            let mut output = argument(0)?;
            if !output.ends_with('\n') {
                output.push('\n');
            }
            charge_allocation(account, output.len() as u64, function, pc)?;
            Val::new(current.string(Some(background), &output), call_loc)
        }
        CoreStringFunction::TrimMargin => {
            let source = argument(0)?;
            let margin = argument(1)?;
            if margin.is_empty() {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "String margin marker must not be empty",
                    function,
                    pc,
                ));
            }
            let mut output = String::new();
            for line in source.split_inclusive('\n') {
                let content_end = line.trim_end_matches(['\r', '\n']).len();
                let content = &line[..content_end];
                let newline = &line[content_end..];
                let marker = content
                    .bytes()
                    .take_while(|byte| matches!(byte, b' ' | b'\t'))
                    .count();
                output.push_str(content[marker..].strip_prefix(&margin).unwrap_or(content));
                output.push_str(newline);
            }
            charge_allocation(account, output.len() as u64, function, pc)?;
            Val::new(current.string(Some(background), &output), call_loc)
        }
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

fn normalize_lexical_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut components: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." if components.last().is_some_and(|last| *last != "..") => {
                components.pop();
            }
            ".." if !absolute => components.push(component),
            ".." => {}
            _ => components.push(component),
        }
    }
    if absolute {
        if components.is_empty() {
            "/".into()
        } else {
            format!("/{}", components.join("/"))
        }
    } else if components.is_empty() {
        ".".into()
    } else {
        components.join("/")
    }
}

#[allow(clippy::too_many_arguments)]
fn run_core_path(
    operation: CorePathFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let call_loc = instruction_location(function, pc);
    let input = if operation == CorePathFunction::Join {
        let DecodedValue::Array(handle) = arguments[0].value() else {
            let view = HeapView {
                current,
                background: Some(background),
            };
            return Err(runtime_type_error(
                "Array(String)",
                &arguments[0],
                &view,
                function,
                pc,
            ));
        };
        let view = HeapView {
            current,
            background: Some(background),
        };
        let items = view
            .sequence(handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
        let mut joined = String::new();
        for (index, item) in items.iter().copied().enumerate() {
            propagate_direct_failure(&item, function, pc)?;
            let part = view
                .string_text(item)
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                .ok_or_else(|| {
                    error(
                        RuntimeErrorKind::TypeMismatch,
                        format!("{} item {index} must be a String", operation.name()),
                        function,
                        pc,
                    )
                })?;
            if part.starts_with('/') {
                joined.clear();
                joined.push_str(part.as_str());
            } else {
                if !joined.is_empty() && !joined.ends_with('/') {
                    joined.push('/');
                }
                joined.push_str(part.as_str());
            }
        }
        joined
    } else {
        let view = HeapView {
            current,
            background: Some(background),
        };
        view.string_text(arguments[0])
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            .map(|text| text.as_str().to_owned())
            .ok_or_else(|| runtime_type_error("String", &arguments[0], &view, function, pc))?
    };
    let normalized = normalize_lexical_path(&input);
    let result = match operation {
        CorePathFunction::Join | CorePathFunction::Normalize => Some(normalized),
        CorePathFunction::Parent => match normalized.as_str() {
            "." | "/" => None,
            value if value.starts_with('/') => value
                .rfind('/')
                .map(|index| if index == 0 { "/" } else { &value[..index] })
                .map(str::to_owned),
            value => Some(
                value
                    .rfind('/')
                    .map_or(".", |index| &value[..index])
                    .to_owned(),
            ),
        },
        CorePathFunction::FileName => match normalized.as_str() {
            "." | "/" | ".." => None,
            value => Some(value.rsplit('/').next().expect("non-empty path").to_owned()),
        },
    };
    let value = if matches!(
        operation,
        CorePathFunction::Parent | CorePathFunction::FileName
    ) {
        if let Some(result) = result {
            let bytes = (result.len() as u64)
                .checked_add(
                    logical_value_bytes(2).map_err(|native_error| {
                        allocation_error(native_error.message, function, pc)
                    })?,
                )
                .ok_or_else(|| allocation_error("Path allocation size overflowed", function, pc))?;
            charge_allocation(account, bytes, function, pc)?;
            let payload = Val::new(current.string(Some(background), &result), call_loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), call_loc),
                    payload,
                })),
                call_loc,
            )
        } else {
            Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), call_loc)
        }
    } else {
        let result = result.expect("path String operation returns a value");
        charge_allocation(account, result.len() as u64, function, pc)?;
        Val::new(current.string(Some(background), &result), call_loc)
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_core_hash(
    operation: CoreHashFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let input = view
        .string_text(arguments[0])
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                function,
                pc,
            )
        })?
        .ok_or_else(|| {
            error(
                RuntimeErrorKind::TypeMismatch,
                format!("{} expects String", operation.name()),
                function,
                pc,
            )
        })?;
    let digest = match operation {
        CoreHashFunction::Sha256 => crate::sha256::hex(input.as_bytes()),
    };
    charge_allocation(account, digest.len() as u64, function, pc)?;
    Ok(VmAction::Return {
        value: Val::new(
            current.string(Some(background), &digest),
            instruction_location(function, pc),
        ),
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_core_dict(
    operation: CoreDictFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let value = match operation {
        CoreDictFunction::Get => {
            let DecodedValue::Dict(handle) = arguments[0].value() else {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                return Err(runtime_type_error(
                    "Dict",
                    &arguments[0],
                    &view,
                    function,
                    pc,
                ));
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let Some(key) = view
                .string_text(arguments[1])
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            else {
                return Err(runtime_type_error(
                    "String",
                    &arguments[1],
                    &view,
                    function,
                    pc,
                ));
            };
            match view
                .dict_get_text(handle, key.as_str())
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            {
                Some(payload) => {
                    charge_allocation(
                        account,
                        logical_value_bytes(2).map_err(|native_error| {
                            allocation_error(native_error.message, function, pc)
                        })?,
                        function,
                        pc,
                    )?;
                    Val::new(
                        DecodedValue::Tagged(current.allocate(Object::Tagged {
                            tag: Val::new(
                                DecodedValue::BuiltinAtom(BuiltinAtom::Some),
                                arguments[0].loc(),
                            ),
                            payload,
                        })),
                        arguments[0].loc(),
                    )
                }
                None => Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::None),
                    arguments[0].loc(),
                ),
            }
        }
        CoreDictFunction::Keys => {
            let entries =
                core_dict_entries(arguments[0], "Dict", function, pc, current, background)?;
            charge_core_dict_output(
                entries.len(),
                entries.iter().map(|(field, _)| field.len()),
                function,
                pc,
                account,
            )?;
            let values = entries
                .into_iter()
                .map(|(field, _)| {
                    Val::new(current.string(Some(background), &field), arguments[0].loc())
                })
                .collect::<Box<[_]>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(values))),
                instruction_location(function, pc),
            )
        }
        CoreDictFunction::Values => {
            let entries =
                core_dict_entries(arguments[0], "Dict", function, pc, current, background)?;
            charge_core_dict_output(entries.len(), std::iter::empty(), function, pc, account)?;
            let values = entries
                .into_iter()
                .map(|(_, value)| value)
                .collect::<Box<[_]>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(values))),
                instruction_location(function, pc),
            )
        }
        CoreDictFunction::Pairs => {
            let entries =
                core_dict_entries(arguments[0], "Dict", function, pc, current, background)?;
            let slot_count = entries.len().checked_mul(3).ok_or_else(|| {
                allocation_error("std/dict.pairs allocation size overflowed", function, pc)
            })?;
            charge_core_dict_output(
                slot_count,
                entries.iter().map(|(field, _)| field.len()),
                function,
                pc,
                account,
            )?;
            let pairs = entries
                .into_iter()
                .map(|(field, value)| {
                    let field =
                        Val::new(current.string(Some(background), &field), arguments[0].loc());
                    Val::new(
                        DecodedValue::Tuple(
                            current.allocate(Object::Tuple(vec![field, value].into())),
                        ),
                        arguments[0].loc(),
                    )
                })
                .collect::<Box<[_]>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(pairs))),
                instruction_location(function, pc),
            )
        }
        CoreDictFunction::FromPairs => {
            let DecodedValue::Array(handle) = arguments[0].value() else {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                return Err(runtime_type_error(
                    "Array",
                    &arguments[0],
                    &view,
                    function,
                    pc,
                ));
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let items = view
                .sequence(handle, false)
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
            let mut entries = Vec::with_capacity(items.len());
            for (index, item) in items.iter().copied().enumerate() {
                let DecodedValue::Tuple(pair) = item.value() else {
                    if let DecodedValue::Failed(failure) = item.value() {
                        return Err(propagated_failure_error(failure, item.loc(), function, pc));
                    }
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!("std/dict.from_pairs item {index} must be a two-element Tuple"),
                        function,
                        pc,
                    ));
                };
                let pair = view
                    .sequence(pair, true)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
                if pair.len() != 2 {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!("std/dict.from_pairs item {index} must be a two-element Tuple"),
                        function,
                        pc,
                    ));
                }
                let Some(field) = view
                    .string_text(pair[0])
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                else {
                    propagate_direct_failure(&pair[0], function, pc)?;
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!("std/dict.from_pairs item {index} key must be a String"),
                        function,
                        pc,
                    ));
                };
                entries.push((field.as_str().to_owned(), pair[1]));
            }
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            if let Some(duplicate) = entries
                .windows(2)
                .find(|pair| pair[0].0 == pair[1].0)
                .map(|pair| pair[0].0.as_str())
            {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    format!("std/dict.from_pairs contains duplicate field {duplicate:?}"),
                    function,
                    pc,
                ));
            }
            allocate_core_dict(entries, function, pc, current, account)?
        }
        CoreDictFunction::Merge => {
            let left =
                core_dict_entries(arguments[0], "left Dict", function, pc, current, background)?;
            let right = core_dict_entries(
                arguments[1],
                "right Dict",
                function,
                pc,
                current,
                background,
            )?;
            let mut merged = Vec::with_capacity(left.len().saturating_add(right.len()));
            let (mut left_index, mut right_index) = (0, 0);
            while left_index < left.len() && right_index < right.len() {
                match left[left_index].0.cmp(&right[right_index].0) {
                    std::cmp::Ordering::Less => {
                        merged.push(left[left_index].clone());
                        left_index += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        merged.push(right[right_index].clone());
                        left_index += 1;
                        right_index += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        merged.push(right[right_index].clone());
                        right_index += 1;
                    }
                }
            }
            merged.extend_from_slice(&left[left_index..]);
            merged.extend_from_slice(&right[right_index..]);
            allocate_core_dict(merged, function, pc, current, account)?
        }
        CoreDictFunction::MapValues | CoreDictFunction::Filter | CoreDictFunction::Fold => {
            unreachable!("callback Dict operations use continuations")
        }
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn start_dict_continuation(
    function: CoreDictFunction,
    arguments: Vec<Val>,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let entries = core_dict_entries(
        arguments[0],
        "Dict",
        &call_function,
        call_pc,
        current,
        background,
    )?;
    let callback_index = if function == CoreDictFunction::Fold {
        2
    } else {
        1
    };
    let callback = arguments[callback_index];
    let view = HeapView {
        current,
        background: Some(background),
    };
    let Some(actual_arity) = view
        .resolved_function_arity(callback)
        .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?
    else {
        return Err(runtime_type_error(
            "Func",
            &callback,
            &view,
            &call_function,
            call_pc,
        ));
    };
    let expected_arity = if function == CoreDictFunction::Fold {
        3
    } else {
        1
    };
    if actual_arity != expected_arity {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            format!(
                "{} callback must accept {expected_arity} arguments, got {actual_arity}",
                function.name()
            ),
            &call_function,
            call_pc,
        ));
    }
    let accumulator = (function == CoreDictFunction::Fold).then_some(arguments[1]);
    next_dict_action(
        DictContinuation {
            function,
            entries,
            callback,
            next_index: 0,
            accumulator,
            output: Vec::new(),
            failed: None,
            return_target,
            trace_frame: RuntimeFrame {
                function: function.name().into(),
                instruction: 0,
                origin: call_function.origin_at(call_pc),
            },
            call_function,
            call_pc,
        },
        current,
        background,
        account,
    )
}

fn resume_dict_continuation(
    mut continuation: DictContinuation,
    value: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let entry_index = continuation.next_index - 1;
    match continuation.function {
        CoreDictFunction::MapValues => {
            let key = continuation.entries[entry_index].0.clone();
            charge_core_dict_output(
                1,
                std::iter::once(key.len()),
                &continuation.call_function,
                continuation.call_pc,
                account,
            )?;
            continuation.output.push((key, value));
        }
        CoreDictFunction::Filter => match value.value() {
            DecodedValue::BuiltinAtom(BuiltinAtom::True) => {
                charge_core_dict_output(
                    1,
                    std::iter::once(continuation.entries[entry_index].0.len()),
                    &continuation.call_function,
                    continuation.call_pc,
                    account,
                )?;
                continuation
                    .output
                    .push(continuation.entries[entry_index].clone());
            }
            DecodedValue::BuiltinAtom(BuiltinAtom::False) => {}
            _ => {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/dict.filter predicate must return 'True or 'False",
                    &continuation.call_function,
                    continuation.call_pc,
                ));
            }
        },
        CoreDictFunction::Fold => continuation.accumulator = Some(value),
        _ => unreachable!("only callback Dict operations suspend"),
    }
    next_dict_action(continuation, current, background, account)
}

fn resume_dict_failure(
    mut continuation: DictContinuation,
    failure: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    match continuation.function {
        CoreDictFunction::MapValues => {
            let key = continuation.entries[continuation.next_index - 1].0.clone();
            continuation.output.push((key, failure));
        }
        CoreDictFunction::Filter => {
            continuation.failed.get_or_insert(failure);
        }
        CoreDictFunction::Fold => {
            return Ok(VmAction::Return {
                value: failure,
                return_target: continuation.return_target,
            });
        }
        _ => unreachable!("only callback Dict operations suspend"),
    }
    next_dict_action(continuation, current, background, account)
}

fn next_dict_action(
    mut continuation: DictContinuation,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    if continuation.next_index >= continuation.entries.len() {
        if let Some(failure) = continuation.failed {
            return Ok(VmAction::Return {
                value: failure,
                return_target: continuation.return_target,
            });
        }
        let value = if continuation.function == CoreDictFunction::Fold {
            continuation
                .accumulator
                .expect("fold continuation has an accumulator")
        } else {
            allocate_core_dict_unchecked(
                continuation.output,
                current,
                instruction_location(&continuation.call_function, continuation.call_pc),
            )
        };
        return Ok(VmAction::Return {
            value,
            return_target: continuation.return_target,
        });
    }

    let (key, value) = continuation.entries[continuation.next_index].clone();
    continuation.next_index += 1;
    if matches!(value.value(), DecodedValue::Failed(_)) {
        return resume_dict_failure(continuation, value, current, background, account);
    }
    let arguments = if continuation.function == CoreDictFunction::Fold {
        charge_allocation(
            account,
            key.len() as u64,
            &continuation.call_function,
            continuation.call_pc,
        )?;
        vec![
            continuation
                .accumulator
                .expect("fold continuation has an accumulator"),
            Val::new(current.string(Some(background), &key), value.loc()),
            value,
        ]
    } else {
        vec![value]
    };
    let callee = continuation.callback;
    let call_function = Arc::clone(&continuation.call_function);
    let call_pc = continuation.call_pc;
    Ok(VmAction::Call {
        callee,
        arguments,
        return_target: ReturnTarget::Native(Box::new(continuation)),
        call_function,
        call_pc,
    })
}

fn core_dict_entries(
    value: Val,
    expected: &str,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<Vec<(String, Val)>, RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let DecodedValue::Dict(handle) = value.value() else {
        return Err(runtime_type_error(expected, &value, &view, function, pc));
    };
    let (fields, values) = view
        .dict_parts(handle)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
    fields
        .iter()
        .zip(values)
        .map(|(field, value)| {
            Ok((
                view.text(*field)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .to_owned(),
                *value,
            ))
        })
        .collect()
}

fn allocate_core_dict(
    entries: Vec<(String, Val)>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    account: &mut QuotaAccount,
) -> Result<Val, RuntimeError> {
    charge_core_dict_output(
        entries.len(),
        entries.iter().map(|(field, _)| field.len()),
        function,
        pc,
        account,
    )?;
    Ok(allocate_core_dict_unchecked(
        entries,
        current,
        instruction_location(function, pc),
    ))
}

fn allocate_core_dict_unchecked(
    entries: Vec<(String, Val)>,
    current: &mut Heap,
    loc: Option<crate::Loc>,
) -> Val {
    let (fields, values): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .map(|(field, value)| (current.intern(&field), value))
        .unzip();
    let shape = current.intern_shape(fields);
    Val::new(
        DecodedValue::Dict(current.allocate(Object::Dict {
            shape,
            values: values.into(),
        })),
        loc,
    )
}

fn charge_core_dict_output(
    value_slots: usize,
    mut text_lengths: impl Iterator<Item = usize>,
    function: &BytecodeFunction,
    pc: usize,
    account: &mut QuotaAccount,
) -> Result<(), RuntimeError> {
    let text_bytes = text_lengths.try_fold(0u64, |total, length| {
        total
            .checked_add(length as u64)
            .ok_or_else(|| allocation_error("std/dict allocation size overflowed", function, pc))
    })?;
    let value_bytes = logical_value_bytes(value_slots)
        .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
    let bytes = text_bytes
        .checked_add(value_bytes)
        .ok_or_else(|| allocation_error("std/dict allocation size overflowed", function, pc))?;
    charge_allocation(account, bytes, function, pc)
}

fn core_dict_heap_error(
    heap_error: crate::heap::HeapError,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    error(
        RuntimeErrorKind::InvalidBytecode,
        heap_error.to_string(),
        function,
        pc,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_core_attributes(
    operation: CoreAttributesFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let (inner, mut attributes) =
        flatten_attributes(arguments[0], "value", function, pc, current, background)?;
    let call_loc = instruction_location(function, pc);
    let value = match operation {
        CoreAttributesFunction::Normalize => allocate_attributes_wrapper(
            inner, attributes, call_loc, function, pc, current, account,
        )?,
        CoreAttributesFunction::Add => {
            let additions = core_dict_entries(
                arguments[1],
                "attributes Dict",
                function,
                pc,
                current,
                background,
            )?;
            for (key, value) in additions {
                attributes.insert(key, value);
            }
            allocate_attributes_wrapper(
                inner, attributes, call_loc, function, pc, current, account,
            )?
        }
        CoreAttributesFunction::Get | CoreAttributesFunction::Has => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            let key = view
                .string_text(arguments[1])
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                .ok_or_else(|| {
                    runtime_type_error("String key", &arguments[1], &view, function, pc)
                })?;
            let found = attributes.get(key.as_str()).copied();
            if operation == CoreAttributesFunction::Has {
                Val::new(
                    DecodedValue::BuiltinAtom(if found.is_some() {
                        BuiltinAtom::True
                    } else {
                        BuiltinAtom::False
                    }),
                    call_loc,
                )
            } else if let Some(payload) = found {
                charge_allocation(
                    account,
                    logical_value_bytes(2)
                        .map_err(|error| allocation_error(error.message, function, pc))?,
                    function,
                    pc,
                )?;
                Val::new(
                    DecodedValue::Tagged(current.allocate(Object::Tagged {
                        tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), call_loc),
                        payload,
                    })),
                    call_loc,
                )
            } else {
                Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), call_loc)
            }
        }
        CoreAttributesFunction::All => allocate_core_dict(
            attributes.into_iter().collect(),
            function,
            pc,
            current,
            account,
        )?,
        CoreAttributesFunction::Strip => inner,
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

fn flatten_attributes(
    mut value: Val,
    path: &str,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<(Val, BTreeMap<String, Val>), RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let mut layers = Vec::new();
    while let DecodedValue::Dict(handle) = value.value() {
        let Some(kind) = view
            .dict_get_text(handle, "kind")
            .map_err(|error| core_dict_heap_error(error, function, pc))?
        else {
            break;
        };
        let Some(kind) = view
            .atom_text(kind)
            .map_err(|error| core_dict_heap_error(error, function, pc))?
        else {
            break;
        };
        if kind != "WithAttributes" {
            break;
        }
        let fields = view
            .dict_fields(handle)
            .map_err(|error| core_dict_heap_error(error, function, pc))?;
        if fields != ["attributes", "inner", "kind"] {
            return Err(error(
                RuntimeErrorKind::TypeMismatch,
                format!(
                    "{path} WithAttributes wrapper must have exactly attributes, inner, and kind fields"
                ),
                function,
                pc,
            ));
        }
        let inner = view
            .dict_get_text(handle, "inner")
            .map_err(|error| core_dict_heap_error(error, function, pc))?
            .expect("validated wrapper field");
        let attributes = view
            .dict_get_text(handle, "attributes")
            .map_err(|error| core_dict_heap_error(error, function, pc))?
            .expect("validated wrapper field");
        let DecodedValue::Dict(attributes) = attributes.value() else {
            return Err(error(
                RuntimeErrorKind::TypeMismatch,
                format!("{path}.attributes must be a Dict"),
                function,
                pc,
            ));
        };
        let (names, values) = view
            .dict_parts(attributes)
            .map_err(|error| core_dict_heap_error(error, function, pc))?;
        let layer = names
            .iter()
            .zip(values)
            .map(|(name, value)| {
                Ok((
                    view.text(*name)
                        .map_err(|error| core_dict_heap_error(error, function, pc))?
                        .to_owned(),
                    *value,
                ))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;
        layers.push(layer);
        value = inner;
    }
    let mut merged = BTreeMap::new();
    for layer in layers.into_iter().rev() {
        merged.extend(layer);
    }
    Ok((value, merged))
}

#[allow(clippy::too_many_arguments)]
fn allocate_attributes_wrapper(
    inner: Val,
    attributes: BTreeMap<String, Val>,
    loc: Option<crate::Loc>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    account: &mut QuotaAccount,
) -> Result<Val, RuntimeError> {
    let attributes = allocate_core_dict(
        attributes.into_iter().collect(),
        function,
        pc,
        current,
        account,
    )?;
    allocate_core_dict(
        vec![
            ("attributes".into(), attributes),
            ("inner".into(), inner),
            (
                "kind".into(),
                Val::new(DecodedValue::Atom(current.intern("WithAttributes")), loc),
            ),
        ],
        function,
        pc,
        current,
        account,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_core_model(
    operation: CoreModelFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    validate_model_context(arguments[0], function, pc, current, background)?;
    if operation == CoreModelFunction::Union {
        return run_core_union_model(
            arguments[1],
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    let member_name = match operation {
        CoreModelFunction::Struct => "fields",
        CoreModelFunction::Enum => "variants",
        CoreModelFunction::Union => unreachable!("Union handled above"),
    };
    let entries = core_dict_entries(
        arguments[1],
        &format!("{member_name} Dict"),
        function,
        pc,
        current,
        background,
    )?;
    if operation == CoreModelFunction::Enum && entries.is_empty() {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            "enum requires at least one variant",
            function,
            pc,
        ));
    }

    let mut normalized = Vec::with_capacity(entries.len());
    for (name, member) in entries {
        let path = format!("{member_name}.{name}");
        let (inner, attributes) =
            flatten_attributes(member, &path, function, pc, current, background)?;
        match operation {
            CoreModelFunction::Struct => {
                if !matches!(
                    inner.value(),
                    DecodedValue::DeclaredType(_)
                        | DecodedValue::SymbolicType(_)
                        | DecodedValue::TypeSlot(_)
                ) {
                    decode_runtime_type_at(inner, &path, current, background).map_err(
                        |message| error(RuntimeErrorKind::TypeMismatch, message, function, pc),
                    )?;
                }
            }
            CoreModelFunction::Enum => {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                let unit = view
                    .atom_text(inner)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .is_some_and(|atom| atom == "None");
                if !unit
                    && !matches!(
                        inner.value(),
                        DecodedValue::DeclaredType(_)
                            | DecodedValue::SymbolicType(_)
                            | DecodedValue::TypeSlot(_)
                    )
                {
                    decode_runtime_type_at(inner, &path, current, background).map_err(
                        |message| error(RuntimeErrorKind::TypeMismatch, message, function, pc),
                    )?;
                }
            }
            CoreModelFunction::Union => unreachable!("Union handled above"),
        }
        let member = allocate_attributes_wrapper(
            inner,
            attributes,
            member.loc().or(instruction_location(function, pc)),
            function,
            pc,
            current,
            account,
        )?;
        normalized.push((name, member));
    }

    let members = allocate_core_dict(normalized, function, pc, current, account)?;
    let kind_name = match operation {
        CoreModelFunction::Struct => "Struct",
        CoreModelFunction::Enum => "Enum",
        CoreModelFunction::Union => unreachable!("Union handled above"),
    };
    let metadata = allocate_core_dict(
        BTreeMap::from([
            (
                "kind".to_owned(),
                Val::new(
                    DecodedValue::Atom(current.intern(kind_name)),
                    instruction_location(function, pc),
                ),
            ),
            (member_name.to_owned(), members),
        ])
        .into_iter()
        .collect(),
        function,
        pc,
        current,
        account,
    )?;
    let value = allocate_attributes_wrapper(
        metadata,
        BTreeMap::new(),
        instruction_location(function, pc),
        function,
        pc,
        current,
        account,
    )?;
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_core_type_desc(
    operation: CoreTypeDescFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let input = arguments[0];
    let view = HeapView {
        current,
        background: Some(background),
    };
    match operation {
        CoreTypeDescFunction::Kind => {
            let kind = if matches!(
                input.value(),
                DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_)
            ) {
                "Ref".to_owned()
            } else if matches!(input.value(), DecodedValue::NativeType(_)) {
                "Opaque".to_owned()
            } else if matches!(input.value(), DecodedValue::TypeSlot(_)) {
                "Ref".to_owned()
            } else {
                let DecodedValue::Dict(handle) = input.value() else {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        "std/type-desc.kind expects Type metadata",
                        function,
                        pc,
                    ));
                };
                let kind = view
                    .dict_get_text(handle, "kind")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .and_then(|value| view.atom_text(value).ok().flatten())
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "std/type-desc.kind expects canonical Type metadata",
                            function,
                            pc,
                        )
                    })?;
                const KINDS: &[&str] = &[
                    "Any",
                    "Never",
                    "Type",
                    "TypeOf",
                    "Int",
                    "Float",
                    "String",
                    "Bytes",
                    "Opaque",
                    "Atom",
                    "Array",
                    "Dict",
                    "Tagged",
                    "Tuple",
                    "Struct",
                    "Enum",
                    "Union",
                    "Func",
                    "WithAttributes",
                    "Bound",
                    "Named",
                    "Dyn",
                ];
                if !KINDS.contains(&kind.as_str()) {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!("unknown Type metadata kind '{kind}"),
                        function,
                        pc,
                    ));
                }
                kind.as_str().to_owned()
            };
            Ok(VmAction::Return {
                value: Val::new(DecodedValue::Atom(current.intern(&kind)), input.loc()),
                return_target,
            })
        }
        CoreTypeDescFunction::Children => {
            let children = type_desc_children(input, &view)
                .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
            charge_allocation(
                account,
                logical_value_bytes(children.len())
                    .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
                function,
                pc,
            )?;
            Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::Array(current.allocate(Object::Array(children.into()))),
                    input.loc(),
                ),
                return_target,
            })
        }
        CoreTypeDescFunction::OpaqueName => {
            let name = if let DecodedValue::NativeType(id) = input.value() {
                Some(
                    view.native_type(id)
                        .map_err(|error| core_dict_heap_error(error, function, pc))?
                        .qualified_name()
                        .to_owned(),
                )
            } else if let DecodedValue::Dict(handle) = input.value() {
                let kind = view
                    .dict_get_text(handle, "kind")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .and_then(|value| view.atom_text(value).ok().flatten());
                if kind.is_some_and(|kind| kind == "Opaque") {
                    view.dict_get_text(handle, "name")
                        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                        .and_then(|value| view.string_text(value).ok().flatten())
                        .map(|text| text.as_str().to_owned())
                } else {
                    None
                }
            } else {
                None
            };
            let value = if let Some(name) = name {
                charge_allocation(
                    account,
                    logical_value_bytes(2)
                        .map_err(|error| allocation_error(error.message, function, pc))?
                        .saturating_add(name.len() as u64),
                    function,
                    pc,
                )?;
                let payload = Val::new(current.string(Some(background), &name), input.loc());
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), input.loc()),
                    payload,
                }))
            } else {
                DecodedValue::BuiltinAtom(BuiltinAtom::None)
            };
            Ok(VmAction::Return {
                value: Val::new(value, input.loc()),
                return_target,
            })
        }
        CoreTypeDescFunction::Resolve => {
            let result = if matches!(
                input.value(),
                DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_)
            ) {
                declared_type_body(input, &view).map_err(|message| {
                    error(RuntimeErrorKind::InvalidBytecode, message, function, pc)
                })?
            } else if let DecodedValue::TypeSlot(handle) = input.value() {
                view.type_slot(handle)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::InvalidBytecode,
                            "recursive Type reference is not initialized",
                            function,
                            pc,
                        )
                    })?
            } else {
                return type_desc_resolve_error(
                    input,
                    return_target,
                    function,
                    pc,
                    current,
                    background,
                    account,
                );
            };
            charge_allocation(
                account,
                logical_value_bytes(2)
                    .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
                function,
                pc,
            )?;
            Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::Tagged(current.allocate(Object::Tagged {
                        tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Ok), input.loc()),
                        payload: result,
                    })),
                    input.loc(),
                ),
                return_target,
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_core_dyn(
    operation: CoreDynFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    if operation == CoreDynFunction::Pack {
        decode_runtime_type(arguments[0], current, background).map_err(|message| {
            error(
                RuntimeErrorKind::TypeMismatch,
                format!("std/dyn.pack expects canonical Type metadata: {message}"),
                function,
                pc,
            )
        })?;
        charge_allocation(
            account,
            logical_value_bytes(2)
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
            function,
            pc,
        )?;
        return Ok(VmAction::Return {
            value: arguments[1].with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
                identity: Arc::new(()),
                descriptor: arguments[0],
                value: arguments[1],
                scheme: None,
                origin: None,
            }))),
            return_target,
        });
    }

    if operation == CoreDynFunction::ProjectWith {
        let DecodedValue::Dyn(handle) = arguments[1].value() else {
            return Err(runtime_shallow_type_error(
                "Dyn",
                arguments[1],
                function,
                pc,
            ));
        };
        let view = HeapView {
            current,
            background: Some(background),
        };
        let (_, packaged_descriptor, payload) = view
            .dyn_parts(handle)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
        let target = crate::types::decode_type_ref(
            ValueRef::work(arguments[0], current, background),
            "std/dyn.project_with target",
        )
        .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        let packaged = crate::types::decode_type_ref(
            ValueRef::work(packaged_descriptor, current, background),
            "std/dyn.project_with package",
        )
        .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        let target_id = current
            .canonical_descriptor_type_id(&target)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
        let packaged_id = current
            .canonical_descriptor_type_id(&packaged)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
        if target_id != packaged_id {
            return Ok(VmAction::Return {
                value: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), payload.loc()),
                return_target,
            });
        }
        charge_allocation(
            account,
            logical_value_bytes(2)
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
            function,
            pc,
        )?;
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), payload.loc()),
                    payload,
                })),
                payload.loc(),
            ),
            return_target,
        });
    }

    let DecodedValue::Dyn(handle) = arguments[0].value() else {
        return Err(runtime_shallow_type_error(
            "Dyn",
            arguments[0],
            function,
            pc,
        ));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let (_, descriptor, value) = view
        .dyn_parts(handle)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
    match operation {
        CoreDynFunction::Pack | CoreDynFunction::ProjectWith => {
            unreachable!("operation handled above")
        }
        CoreDynFunction::Desc => Ok(VmAction::Return {
            value: descriptor,
            return_target,
        }),
        CoreDynFunction::Kind => {
            let kind = match value.value() {
                DecodedValue::Failed(failure) => {
                    return Err(propagated_failure_error(failure, value.loc(), function, pc));
                }
                DecodedValue::Int(_) => "Int",
                DecodedValue::Float(_) => "Float",
                DecodedValue::InlineString(_) | DecodedValue::ShortString(_) => "String",
                DecodedValue::Bytes(_) => "Bytes",
                DecodedValue::Opaque(_) => "Opaque",
                DecodedValue::NativeType(_) => "Type",
                DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_) => "Type",
                DecodedValue::Dict(_) => "Dict",
                DecodedValue::Array(_) => "Array",
                DecodedValue::BuiltinAtom(_)
                | DecodedValue::InlineAtom(_)
                | DecodedValue::Atom(_) => "Atom",
                DecodedValue::Tagged(_) => "Tagged",
                DecodedValue::Tuple(_) => "Tuple",
                DecodedValue::Func(_) => "Func",
                DecodedValue::FuncRef(_) => "Func",
                DecodedValue::Dyn(_) => "Dyn",
                DecodedValue::Module(_) => {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        "Dyn cannot contain a Module object",
                        function,
                        pc,
                    ));
                }
                DecodedValue::TypeSlot(_) => {
                    return Err(error(
                        RuntimeErrorKind::InvalidBytecode,
                        "Dyn payload cannot be an internal up-link",
                        function,
                        pc,
                    ));
                }
            };
            Ok(VmAction::Return {
                value: Val::new(DecodedValue::Atom(current.intern(kind)), value.loc()),
                return_target,
            })
        }
        CoreDynFunction::CheckInt
        | CoreDynFunction::CheckFloat
        | CoreDynFunction::CheckString
        | CoreDynFunction::CheckBytes => {
            let expected = match operation {
                CoreDynFunction::CheckInt => "Int",
                CoreDynFunction::CheckFloat => "Float",
                CoreDynFunction::CheckString => "String",
                CoreDynFunction::CheckBytes => "Bytes",
                _ => unreachable!(),
            };
            let descriptor_kind = dyn_descriptor_leaf_kind(descriptor, &view)
                .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
            let value_matches = match operation {
                CoreDynFunction::CheckInt => matches!(value.value(), DecodedValue::Int(_)),
                CoreDynFunction::CheckFloat => matches!(value.value(), DecodedValue::Float(_)),
                CoreDynFunction::CheckString => matches!(
                    value.value(),
                    DecodedValue::InlineString(_) | DecodedValue::ShortString(_)
                ),
                CoreDynFunction::CheckBytes => matches!(value.value(), DecodedValue::Bytes(_)),
                _ => unreachable!(),
            };
            if descriptor_kind != expected || !value_matches {
                return Ok(VmAction::Return {
                    value: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), value.loc()),
                    return_target,
                });
            }
            charge_allocation(
                account,
                logical_value_bytes(2)
                    .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
                function,
                pc,
            )?;
            Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::Tagged(current.allocate(Object::Tagged {
                        tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), value.loc()),
                        payload: value,
                    })),
                    value.loc(),
                ),
                return_target,
            })
        }
        CoreDynFunction::Field
        | CoreDynFunction::Fields
        | CoreDynFunction::ArrayItems
        | CoreDynFunction::TupleItems
        | CoreDynFunction::Tag
        | CoreDynFunction::Payload => {
            let field = if operation == CoreDynFunction::Field {
                Some(
                    view.string_text(arguments[1])
                        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                        .ok_or_else(|| {
                            runtime_shallow_type_error("String", arguments[1], function, pc)
                        })?
                        .to_owned(),
                )
            } else {
                None
            };
            let observation =
                observe_dyn_structure(operation, descriptor, value, field.as_deref(), &view);
            finish_dyn_observation(
                operation,
                arguments[0],
                observation,
                return_target,
                function,
                pc,
                current,
                background,
                account,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_core_eq(
    operation: CoreEqFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<VmAction, RuntimeError> {
    match operation {
        CoreEqFunction::Equal => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            propagate_data_failures(arguments, &view, function, pc)?;
            let equal = view
                .values_equal(arguments[0], arguments[1])
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        pc,
                    )
                })?;
            Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::BuiltinAtom(if equal {
                        BuiltinAtom::True
                    } else {
                        BuiltinAtom::False
                    }),
                    instruction_location(function, pc),
                ),
                return_target,
            })
        }
    }
}

enum DynObservation {
    Child(Val, Val),
    Children(Vec<(Val, Val)>),
    NamedChildren(Vec<(String, Val, Val)>),
    Tag(String),
    Payload(Option<(Val, Val)>),
}

fn observe_dyn_structure(
    operation: CoreDynFunction,
    descriptor: Val,
    value: Val,
    field: Option<&str>,
    view: &HeapView<'_>,
) -> Result<DynObservation, String> {
    let descriptor = normalize_dyn_descriptor(descriptor, view)?;
    let value = view
        .unwrap_declared(value)
        .map_err(|error| error.to_string())?;
    let DecodedValue::Dict(type_handle) = descriptor.value() else {
        return Err("Dyn descriptor is not Type metadata".into());
    };
    let kind = view
        .dict_get_text(type_handle, "kind")
        .map_err(|error| error.to_string())?
        .and_then(|kind| view.atom_text(kind).ok().flatten())
        .ok_or_else(|| "Dyn descriptor has no Atom kind".to_owned())?;
    let type_field = |name: &str| {
        view.dict_get_text(type_handle, name)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("{kind} descriptor is missing {name}"))
    };
    match operation {
        CoreDynFunction::Field => {
            let name = field.expect("field operation has a name");
            let child_value = match value.value() {
                DecodedValue::Dict(value_handle) => view
                    .dict_get_text(value_handle, name)
                    .map_err(|error| error.to_string())?,
                DecodedValue::Module(value_handle) => view
                    .module_get_text(value_handle, name)
                    .map_err(|error| error.to_string())?,
                _ => return Err(format!("dyn.field expected {kind} runtime record")),
            };
            let child_value =
                child_value.ok_or_else(|| format!("dyn.field could not find field {name:?}"))?;
            let child_desc = match kind.as_str() {
                "Struct" => {
                    let DecodedValue::Dict(fields) = type_field("fields")?.value() else {
                        return Err("Struct.fields descriptor must be a Dict".into());
                    };
                    view.dict_get_text(fields, name)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("Struct has no declared field {name:?}"))?
                }
                "Dict" => type_field("item")?,
                _ => return Err(format!("dyn.field does not support descriptor kind {kind}")),
            };
            Ok(DynObservation::Child(child_desc, child_value))
        }
        CoreDynFunction::Fields => {
            let value_fields = match value.value() {
                DecodedValue::Dict(value_handle) => {
                    let (fields, values) = view
                        .dict_parts(value_handle)
                        .map_err(|error| error.to_string())?;
                    fields
                        .iter()
                        .zip(values)
                        .map(|(name, value)| {
                            view.text(*name)
                                .map(|name| (name.to_owned(), *value))
                                .map_err(|error| error.to_string())
                        })
                        .collect::<Result<Vec<_>, String>>()?
                }
                DecodedValue::Module(value_handle) => view
                    .module_fields(value_handle)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .map(|name| {
                        view.module_get_text(value_handle, name)
                            .map_err(|error| error.to_string())?
                            .map(|value| (name.to_owned(), value))
                            .ok_or_else(|| "Module export disappeared while iterating".to_owned())
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                _ => return Err(format!("dyn.fields expected {kind} runtime record")),
            };
            let descriptors = match kind.as_str() {
                "Struct" => {
                    let DecodedValue::Dict(fields) = type_field("fields")?.value() else {
                        return Err("Struct.fields descriptor must be a Dict".into());
                    };
                    let (names, descriptors) =
                        view.dict_parts(fields).map_err(|error| error.to_string())?;
                    let names = names
                        .iter()
                        .map(|name| view.text(*name).map(str::to_owned))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?;
                    if names
                        != value_fields
                            .iter()
                            .map(|(name, _)| name.clone())
                            .collect::<Vec<_>>()
                    {
                        return Err(
                            "Struct descriptor and runtime value have different fields".into()
                        );
                    }
                    descriptors.to_vec()
                }
                "Dict" => vec![type_field("item")?; value_fields.len()],
                _ => {
                    return Err(format!(
                        "dyn.fields does not support descriptor kind {kind}"
                    ));
                }
            };
            let fields = value_fields
                .into_iter()
                .zip(descriptors)
                .map(|((name, value), descriptor)| (name, descriptor, value))
                .collect();
            Ok(DynObservation::NamedChildren(fields))
        }
        CoreDynFunction::ArrayItems => {
            if kind != "Array" {
                return Err(format!("dyn.array_items expected Array, got {kind}"));
            }
            let DecodedValue::Array(handle) = value.value() else {
                return Err("dyn.array_items expected runtime Array".into());
            };
            let item = type_field("item")?;
            let values = view
                .sequence(handle, false)
                .map_err(|error| error.to_string())?;
            Ok(DynObservation::Children(
                values.iter().map(|value| (item, *value)).collect(),
            ))
        }
        CoreDynFunction::TupleItems => {
            if kind != "Tuple" {
                return Err(format!("dyn.tuple_items expected Tuple, got {kind}"));
            }
            let DecodedValue::Tuple(handle) = value.value() else {
                return Err("dyn.tuple_items expected runtime Tuple".into());
            };
            let DecodedValue::Array(items) = type_field("items")?.value() else {
                return Err("Tuple.items descriptor must be an Array".into());
            };
            let descriptors = view
                .sequence(items, false)
                .map_err(|error| error.to_string())?;
            let values = view
                .sequence(handle, true)
                .map_err(|error| error.to_string())?;
            if descriptors.len() != values.len() {
                return Err("Tuple descriptor and runtime value have different lengths".into());
            }
            Ok(DynObservation::Children(
                descriptors
                    .iter()
                    .copied()
                    .zip(values.iter().copied())
                    .collect(),
            ))
        }
        CoreDynFunction::Tag => {
            let (tag, _) = dyn_tagged_parts(kind.as_str(), type_handle, value, view)?;
            Ok(DynObservation::Tag(tag))
        }
        CoreDynFunction::Payload => {
            let (_, payload) = dyn_tagged_parts(kind.as_str(), type_handle, value, view)?;
            Ok(DynObservation::Payload(payload))
        }
        _ => unreachable!("only structural operations reach observer"),
    }
}

fn normalize_dyn_descriptor(mut descriptor: Val, view: &HeapView<'_>) -> Result<Val, String> {
    loop {
        descriptor = declared_type_body(descriptor, view)?;
        if let DecodedValue::TypeSlot(handle) = descriptor.value() {
            descriptor = view
                .type_slot(handle)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Dyn descriptor reference is not initialized".to_owned())?;
            continue;
        }
        let DecodedValue::Dict(handle) = descriptor.value() else {
            return Err("Dyn descriptor is not canonical Type metadata".into());
        };
        let kind = view
            .dict_get_text(handle, "kind")
            .map_err(|error| error.to_string())?
            .and_then(|kind| view.atom_text(kind).ok().flatten())
            .ok_or_else(|| "Dyn descriptor is missing an Atom kind".to_owned())?;
        if kind != "WithAttributes" {
            return Ok(descriptor);
        }
        descriptor = view
            .dict_get_text(handle, "inner")
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "WithAttributes descriptor is missing inner".to_owned())?;
    }
}

fn dyn_tagged_parts(
    kind: &str,
    type_handle: Handle,
    value: Val,
    view: &HeapView<'_>,
) -> Result<(String, Option<(Val, Val)>), String> {
    let runtime = match value.value() {
        DecodedValue::BuiltinAtom(_) | DecodedValue::InlineAtom(_) | DecodedValue::Atom(_) => {
            let tag = view
                .atom_text(value)
                .map_err(|error| error.to_string())?
                .expect("Atom has text")
                .as_str()
                .to_owned();
            (tag, None)
        }
        DecodedValue::Tagged(handle) => {
            let (tag, payload) = view.tagged(handle).map_err(|error| error.to_string())?;
            let tag = view
                .atom_text(tag)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Tagged runtime tag is not an Atom".to_owned())?
                .as_str()
                .to_owned();
            (tag, Some(payload))
        }
        _ => {
            return Err(format!(
                "dyn tagged observer expected Atom or Tagged for {kind}"
            ));
        }
    };
    match kind {
        "Atom" => {
            let expected = view
                .dict_get_text(type_handle, "tag")
                .map_err(|error| error.to_string())?
                .and_then(|tag| view.atom_text(tag).ok().flatten())
                .ok_or_else(|| "Atom descriptor has no tag".to_owned())?;
            if runtime.0 != expected.as_str() || runtime.1.is_some() {
                return Err(format!("expected unit tag '{expected}"));
            }
            Ok((runtime.0, None))
        }
        "Tagged" => {
            let expected = view
                .dict_get_text(type_handle, "tag")
                .map_err(|error| error.to_string())?
                .and_then(|tag| view.atom_text(tag).ok().flatten())
                .ok_or_else(|| "Tagged descriptor has no tag".to_owned())?;
            if runtime.0 != expected.as_str() {
                return Err(format!("expected tag '{expected}"));
            }
            let payload = runtime
                .1
                .ok_or_else(|| format!("tag '{expected} requires a payload"))?;
            let payload_desc = view
                .dict_get_text(type_handle, "payload")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Tagged descriptor has no payload".to_owned())?;
            Ok((runtime.0, Some((payload_desc, payload))))
        }
        "Enum" => {
            let DecodedValue::Dict(variants) = view
                .dict_get_text(type_handle, "variants")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Enum descriptor has no variants".to_owned())?
                .value()
            else {
                return Err("Enum.variants descriptor must be a Dict".into());
            };
            let variant = view
                .dict_get_text(variants, &runtime.0)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("Enum has no variant {:?}", runtime.0))?;
            let (inner, _) = strip_runtime_attributes(variant, "Dyn.enum.variant", view)?;
            let unit = view
                .atom_text(inner)
                .ok()
                .flatten()
                .is_some_and(|atom| atom == "None");
            match (unit, runtime.1) {
                (true, None) => Ok((runtime.0, None)),
                (true, Some(_)) => Err(format!("unit variant {:?} has a payload", runtime.0)),
                (false, Some(payload)) => Ok((runtime.0, Some((variant, payload)))),
                (false, None) => Err(format!("variant {:?} requires a payload", runtime.0)),
            }
        }
        _ => Err(format!(
            "dyn tagged observer does not support descriptor kind {kind}"
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_dyn_observation(
    operation: CoreDynFunction,
    input: Val,
    observation: Result<DynObservation, String>,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let payload = match observation {
        Ok(observation) => {
            let units = match &observation {
                DynObservation::Child(_, _) => 3,
                DynObservation::Children(children) => 2 + children.len() * 3,
                DynObservation::NamedChildren(children) => {
                    2 + children
                        .iter()
                        .map(|(name, _, _)| 5 + name.len())
                        .sum::<usize>()
                }
                DynObservation::Tag(tag) => 2 + tag.len(),
                DynObservation::Payload(None) => 2,
                DynObservation::Payload(Some(_)) => 5,
            };
            charge_allocation(
                account,
                logical_value_bytes(units)
                    .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
                function,
                pc,
            )?;
            let value = match observation {
                DynObservation::Child(descriptor, value) => {
                    value.with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
                        identity: Arc::new(()),
                        descriptor,
                        value,
                        scheme: None,
                        origin: None,
                    })))
                }
                DynObservation::Children(children) => {
                    let children = children
                        .into_iter()
                        .map(|(descriptor, value)| {
                            value.with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
                                identity: Arc::new(()),
                                descriptor,
                                value,
                                scheme: None,
                                origin: None,
                            })))
                        })
                        .collect();
                    Val::new(
                        DecodedValue::Array(current.allocate(Object::Array(children))),
                        input.loc(),
                    )
                }
                DynObservation::NamedChildren(children) => {
                    let children = children
                        .into_iter()
                        .map(|(name, descriptor, value)| {
                            let name =
                                Val::new(current.string(Some(background), &name), input.loc());
                            let child = value.with_value(DecodedValue::Dyn(current.allocate(
                                Object::Dyn {
                                    identity: Arc::new(()),
                                    descriptor,
                                    value,
                                    scheme: None,
                                    origin: None,
                                },
                            )));
                            Val::new(
                                DecodedValue::Tuple(
                                    current.allocate(Object::Tuple(vec![name, child].into())),
                                ),
                                value.loc(),
                            )
                        })
                        .collect();
                    Val::new(
                        DecodedValue::Array(current.allocate(Object::Array(children))),
                        input.loc(),
                    )
                }
                DynObservation::Tag(tag) => {
                    Val::new(current.string(Some(background), &tag), input.loc())
                }
                DynObservation::Payload(None) => {
                    Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), input.loc())
                }
                DynObservation::Payload(Some((descriptor, value))) => {
                    let child =
                        value.with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
                            identity: Arc::new(()),
                            descriptor,
                            value,
                            scheme: None,
                            origin: None,
                        })));
                    Val::new(
                        DecodedValue::Tagged(current.allocate(Object::Tagged {
                            tag: Val::new(
                                DecodedValue::BuiltinAtom(BuiltinAtom::Some),
                                input.loc(),
                            ),
                            payload: child,
                        })),
                        input.loc(),
                    )
                }
            };
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Ok), input.loc()),
                    payload: value,
                })),
                input.loc(),
            )
        }
        Err(message) => {
            let rule = operation.name().trim_start_matches("std/");
            let bytes = logical_value_bytes(6)
                .and_then(|bytes| {
                    bytes
                        .checked_add(u64::try_from(message.len() + rule.len()).unwrap_or(u64::MAX))
                        .ok_or_else(|| {
                            NativeError::allocation_limit("Dyn observer error size overflowed")
                        })
                })
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
            charge_allocation(account, bytes, function, pc)?;
            let message = Val::new(current.string(Some(background), &message), input.loc());
            let rule = Val::new(current.string(Some(background), rule), input.loc());
            let fields = ["data", "message", "rule"]
                .into_iter()
                .map(|field| current.intern(field))
                .collect();
            let shape = current.intern_shape(fields);
            let blame = Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: vec![input, message, rule].into(),
                })),
                input.loc(),
            );
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Err), input.loc()),
                    payload: blame,
                })),
                input.loc(),
            )
        }
    };
    Ok(VmAction::Return {
        value: payload,
        return_target,
    })
}

fn dyn_descriptor_leaf_kind(mut descriptor: Val, view: &HeapView<'_>) -> Result<String, String> {
    loop {
        descriptor = declared_type_body(descriptor, view)?;
        if let DecodedValue::TypeSlot(handle) = descriptor.value() {
            descriptor = view
                .type_slot(handle)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Dyn descriptor reference is not initialized".to_owned())?;
            continue;
        }
        let DecodedValue::Dict(handle) = descriptor.value() else {
            return Err("Dyn descriptor is not canonical Type metadata".into());
        };
        let kind = view
            .dict_get_text(handle, "kind")
            .map_err(|error| error.to_string())?
            .and_then(|kind| view.atom_text(kind).ok().flatten())
            .ok_or_else(|| "Dyn descriptor is missing an Atom kind".to_owned())?;
        if kind != "WithAttributes" {
            return Ok(kind.as_str().to_owned());
        }
        descriptor = view
            .dict_get_text(handle, "inner")
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "WithAttributes descriptor is missing inner".to_owned())?;
    }
}

fn declared_type_body(value: Val, view: &HeapView<'_>) -> Result<Val, String> {
    let handle = match value.value() {
        DecodedValue::DeclaredType(handle) | DecodedValue::SymbolicType(handle) => handle,
        _ => return Ok(value),
    };
    let body = match view.object(handle).map_err(|error| error.to_string())? {
        Object::DeclaredType { body, .. } | Object::SymbolicType { body, .. } => body,
        _ => return Err("declared Type handle refers to another object kind".into()),
    };
    Ok(*body)
}

fn type_desc_children(input: Val, view: &HeapView<'_>) -> Result<Vec<Val>, String> {
    if matches!(
        input.value(),
        DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_)
    ) {
        return Ok(Vec::new());
    }
    if matches!(input.value(), DecodedValue::NativeType(_)) {
        return Ok(Vec::new());
    }
    if matches!(input.value(), DecodedValue::TypeSlot(_)) {
        return Ok(Vec::new());
    }
    let DecodedValue::Dict(handle) = input.value() else {
        return Err("std/type-desc.children expects Type metadata".into());
    };
    let kind = view
        .dict_get_text(handle, "kind")
        .map_err(|error| error.to_string())?
        .and_then(|value| view.atom_text(value).ok().flatten())
        .ok_or_else(|| "Type metadata is missing an Atom kind".to_owned())?;
    let get = |field: &str| {
        view.dict_get_text(handle, field)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("{kind} Type metadata is missing {field}"))
    };
    match kind.as_str() {
        "TypeOf" => Ok(vec![get("instance")?]),
        "Array" | "Dict" => Ok(vec![get("item")?]),
        "Tagged" => Ok(vec![get("payload")?]),
        "WithAttributes" => Ok(vec![get("inner")?]),
        "Tuple" | "Union" => {
            let field = if kind == "Tuple" { "items" } else { "variants" };
            let DecodedValue::Array(items) = get(field)?.value() else {
                return Err(format!("{kind}.{field} must be an Array"));
            };
            view.sequence(items, false)
                .map(|items| items.to_vec())
                .map_err(|error| error.to_string())
        }
        "Struct" => {
            let DecodedValue::Dict(fields) = get("fields")?.value() else {
                return Err("Struct.fields must be a Dict".into());
            };
            view.dict_parts(fields)
                .map(|(_, values)| values.to_vec())
                .map_err(|error| error.to_string())
        }
        "Enum" => {
            let DecodedValue::Dict(variants) = get("variants")?.value() else {
                return Err("Enum.variants must be a Dict".into());
            };
            let (_, values) = view
                .dict_parts(variants)
                .map_err(|error| error.to_string())?;
            values
                .iter()
                .filter_map(|value| {
                    let stripped = strip_runtime_attributes(*value, "Type.variants", view);
                    match stripped {
                        Ok((inner, _))
                            if view
                                .atom_text(inner)
                                .ok()
                                .flatten()
                                .is_some_and(|atom| atom == "None") =>
                        {
                            None
                        }
                        Ok((inner, _)) => Some(Ok(inner)),
                        Err(error) => Some(Err(error)),
                    }
                })
                .collect()
        }
        "Any" | "Never" | "Type" | "Dyn" | "Int" | "Float" | "String" | "Bytes" | "Opaque"
        | "Atom" | "Func" | "Bound" | "Named" => Ok(Vec::new()),
        other => Err(format!("unknown Type metadata kind '{other}")),
    }
}

#[allow(clippy::too_many_arguments)]
fn type_desc_resolve_error(
    input: Val,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let message = "type descriptor is not a recursive reference";
    let rule = "type-desc.resolve";
    let bytes = logical_value_bytes(6)
        .and_then(|bytes| {
            bytes
                .checked_add(u64::try_from(message.len() + rule.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| NativeError::allocation_limit("TypeDesc error size overflowed"))
        })
        .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
    charge_allocation(account, bytes, function, pc)?;
    let message = Val::new(current.string(Some(background), message), input.loc());
    let rule = Val::new(current.string(Some(background), rule), input.loc());
    let fields = ["data", "message", "rule"]
        .into_iter()
        .map(|field| current.intern(field))
        .collect();
    let shape = current.intern_shape(fields);
    let blame = Val::new(
        DecodedValue::Dict(current.allocate(Object::Dict {
            shape,
            values: vec![input, message, rule].into(),
        })),
        input.loc(),
    );
    Ok(VmAction::Return {
        value: Val::new(
            DecodedValue::Tagged(current.allocate(Object::Tagged {
                tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Err), input.loc()),
                payload: blame,
            })),
            input.loc(),
        ),
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_core_union_model(
    variants: Val,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let DecodedValue::Array(handle) = variants.value() else {
        let view = HeapView {
            current,
            background: Some(background),
        };
        return Err(runtime_type_error(
            "variants Array",
            &variants,
            &view,
            function,
            pc,
        ));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let variants = view
        .sequence(handle, false)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
        .to_vec();
    if variants.is_empty() {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            "union requires at least one variant",
            function,
            pc,
        ));
    }
    let mut normalized = Vec::with_capacity(variants.len());
    for (index, variant) in variants.into_iter().enumerate() {
        let path = format!("variants[{index}]");
        let (inner, attributes) =
            flatten_attributes(variant, &path, function, pc, current, background)?;
        if !matches!(inner.value(), DecodedValue::TypeSlot(_)) {
            decode_runtime_type_at(inner, &path, current, background)
                .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        }
        normalized.push(allocate_attributes_wrapper(
            inner,
            attributes,
            variant.loc().or(instruction_location(function, pc)),
            function,
            pc,
            current,
            account,
        )?);
    }
    charge_allocation(
        account,
        logical_value_bytes(normalized.len())
            .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
        function,
        pc,
    )?;
    let variants = Val::new(
        DecodedValue::Array(current.allocate(Object::Array(normalized.into()))),
        instruction_location(function, pc),
    );
    let metadata = allocate_core_dict(
        vec![
            (
                "kind".into(),
                Val::new(
                    DecodedValue::Atom(current.intern("Union")),
                    instruction_location(function, pc),
                ),
            ),
            ("variants".into(), variants),
        ],
        function,
        pc,
        current,
        account,
    )?;
    let value = allocate_attributes_wrapper(
        metadata,
        BTreeMap::new(),
        instruction_location(function, pc),
        function,
        pc,
        current,
        account,
    )?;
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_core_builtin_type(
    operation: CoreBuiltinTypeFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let variants = match operation {
        CoreBuiltinTypeFunction::FoldControl => vec![
            ("Break".to_owned(), Some(arguments[1])),
            ("Continue".to_owned(), Some(arguments[0])),
        ],
        CoreBuiltinTypeFunction::Option => vec![
            ("None".to_owned(), None),
            ("Some".to_owned(), Some(arguments[0])),
        ],
        CoreBuiltinTypeFunction::Result => vec![
            ("Err".to_owned(), Some(arguments[1])),
            ("Ok".to_owned(), Some(arguments[0])),
        ],
    };
    let value = allocate_builtin_enum(variants, function, pc, current, background, account)?;
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn allocate_builtin_enum(
    variants: Vec<(String, Option<Val>)>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<Val, RuntimeError> {
    let loc = instruction_location(function, pc);
    let mut normalized = Vec::with_capacity(variants.len());
    for (name, payload) in variants {
        let path = format!("variants.{name}");
        let (inner, attributes) = if let Some(payload) = payload {
            let (inner, attributes) =
                flatten_attributes(payload, &path, function, pc, current, background)?;
            if !matches!(inner.value(), DecodedValue::TypeSlot(_)) {
                decode_runtime_type_at(inner, &path, current, background).map_err(|message| {
                    error(RuntimeErrorKind::TypeMismatch, message, function, pc)
                })?;
            }
            (inner, attributes)
        } else {
            (
                Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), loc),
                BTreeMap::new(),
            )
        };
        let variant = allocate_attributes_wrapper(
            inner,
            attributes,
            inner.loc().or(loc),
            function,
            pc,
            current,
            account,
        )?;
        normalized.push((name, variant));
    }
    let variants = allocate_core_dict(normalized, function, pc, current, account)?;
    let metadata = allocate_core_dict(
        vec![
            (
                "kind".into(),
                Val::new(DecodedValue::Atom(current.intern("Enum")), loc),
            ),
            ("variants".into(), variants),
        ],
        function,
        pc,
        current,
        account,
    )?;
    allocate_attributes_wrapper(
        metadata,
        BTreeMap::new(),
        loc,
        function,
        pc,
        current,
        account,
    )
}

fn validate_model_context(
    context: Val,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<(), RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    if view
        .atom_text(context)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
        .is_some_and(|atom| atom == "None")
    {
        return Ok(());
    }
    let DecodedValue::Dict(handle) = context.value() else {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            "model context must be 'None or a Type context",
            function,
            pc,
        ));
    };
    let fields = view
        .dict_fields(handle)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
    let kind = view
        .dict_get_text(handle, "kind")
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
        .and_then(|value| view.atom_text(value).ok().flatten());
    let name = view
        .dict_get_text(handle, "name")
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
        .and_then(|value| view.string_text(value).ok().flatten());
    if fields == ["kind", "name"] && kind.is_some_and(|kind| kind == "Type") && name.is_some() {
        Ok(())
    } else {
        Err(error(
            RuntimeErrorKind::TypeMismatch,
            "model context must be 'None or {kind: 'Type, name: String}",
            function,
            pc,
        ))
    }
}

#[derive(Clone, Debug)]
struct CodecType {
    kind: CodecKind,
    rule: Val,
    attributes: BTreeMap<String, Val>,
    declared_owner: Option<Val>,
}

#[derive(Clone, Debug)]
enum CodecKind {
    TypeSlot(Handle),
    TypeRef(Handle),
    Any,
    Type,
    Dyn,
    Int,
    Float,
    String,
    Bytes,
    Opaque,
    Atom(String),
    Array(Box<CodecType>),
    Dict(Box<CodecType>),
    Tagged {
        tag: String,
        payload: Box<CodecType>,
    },
    Tuple(Vec<CodecType>),
    Struct(BTreeMap<String, CodecType>),
    Enum(BTreeMap<String, CodecEnumVariant>),
    Union(Vec<CodecType>),
    Function,
}

#[derive(Clone, Debug)]
struct CodecEnumVariant {
    payload: Option<Box<CodecType>>,
    attributes: BTreeMap<String, Val>,
    rule: Val,
}

#[derive(Clone, Debug)]
enum CodecNode {
    Existing(Val),
    SemanticValue {
        owner: Val,
        raw: Box<Self>,
    },
    Declared {
        owner: Val,
        payload: Box<Self>,
        loc: Option<crate::Loc>,
    },
    Atom(BuiltinAtom, Option<crate::Loc>),
    NamedAtom(String, Option<crate::Loc>),
    Array(Vec<Self>, Option<crate::Loc>),
    Tuple(Vec<Self>, Option<crate::Loc>),
    Tagged {
        tag: Box<Self>,
        payload: Box<Self>,
        loc: Option<crate::Loc>,
    },
    Dict(Vec<(String, Self)>, Option<crate::Loc>),
    String(String, Option<crate::Loc>),
}

#[derive(Clone, Copy)]
enum CodecDirection {
    Decode,
    Encode,
}

#[derive(Clone, Debug)]
struct CodecFailure {
    message: String,
    data: Val,
    rule: Val,
    predicate: Option<Box<PredicateRequest>>,
}

impl CodecFailure {
    fn new(message: impl Into<String>, data: Val, rule: Val) -> Self {
        Self {
            message: message.into(),
            data,
            rule,
            predicate: None,
        }
    }

    fn predicate(path: String, callee: Val, value: Val, rule: Val) -> Self {
        Self {
            message: "JSON skip predicate requires evaluation".into(),
            data: value,
            rule,
            predicate: Some(Box::new(PredicateRequest {
                path,
                callee,
                value,
            })),
        }
    }
}

#[derive(Clone, Debug)]
struct PredicateRequest {
    path: String,
    callee: Val,
    value: Val,
}

#[derive(Debug)]
struct JsonEncodeContinuation {
    input: JsonEncodeInput,
    value_owner: Val,
    decisions: BTreeMap<String, bool>,
    pending_path: String,
    pending_rule: Val,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    trace_frame: RuntimeFrame,
}

#[derive(Debug)]
enum JsonEncodeInput {
    Typed { schema: CodecType, value: Val },
    Dynamic(Val),
}

impl JsonEncodeInput {
    fn value(&self) -> Val {
        match self {
            Self::Typed { value, .. } | Self::Dynamic(value) => *value,
        }
    }
}

impl NativeContinuation for JsonEncodeContinuation {
    fn return_target(&self) -> &ReturnTarget {
        &self.return_target
    }

    fn trace_frame(&self) -> &RuntimeFrame {
        &self.trace_frame
    }

    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_json_encode_continuation(*self, value, current, background, account)
    }

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        _current: &mut Heap,
        _background: &Heap,
        _account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        Ok(VmAction::Return {
            value: failure,
            return_target: self.return_target,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn run_core_codec(
    operation: CoreCodecFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let direction = match operation {
        CoreCodecFunction::Decode => CodecDirection::Decode,
        CoreCodecFunction::Encode => CodecDirection::Encode,
    };
    if matches!(direction, CodecDirection::Encode) {
        let source_owner = {
            let view = HeapView {
                current,
                background: Some(background),
            };
            propagate_data_failures(&[arguments[1]], &view, function, pc)?;
            view.type_witness(arguments[1]).map_err(|heap_error| {
                error(
                    RuntimeErrorKind::TypeMismatch,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            })?
        };
        if source_owner.is_none() {
            return continue_json_encode(
                JsonEncodeInput::Dynamic(arguments[1]),
                arguments[0],
                BTreeMap::new(),
                arguments[1],
                return_target,
                Arc::new(function.clone()),
                pc,
                current,
                background,
                account,
            );
        }
    }
    let (schema_owner, value_owner) = {
        let view = HeapView {
            current,
            background: Some(background),
        };
        propagate_data_failures(&[arguments[1]], &view, function, pc)?;
        match direction {
            CodecDirection::Decode => {
                let owner = view
                    .type_witness(arguments[1])
                    .map_err(|heap_error| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            heap_error.to_string(),
                            function,
                            pc,
                        )
                    })?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "std/codec.decode expects std/value.Value input",
                            function,
                            pc,
                        )
                    })?;
                (arguments[0], owner)
            }
            CodecDirection::Encode => {
                let owner = view
                    .type_witness(arguments[1])
                    .map_err(|heap_error| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            heap_error.to_string(),
                            function,
                            pc,
                        )
                    })?
                    .expect("unowned encode inputs returned above");
                (owner, arguments[0])
            }
        }
    };
    let identity = {
        let view = HeapView {
            current,
            background: Some(background),
        };
        matches!(
            (
                view.declared_type_id(schema_owner),
                view.declared_type_id(value_owner),
            ),
            (Ok(schema), Ok(value)) if schema == value
        )
    };
    if identity {
        return finish_codec_result(
            Ok(CodecNode::Existing(arguments[1])),
            arguments[1],
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    let schema = decode_runtime_type(schema_owner, current, background)
        .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
    assert_codec_graph_ready(&schema, current, background).map_err(
        |graph_error| match graph_error {
            CodecGraphError::Pending => error(
                RuntimeErrorKind::UninitializedDefinition,
                "codec was invoked before recursive type metadata was sealed",
                function,
                pc,
            ),
            CodecGraphError::Invalid(message) => {
                error(RuntimeErrorKind::TypeMismatch, message, function, pc)
            }
        },
    )?;
    if matches!(direction, CodecDirection::Encode) {
        return continue_json_encode(
            JsonEncodeInput::Typed {
                schema,
                value: arguments[1],
            },
            value_owner,
            BTreeMap::new(),
            arguments[1],
            return_target,
            Arc::new(function.clone()),
            pc,
            current,
            background,
            account,
        );
    }
    let unwrap_bytes =
        semantic_value_unwrap_bytes(current, Some(background), arguments[1], value_owner).map_err(
            |heap_error| {
                error(
                    RuntimeErrorKind::TypeMismatch,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            },
        )?;
    charge_allocation(account, unwrap_bytes, function, pc)?;
    let raw = unwrap_semantic_value(current, Some(background), arguments[1], value_owner).map_err(
        |heap_error| {
            error(
                RuntimeErrorKind::TypeMismatch,
                heap_error.to_string(),
                function,
                pc,
            )
        },
    )?;
    let result = transform_codec(
        &schema,
        raw,
        direction,
        "$",
        &BTreeMap::new(),
        current,
        background,
    );
    finish_codec_result(
        result,
        arguments[1],
        return_target,
        function,
        pc,
        current,
        background,
        account,
    )
}

#[allow(clippy::too_many_arguments)]
fn continue_json_encode(
    input: JsonEncodeInput,
    value_owner: Val,
    decisions: BTreeMap<String, bool>,
    diagnostic_input: Val,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let result = match &input {
        JsonEncodeInput::Typed { schema, value } => transform_codec(
            schema,
            *value,
            CodecDirection::Encode,
            "$",
            &decisions,
            current,
            background,
        ),
        JsonEncodeInput::Dynamic(value) => transform_dynamic_encode(
            *value,
            "$",
            &decisions,
            current,
            background,
            &mut HashSet::new(),
        )
        .map(|(node, _)| node),
    };
    if let Err(failure) = &result
        && let Some(request) = &failure.predicate
    {
        let continuation = JsonEncodeContinuation {
            input,
            value_owner,
            decisions,
            pending_path: request.path.clone(),
            pending_rule: failure.rule,
            return_target,
            trace_frame: RuntimeFrame {
                function: "std/codec.encode".into(),
                instruction: 0,
                origin: call_function.origin_at(call_pc),
            },
            call_function: Arc::clone(&call_function),
            call_pc,
        };
        return Ok(VmAction::Call {
            callee: request.callee,
            arguments: vec![request.value],
            return_target: ReturnTarget::Native(Box::new(continuation)),
            call_function,
            call_pc,
        });
    }
    let result = result.map(|raw| CodecNode::SemanticValue {
        owner: value_owner,
        raw: Box::new(raw),
    });
    finish_codec_result(
        result,
        diagnostic_input,
        return_target,
        &call_function,
        call_pc,
        current,
        background,
        account,
    )
}

fn resume_json_encode_continuation(
    mut continuation: JsonEncodeContinuation,
    value: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let decision = match value.value() {
        DecodedValue::BuiltinAtom(BuiltinAtom::True) => true,
        DecodedValue::BuiltinAtom(BuiltinAtom::False) => false,
        _ => {
            let mut runtime = error(
                RuntimeErrorKind::TypeMismatch,
                "std/json.skip_serializing_if predicate must return 'True or 'False",
                &continuation.call_function,
                continuation.call_pc,
            );
            runtime.set_locations(value.loc(), continuation.pending_rule.loc());
            return Err(runtime);
        }
    };
    continuation
        .decisions
        .insert(continuation.pending_path.clone(), decision);
    let diagnostic_input = continuation.input.value();
    continue_json_encode(
        continuation.input,
        continuation.value_owner,
        continuation.decisions,
        diagnostic_input,
        continuation.return_target,
        continuation.call_function,
        continuation.call_pc,
        current,
        background,
        account,
    )
}

fn transform_dynamic_encode(
    value: Val,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
    active: &mut HashSet<Handle>,
) -> Result<(CodecNode, bool), CodecFailure> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    if let Some(owner) = view
        .type_witness(value)
        .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
    {
        let schema = decode_runtime_type(owner, current, background)
            .map_err(|message| CodecFailure::new(message, value, owner))?;
        assert_codec_graph_ready(&schema, current, background).map_err(|error| {
            let message = match error {
                CodecGraphError::Pending => {
                    "codec was invoked before recursive type metadata was sealed".into()
                }
                CodecGraphError::Invalid(message) => message,
            };
            CodecFailure::new(message, value, owner)
        })?;
        return transform_codec(
            &schema,
            value,
            CodecDirection::Encode,
            path,
            predicate_decisions,
            current,
            background,
        )
        .map(|node| (node, true));
    }

    let result = match value.value() {
        DecodedValue::BuiltinAtom(BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False)
        | DecodedValue::Int(_)
        | DecodedValue::InlineString(_)
        | DecodedValue::ShortString(_)
        | DecodedValue::Bytes(_) => Ok((CodecNode::Existing(value), false)),
        DecodedValue::Float(number) if number.is_finite() => {
            Ok((CodecNode::Existing(value), false))
        }
        DecodedValue::Float(_) => Err(CodecFailure::new(
            "semantic Value cannot contain a non-finite Float",
            value,
            value,
        )),
        DecodedValue::Array(handle) => {
            if !active.insert(handle) {
                return Err(CodecFailure::new(
                    "semantic Value cannot contain a cycle",
                    value,
                    value,
                ));
            }
            let result = view
                .sequence(handle, false)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    transform_dynamic_encode(
                        *item,
                        &format!("{path}[{index}]"),
                        predicate_decisions,
                        current,
                        background,
                        active,
                    )
                })
                .collect::<Result<Vec<_>, _>>();
            active.remove(&handle);
            result.map(|items| {
                if items.iter().any(|(_, transformed)| *transformed) {
                    (
                        CodecNode::Array(
                            items.into_iter().map(|(item, _)| item).collect(),
                            value.loc(),
                        ),
                        true,
                    )
                } else {
                    (CodecNode::Existing(value), false)
                }
            })
        }
        DecodedValue::Dict(handle) => {
            if !active.insert(handle) {
                return Err(CodecFailure::new(
                    "semantic Value cannot contain a cycle",
                    value,
                    value,
                ));
            }
            let (fields, values) = view
                .dict_parts(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?;
            let result = fields
                .iter()
                .zip(values)
                .map(|(field, item)| {
                    let name = view
                        .text(*field)
                        .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                        .to_owned();
                    transform_dynamic_encode(
                        *item,
                        &format!("{path}.{name}"),
                        predicate_decisions,
                        current,
                        background,
                        active,
                    )
                    .map(|(item, transformed)| (name, item, transformed))
                })
                .collect::<Result<Vec<_>, _>>();
            active.remove(&handle);
            result.map(|fields| {
                if fields.iter().any(|(_, _, transformed)| *transformed) {
                    (
                        CodecNode::Dict(
                            fields
                                .into_iter()
                                .map(|(name, item, _)| (name, item))
                                .collect(),
                            value.loc(),
                        ),
                        true,
                    )
                } else {
                    (CodecNode::Existing(value), false)
                }
            })
        }
        DecodedValue::Tagged(handle) => {
            let (tag, payload) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?;
            if tag.value() == DecodedValue::BuiltinAtom(BuiltinAtom::Some) {
                return transform_dynamic_encode(
                    payload,
                    path,
                    predicate_decisions,
                    current,
                    background,
                    active,
                )
                .map(|(node, _)| (node, true));
            }
            let temporal = view
                .atom_text(tag)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                .is_some_and(|tag| {
                    matches!(
                        tag.as_str(),
                        "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
                    )
                })
                && view
                    .string_text(payload)
                    .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                    .is_some();
            if temporal {
                Ok((CodecNode::Existing(value), false))
            } else {
                Err(CodecFailure::new(
                    "raw data graph contains unsupported tagged value",
                    value,
                    value,
                ))
            }
        }
        DecodedValue::NativeType(_)
        | DecodedValue::DeclaredType(_)
        | DecodedValue::SymbolicType(_)
        | DecodedValue::TypeSlot(_) => Err(CodecFailure::new(
            "semantic Value cannot encode Type",
            value,
            value,
        )),
        _ => Err(CodecFailure::new(
            format!("raw data graph contains unsupported {:?}", value.value()),
            value,
            value,
        )),
    };
    result
}

#[allow(clippy::too_many_arguments)]
fn finish_codec_result(
    result: Result<CodecNode, CodecFailure>,
    input: Val,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let (tag, payload) = match result {
        Ok(node) => (BuiltinAtom::Ok, node),
        Err(failure) => {
            let loc = failure.data.loc();
            (
                BuiltinAtom::Err,
                CodecNode::Dict(
                    vec![
                        ("message".into(), CodecNode::String(failure.message, loc)),
                        ("data".into(), CodecNode::Existing(failure.data)),
                        ("rule".into(), CodecNode::Existing(failure.rule)),
                    ],
                    loc,
                ),
            )
        }
    };
    let bytes = codec_node_bytes(&payload, current, background)
        .and_then(|bytes| {
            bytes
                .checked_add(logical_value_bytes(2)?)
                .ok_or_else(|| NativeError::allocation_limit("codec Result size overflowed"))
        })
        .map_err(|native_error| match native_error.limit() {
            Some(_) => allocation_error(native_error.message, function, pc),
            None => error(
                RuntimeErrorKind::TypeMismatch,
                native_error.message,
                function,
                pc,
            ),
        })?;
    charge_allocation(account, bytes, function, pc)?;
    let payload = materialize_codec_node(payload, current, background);
    let value = Val::new(
        DecodedValue::Tagged(current.allocate(Object::Tagged {
            tag: Val::new(DecodedValue::BuiltinAtom(tag), input.loc()),
            payload,
        })),
        input.loc(),
    );
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

fn decode_runtime_type(value: Val, current: &Heap, background: &Heap) -> Result<CodecType, String> {
    decode_runtime_type_at(value, "Type", current, background)
}

#[derive(Debug)]
enum CodecGraphError {
    Pending,
    Invalid(String),
}

fn assert_codec_graph_ready(
    schema: &CodecType,
    current: &Heap,
    background: &Heap,
) -> Result<(), CodecGraphError> {
    fn visit(
        schema: &CodecType,
        current: &Heap,
        background: &Heap,
        visited: &mut HashSet<Handle>,
    ) -> Result<(), CodecGraphError> {
        match &schema.kind {
            CodecKind::TypeSlot(handle) => {
                if !visited.insert(*handle) {
                    return Ok(());
                }
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                let resolved = view
                    .type_slot(*handle)
                    .map_err(|error| CodecGraphError::Invalid(error.to_string()))?
                    .ok_or(CodecGraphError::Pending)?;
                let resolved = decode_runtime_type(resolved, current, background)
                    .map_err(CodecGraphError::Invalid)?;
                visit(&resolved, current, background, visited)
            }
            CodecKind::TypeRef(handle) => {
                if !visited.insert(*handle) {
                    return Ok(());
                }
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                let body = match view
                    .object(*handle)
                    .map_err(|error| CodecGraphError::Invalid(error.to_string()))?
                {
                    Object::DeclaredType { body, .. } | Object::SymbolicType { body, .. } => body,
                    _ => return Err(CodecGraphError::Pending),
                };
                let resolved = decode_runtime_type(*body, current, background)
                    .map_err(CodecGraphError::Invalid)?;
                visit(&resolved, current, background, visited)
            }
            CodecKind::Array(item) | CodecKind::Dict(item) => {
                visit(item, current, background, visited)
            }
            CodecKind::Tagged { payload, .. } => visit(payload, current, background, visited),
            CodecKind::Tuple(items) | CodecKind::Union(items) => items
                .iter()
                .try_for_each(|item| visit(item, current, background, visited)),
            CodecKind::Struct(fields) => fields
                .values()
                .try_for_each(|field| visit(field, current, background, visited)),
            CodecKind::Enum(variants) => variants.values().try_for_each(|variant| {
                if let Some(payload) = &variant.payload {
                    visit(payload, current, background, visited)?;
                }
                Ok(())
            }),
            CodecKind::Any
            | CodecKind::Type
            | CodecKind::Dyn
            | CodecKind::Int
            | CodecKind::Float
            | CodecKind::String
            | CodecKind::Bytes
            | CodecKind::Opaque
            | CodecKind::Atom(_)
            | CodecKind::Function => Ok(()),
        }
    }

    visit(schema, current, background, &mut HashSet::new())
}

fn decode_runtime_type_at(
    value: Val,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<CodecType, String> {
    if matches!(value.value(), DecodedValue::NativeType(_)) {
        return Ok(CodecType {
            kind: CodecKind::Opaque,
            rule: value,
            attributes: BTreeMap::new(),
            declared_owner: None,
        });
    }
    if let DecodedValue::TypeSlot(handle) = value.value() {
        return Ok(CodecType {
            kind: CodecKind::TypeSlot(handle),
            rule: value,
            attributes: BTreeMap::new(),
            declared_owner: None,
        });
    }
    let view = HeapView {
        current,
        background: Some(background),
    };
    if let DecodedValue::DeclaredType(handle) | DecodedValue::SymbolicType(handle) = value.value() {
        return Ok(CodecType {
            kind: CodecKind::TypeRef(handle),
            rule: value,
            attributes: BTreeMap::new(),
            declared_owner: None,
        });
    }
    let DecodedValue::Dict(handle) = value.value() else {
        return Err(format!("{path} must be Type metadata"));
    };
    let kind = view
        .dict_get_text(handle, "kind")
        .map_err(|error| error.to_string())?
        .and_then(|kind| view.atom_text(kind).ok().flatten())
        .ok_or_else(|| format!("{path}.kind must be an Atom"))?;
    if kind == "WithAttributes" {
        let fields = view
            .dict_fields(handle)
            .map_err(|error| error.to_string())?;
        if fields != ["attributes", "inner", "kind"] {
            return Err(format!(
                "{path} WithAttributes wrapper must have exactly attributes, inner, and kind fields"
            ));
        }
        let attributes = view
            .dict_get_text(handle, "attributes")
            .map_err(|error| error.to_string())?
            .expect("validated wrapper field");
        let DecodedValue::Dict(attribute_handle) = attributes.value() else {
            return Err(format!("{path}.attributes must be a Dict"));
        };
        let inner = view
            .dict_get_text(handle, "inner")
            .map_err(|error| error.to_string())?
            .expect("validated wrapper field");
        let mut decoded = decode_runtime_type_at(inner, path, current, background)?;
        let (names, values) = view
            .dict_parts(attribute_handle)
            .map_err(|error| error.to_string())?;
        for (name, attribute) in names.iter().zip(values) {
            decoded.attributes.insert(
                view.text(*name)
                    .map_err(|error| error.to_string())?
                    .to_owned(),
                *attribute,
            );
        }
        if !decoded.attributes.is_empty() || decoded.rule.loc().is_none() {
            decoded.rule = value;
        }
        return Ok(decoded);
    }
    let kind = match kind.as_str() {
        "Bound" => CodecKind::Any,
        "Named" => CodecKind::Any,
        "Any" => CodecKind::Any,
        "Type" => CodecKind::Type,
        "Dyn" => CodecKind::Dyn,
        "Int" => CodecKind::Int,
        "Float" => CodecKind::Float,
        "String" => CodecKind::String,
        "Bytes" => CodecKind::Bytes,
        "Opaque" => return Err(format!("{path} uses an unsupported opaque type")),
        "Atom" => {
            let tag = view
                .dict_get_text(handle, "tag")
                .map_err(|error| error.to_string())?
                .and_then(|tag| view.atom_text(tag).ok().flatten())
                .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
            CodecKind::Atom(tag.as_str().to_owned())
        }
        "Array" => {
            let item = view
                .dict_get_text(handle, "item")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.item is missing"))?;
            CodecKind::Array(Box::new(decode_runtime_type_at(
                item,
                &format!("{path}.item"),
                current,
                background,
            )?))
        }
        "Dict" => {
            let item = view
                .dict_get_text(handle, "item")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.item is missing"))?;
            CodecKind::Dict(Box::new(decode_runtime_type_at(
                item,
                &format!("{path}.item"),
                current,
                background,
            )?))
        }
        "Tagged" => {
            let tag = view
                .dict_get_text(handle, "tag")
                .map_err(|error| error.to_string())?
                .and_then(|tag| view.atom_text(tag).ok().flatten())
                .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
            let payload = view
                .dict_get_text(handle, "payload")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.payload is missing"))?;
            CodecKind::Tagged {
                tag: tag.as_str().to_owned(),
                payload: Box::new(decode_runtime_type_at(
                    payload,
                    &format!("{path}.payload"),
                    current,
                    background,
                )?),
            }
        }
        "Tuple" | "Union" => {
            let field = if kind == "Tuple" { "items" } else { "variants" };
            let items = view
                .dict_get_text(handle, field)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.{field} is missing"))?;
            let DecodedValue::Array(items) = items.value() else {
                return Err(format!("{path}.{field} must be an Array"));
            };
            let items = view
                .sequence(items, false)
                .map_err(|error| error.to_string())?
                .to_vec();
            let decoded = items
                .into_iter()
                .enumerate()
                .map(|(index, item)| {
                    decode_runtime_type_at(
                        item,
                        &format!("{path}.{field}[{index}]"),
                        current,
                        background,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            if kind == "Tuple" {
                CodecKind::Tuple(decoded)
            } else {
                CodecKind::Union(decoded)
            }
        }
        "Struct" => {
            let fields = view
                .dict_get_text(handle, "fields")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.fields is missing"))?;
            let DecodedValue::Dict(fields) = fields.value() else {
                return Err(format!("{path}.fields must be a Dict"));
            };
            let (names, values) = view.dict_parts(fields).map_err(|error| error.to_string())?;
            let entries = names
                .iter()
                .zip(values)
                .map(|(name, value)| Ok((view.text(*name)?.to_owned(), *value)))
                .collect::<Result<Vec<_>, crate::heap::HeapError>>()
                .map_err(|error| error.to_string())?;
            CodecKind::Struct(
                entries
                    .into_iter()
                    .map(|(name, value)| {
                        let field = decode_runtime_type_at(
                            value,
                            &format!("{path}.fields.{name}"),
                            current,
                            background,
                        )?;
                        Ok((name, field))
                    })
                    .collect::<Result<_, String>>()?,
            )
        }
        "Enum" => {
            let variants = view
                .dict_get_text(handle, "variants")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.variants is missing"))?;
            let DecodedValue::Dict(variants) = variants.value() else {
                return Err(format!("{path}.variants must be a Dict"));
            };
            let (names, values) = view
                .dict_parts(variants)
                .map_err(|error| error.to_string())?;
            if names.is_empty() {
                return Err(format!("{path}.variants must not be empty"));
            }
            let mut decoded = BTreeMap::new();
            for (name, variant) in names.iter().zip(values) {
                let name = view.text(*name).map_err(|error| error.to_string())?;
                let variant_path = format!("{path}.variants.{name}");
                let (inner, attributes) = strip_runtime_attributes(*variant, &variant_path, &view)?;
                let payload = if view
                    .atom_text(inner)
                    .map_err(|error| error.to_string())?
                    .is_some_and(|atom| atom == "None")
                {
                    None
                } else {
                    Some(Box::new(decode_runtime_type_at(
                        inner,
                        &variant_path,
                        current,
                        background,
                    )?))
                };
                decoded.insert(
                    name.to_owned(),
                    CodecEnumVariant {
                        payload,
                        attributes,
                        rule: *variant,
                    },
                );
            }
            CodecKind::Enum(decoded)
        }
        "Func" => CodecKind::Function,
        other => return Err(format!("{path}.kind has unsupported value '{other}")),
    };
    Ok(CodecType {
        kind,
        rule: value,
        attributes: BTreeMap::new(),
        declared_owner: None,
    })
}

fn strip_runtime_attributes(
    mut value: Val,
    path: &str,
    view: &HeapView<'_>,
) -> Result<(Val, BTreeMap<String, Val>), String> {
    let mut collected = BTreeMap::new();
    while let DecodedValue::Dict(handle) = value.value() {
        let kind = view
            .dict_get_text(handle, "kind")
            .map_err(|error| error.to_string())?
            .and_then(|kind| view.atom_text(kind).ok().flatten());
        if !kind.is_some_and(|kind| kind == "WithAttributes") {
            break;
        }
        let fields = view
            .dict_fields(handle)
            .map_err(|error| error.to_string())?;
        if fields != ["attributes", "inner", "kind"] {
            return Err(format!(
                "{path} WithAttributes wrapper must have exactly attributes, inner, and kind fields"
            ));
        }
        let attributes = view
            .dict_get_text(handle, "attributes")
            .map_err(|error| error.to_string())?
            .expect("validated wrapper field");
        let DecodedValue::Dict(attributes) = attributes.value() else {
            return Err(format!("{path}.attributes must be a Dict"));
        };
        let (names, values) = view
            .dict_parts(attributes)
            .map_err(|error| error.to_string())?;
        for (name, attribute) in names.iter().zip(values) {
            collected
                .entry(
                    view.text(*name)
                        .map_err(|error| error.to_string())?
                        .to_owned(),
                )
                .or_insert(*attribute);
        }
        value = view
            .dict_get_text(handle, "inner")
            .map_err(|error| error.to_string())?
            .expect("validated wrapper field");
    }
    Ok((value, collected))
}

fn option_item(schema: &CodecType) -> Option<&CodecType> {
    if let CodecKind::Enum(variants) = &schema.kind {
        if variants.len() == 2
            && variants
                .get("None")
                .is_some_and(|variant| variant.payload.is_none())
        {
            return variants
                .get("Some")
                .and_then(|variant| variant.payload.as_ref())
                .map(Box::as_ref);
        }
        return None;
    }
    let CodecKind::Union(variants) = &schema.kind else {
        return None;
    };
    if variants.len() != 2 {
        return None;
    }
    let none = variants
        .iter()
        .any(|variant| matches!(&variant.kind, CodecKind::Atom(tag) if tag == "None"));
    let some = variants.iter().find_map(|variant| {
        let CodecKind::Tagged { tag, payload } = &variant.kind else {
            return None;
        };
        (tag == "Some").then_some(payload.as_ref())
    });
    none.then_some(some).flatten()
}

fn is_bool_enum(variants: &BTreeMap<String, CodecEnumVariant>) -> bool {
    variants.len() == 2
        && variants
            .get("False")
            .is_some_and(|variant| variant.payload.is_none())
        && variants
            .get("True")
            .is_some_and(|variant| variant.payload.is_none())
}

fn text_codec_bridge(schema: &CodecType, view: &HeapView<'_>) -> Result<bool, String> {
    let decode = schema.attributes.get("std/string.decode_by_parse");
    let encode = schema.attributes.get("std/string.encode_by_display");
    if decode.is_none() && encode.is_none() {
        return Ok(false);
    }
    if decode.is_none() || encode.is_none() {
        return Err(
            "std/string.decode_by_parse and std/string.encode_by_display must be used together"
                .into(),
        );
    }
    for (name, marker) in [
        ("std/string.decode_by_parse", decode.expect("checked")),
        ("std/string.encode_by_display", encode.expect("checked")),
    ] {
        if !view
            .atom_text(*marker)
            .map_err(|error| error.to_string())?
            .is_some_and(|atom| atom == "True")
        {
            return Err(format!("{name} must be 'True"));
        }
    }
    Ok(true)
}

fn parsed_codec_node(value: crate::regex::ParsedValue, loc: Option<crate::Loc>) -> CodecNode {
    match value {
        crate::regex::ParsedValue::String(value) => CodecNode::String(value, loc),
        crate::regex::ParsedValue::Int(value) => {
            CodecNode::Existing(Val::new(DecodedValue::Int(value), loc))
        }
        crate::regex::ParsedValue::Float(value) => {
            CodecNode::Existing(Val::new(DecodedValue::Float(value), loc))
        }
        crate::regex::ParsedValue::None => CodecNode::Atom(BuiltinAtom::None, loc),
        crate::regex::ParsedValue::Some(value) => CodecNode::Tagged {
            tag: Box::new(CodecNode::Atom(BuiltinAtom::Some, loc)),
            payload: Box::new(parsed_codec_node(*value, loc)),
            loc,
        },
        crate::regex::ParsedValue::Struct(fields) => CodecNode::Dict(
            fields
                .into_iter()
                .map(|(name, value)| (name, parsed_codec_node(value, loc)))
                .collect(),
            loc,
        ),
    }
}

fn transform_codec(
    schema: &CodecType,
    value: Val,
    direction: CodecDirection,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    if let Some(owner) = schema.declared_owner {
        let mut structural = schema.clone();
        structural.declared_owner = None;
        return match direction {
            CodecDirection::Decode => transform_codec(
                &structural,
                value,
                direction,
                path,
                predicate_decisions,
                current,
                background,
            )
            .map(|payload| CodecNode::Declared {
                owner,
                payload: Box::new(payload),
                loc: value.loc(),
            }),
            CodecDirection::Encode => {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                let Some(actual_owner) = view
                    .type_witness(value)
                    .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                else {
                    return Err(CodecFailure::new(
                        format!("{path}: expected a declared value"),
                        value,
                        schema.rule,
                    ));
                };
                let same_owner = view
                    .values_equal(actual_owner, owner)
                    .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
                if !same_owner {
                    return Err(CodecFailure::new(
                        format!("{path}: declared type identity does not match codec"),
                        value,
                        schema.rule,
                    ));
                }
                transform_codec(
                    &structural,
                    value.without_type_id(),
                    direction,
                    path,
                    predicate_decisions,
                    current,
                    background,
                )
            }
        };
    }
    if option_item(schema).is_some() {
        return transform_codec_field(
            schema,
            value,
            direction,
            path,
            predicate_decisions,
            current,
            background,
        );
    }
    let view = HeapView {
        current,
        background: Some(background),
    };
    if !matches!(schema.kind, CodecKind::TypeSlot(_) | CodecKind::TypeRef(_)) {
        let bridged = text_codec_bridge(schema, &view).map_err(|message| {
            CodecFailure::new(format!("{path}: {message}"), value, schema.rule)
        })?;
        if bridged {
            let metadata = ValueRef {
                value: schema.rule,
                view,
            };
            return match direction {
                CodecDirection::Decode => {
                    let source = view
                        .string_text(value)
                        .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                        .ok_or_else(|| {
                            CodecFailure::new(
                                format!("{path}: expected String text representation"),
                                value,
                                schema.rule,
                            )
                        })?;
                    crate::regex::parse_value(metadata, source.as_str())
                        .map(|parsed| parsed_codec_node(parsed, value.loc()))
                        .map_err(|message| {
                            CodecFailure::new(format!("{path}: {message}"), value, schema.rule)
                        })
                }
                CodecDirection::Encode => {
                    crate::fmt::display_value(metadata, ValueRef { value, view })
                        .map(|text| CodecNode::String(text, value.loc()))
                        .map_err(|error| {
                            CodecFailure::new(
                                format!("{path}: {}", error.message),
                                value,
                                schema.rule,
                            )
                        })
                }
            };
        }
    }
    match &schema.kind {
        CodecKind::TypeSlot(handle) => {
            let resolved = view
                .type_slot(*handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .ok_or_else(|| {
                    CodecFailure::new("recursive type link is not initialized", value, schema.rule)
                })?;
            let resolved = decode_runtime_type(resolved, current, background)
                .map_err(|message| CodecFailure::new(message, value, schema.rule))?;
            transform_codec(
                &resolved,
                value,
                direction,
                path,
                predicate_decisions,
                current,
                background,
            )
        }
        CodecKind::TypeRef(handle) => {
            let Object::DeclaredType { body, .. } = view
                .object(*handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
            else {
                return Err(CodecFailure::new(
                    "type ref is not sealed",
                    value,
                    schema.rule,
                ));
            };
            let mut resolved = decode_runtime_type(*body, current, background)
                .map_err(|message| CodecFailure::new(message, value, schema.rule))?;
            resolved.declared_owner = Some(Val::unknown(DecodedValue::DeclaredType(*handle)));
            transform_codec(
                &resolved,
                value,
                direction,
                path,
                predicate_decisions,
                current,
                background,
            )
        }
        CodecKind::Any => Ok(CodecNode::Existing(value)),
        CodecKind::Type => decode_runtime_type(value, current, background)
            .map(|_| CodecNode::Existing(value))
            .map_err(|message| CodecFailure::new(message, value, schema.rule)),
        CodecKind::Dyn if matches!(value.value(), DecodedValue::Dyn(_)) => {
            Ok(CodecNode::Existing(value))
        }
        CodecKind::Int if matches!(value.value(), DecodedValue::Int(_)) => {
            Ok(CodecNode::Existing(value))
        }
        CodecKind::Float if matches!(value.value(), DecodedValue::Float(_)) => {
            Ok(CodecNode::Existing(value))
        }
        CodecKind::String
            if view
                .string_text(value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .is_some() =>
        {
            Ok(CodecNode::Existing(value))
        }
        CodecKind::Atom(expected) => {
            let actual = view
                .atom_text(value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            if actual.is_some_and(|actual| actual.as_str() == expected) {
                Ok(CodecNode::Existing(value))
            } else {
                Err(CodecFailure::new(
                    format!("{path}: expected '{expected}"),
                    value,
                    schema.rule,
                ))
            }
        }
        CodecKind::Array(item) => {
            let DecodedValue::Array(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected Array"),
                    value,
                    schema.rule,
                ));
            };
            let values = view
                .sequence(handle, false)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .to_vec();
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    transform_codec(
                        item,
                        value,
                        direction,
                        &format!("{path}[{index}]"),
                        predicate_decisions,
                        current,
                        background,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|items| CodecNode::Array(items, value.loc()))
        }
        CodecKind::Dict(item) => {
            let DecodedValue::Dict(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected Dict"),
                    value,
                    schema.rule,
                ));
            };
            let (names, values) = view
                .dict_parts(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            names
                .iter()
                .zip(values)
                .map(|(name, item_value)| {
                    let name = view
                        .text(*name)
                        .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                        .to_owned();
                    let node = transform_codec(
                        item,
                        *item_value,
                        direction,
                        &format!("{path}.{name}"),
                        predicate_decisions,
                        current,
                        background,
                    )?;
                    Ok((name, node))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|fields| CodecNode::Dict(fields, value.loc()))
        }
        CodecKind::Tagged { tag, payload } => {
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected '{tag}(payload)"),
                    value,
                    schema.rule,
                ));
            };
            let (actual_tag, actual_payload) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            if view
                .atom_text(actual_tag)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .is_none_or(|actual| actual.as_str() != tag)
            {
                return Err(CodecFailure::new(
                    format!("{path}: expected tag '{tag}"),
                    value,
                    schema.rule,
                ));
            }
            Ok(CodecNode::Tagged {
                tag: Box::new(CodecNode::NamedAtom(tag.clone(), value.loc())),
                payload: Box::new(transform_codec(
                    payload,
                    actual_payload,
                    direction,
                    path,
                    predicate_decisions,
                    current,
                    background,
                )?),
                loc: value.loc(),
            })
        }
        CodecKind::Tuple(items) => {
            let (handle, input_is_tuple) = match (direction, value.value()) {
                (CodecDirection::Decode, DecodedValue::Array(handle)) => (handle, false),
                (CodecDirection::Encode, DecodedValue::Tuple(handle)) => (handle, true),
                (CodecDirection::Decode, _) => {
                    return Err(CodecFailure::new(
                        format!("{path}: expected Array"),
                        value,
                        schema.rule,
                    ));
                }
                (CodecDirection::Encode, _) => {
                    return Err(CodecFailure::new(
                        format!("{path}: expected Tuple"),
                        value,
                        schema.rule,
                    ));
                }
            };
            let values = view
                .sequence(handle, input_is_tuple)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .to_vec();
            if values.len() != items.len() {
                return Err(CodecFailure::new(
                    format!("{path}: expected {} items", items.len()),
                    value,
                    schema.rule,
                ));
            }
            let nodes = items
                .iter()
                .zip(values)
                .enumerate()
                .map(|(index, (item, value))| {
                    transform_codec(
                        item,
                        value,
                        direction,
                        &format!("{path}[{index}]"),
                        predicate_decisions,
                        current,
                        background,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(match direction {
                CodecDirection::Decode => CodecNode::Tuple(nodes, value.loc()),
                CodecDirection::Encode => CodecNode::Array(nodes, value.loc()),
            })
        }
        CodecKind::Struct(fields) => transform_codec_struct(
            schema,
            fields,
            value,
            direction,
            path,
            predicate_decisions,
            current,
            background,
        ),
        CodecKind::Union(variants) => {
            let mut errors = Vec::new();
            for variant in variants {
                match transform_codec(
                    variant,
                    value,
                    direction,
                    path,
                    predicate_decisions,
                    current,
                    background,
                ) {
                    Ok(node) => return Ok(node),
                    Err(failure) if failure.predicate.is_some() => return Err(failure),
                    Err(failure) => errors.push(failure.message),
                }
            }
            Err(CodecFailure::new(
                format!(
                    "{path}: value matches no Union variant ({})",
                    errors.join("; ")
                ),
                value,
                schema.rule,
            ))
        }
        CodecKind::Enum(variants) if is_bool_enum(variants) => {
            if matches!(
                value.value(),
                DecodedValue::BuiltinAtom(BuiltinAtom::True | BuiltinAtom::False)
            ) {
                Ok(CodecNode::Existing(value))
            } else {
                Err(CodecFailure::new(
                    format!("{path}: expected Bool"),
                    value,
                    schema.rule,
                ))
            }
        }
        CodecKind::Enum(variants) => transform_codec_enum(
            schema,
            variants,
            value,
            direction,
            path,
            predicate_decisions,
            current,
            background,
        ),
        CodecKind::Bytes => Err(CodecFailure::new(
            format!("{path}: Bytes has no JSON codec"),
            value,
            schema.rule,
        )),
        CodecKind::Opaque => Err(CodecFailure::new(
            format!("{path}: Opaque has no JSON codec"),
            value,
            schema.rule,
        )),
        CodecKind::Function => Err(CodecFailure::new(
            format!("{path}: Function has no JSON codec"),
            value,
            schema.rule,
        )),
        _ => Err(CodecFailure::new(
            format!("{path}: expected {}", codec_type_name(schema)),
            value,
            schema.rule,
        )),
    }
}

fn validate_codec_value_without_skipping(
    schema: &CodecType,
    value: Val,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    let mut decisions = BTreeMap::new();
    loop {
        match transform_codec(
            schema,
            value,
            CodecDirection::Encode,
            path,
            &decisions,
            current,
            background,
        ) {
            Ok(node) => return Ok(node),
            Err(failure) => {
                let Some(request) = &failure.predicate else {
                    return Err(failure);
                };
                decisions.insert(request.path.clone(), false);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn transform_codec_struct(
    schema: &CodecType,
    fields: &BTreeMap<String, CodecType>,
    value: Val,
    direction: CodecDirection,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    let DecodedValue::Dict(handle) = value.value() else {
        return Err(CodecFailure::new(
            format!("{path}: expected Dict"),
            value,
            schema.rule,
        ));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let (names, values) = view
        .dict_parts(handle)
        .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
    let input = names
        .iter()
        .zip(values)
        .map(|(name, value)| Ok((view.text(*name)?.to_owned(), *value)))
        .collect::<Result<BTreeMap<_, _>, crate::heap::HeapError>>()
        .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
    let plan = plan_struct(schema, fields, value, path, &view)?;
    match direction {
        CodecDirection::Decode => {
            let mut consumed = HashSet::new();
            let output = decode_struct_fields(
                &plan,
                &input,
                &mut consumed,
                value,
                path,
                predicate_decisions,
                current,
                background,
            )?;
            if let Some(unknown) = input.keys().find(|name| !consumed.contains(*name)) {
                return Err(CodecFailure::new(
                    format!("{path}.{unknown}: unknown field"),
                    input[unknown],
                    schema.rule,
                ));
            }
            Ok(CodecNode::Dict(output, value.loc()))
        }
        CodecDirection::Encode => {
            let mut emitted = BTreeMap::new();
            encode_struct_fields(
                &plan,
                &input,
                &mut emitted,
                value,
                path,
                predicate_decisions,
                current,
                background,
            )?;
            Ok(CodecNode::Dict(emitted.into_iter().collect(), value.loc()))
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum SkipPolicy {
    None,
    False,
    Empty,
    Function(Val),
}

#[derive(Clone, Debug)]
struct StructFieldPlan {
    internal_name: String,
    external_name: Option<String>,
    schema: CodecType,
    flattened: Option<Box<StructPlan>>,
    default: Option<Val>,
    skip: Option<SkipPolicy>,
    config_rule: Val,
}

#[derive(Clone, Debug)]
struct StructPlan {
    fields: Vec<StructFieldPlan>,
}

fn plan_struct(
    schema: &CodecType,
    fields: &BTreeMap<String, CodecType>,
    data: Val,
    path: &str,
    view: &HeapView<'_>,
) -> Result<StructPlan, CodecFailure> {
    let rename_all = match schema.attributes.get("std/json.rename_all").copied() {
        Some(rule) => {
            if view
                .atom_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .is_none_or(|atom| atom != "CamelCase")
            {
                return Err(CodecFailure::new(
                    format!("{path}: rename_all must be 'CamelCase"),
                    data,
                    rule,
                ));
            }
            true
        }
        None => false,
    };
    let mut planned = Vec::with_capacity(fields.len());
    let mut external_names: BTreeMap<String, Val> = BTreeMap::new();
    for (internal_name, field) in fields {
        let mut field_schema = resolve_codec_type_once(field, data, view)?;
        if field_schema.rule.loc().is_none() {
            field_schema.rule = schema.rule;
        }
        let rename = field.attributes.get("std/json.rename").copied();
        let rename = rename
            .map(|rule| {
                view.string_text(rule)
                    .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                    .map(|text| text.as_str().to_owned())
                    .ok_or_else(|| {
                        CodecFailure::new(
                            format!("{path}.{internal_name}: rename must be a String"),
                            data,
                            rule,
                        )
                    })
            })
            .transpose()?;
        let flatten_rule = field.attributes.get("std/json.flatten").copied();
        let flatten = if let Some(rule) = flatten_rule {
            if view
                .atom_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .is_none_or(|atom| atom != "True")
            {
                return Err(CodecFailure::new(
                    format!("{path}.{internal_name}: flatten must be 'True"),
                    data,
                    rule,
                ));
            }
            true
        } else {
            false
        };
        let default = field.attributes.get("std/json.default").copied();
        if flatten && (rename.is_some() || default.is_some()) {
            let rule = rename
                .and_then(|_| field.attributes.get("std/json.rename").copied())
                .or_else(|| field.attributes.get("std/json.default").copied())
                .unwrap_or(field.rule);
            return Err(CodecFailure::new(
                format!(
                    "{path}.{internal_name}: flatten cannot be combined with rename or default"
                ),
                data,
                rule,
            ));
        }
        let skip = field
            .attributes
            .get("std/json.skip_serializing_if")
            .copied()
            .map(|rule| {
                let policy = view
                    .atom_text(rule)
                    .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?;
                match policy.as_ref().map(crate::TextRef::as_str) {
                    Some("None") => Ok(SkipPolicy::None),
                    Some("False") => Ok(SkipPolicy::False),
                    Some("Empty") => Ok(SkipPolicy::Empty),
                    _ => {
                        let Some(arity) = view.resolved_function_arity(rule).map_err(|error| {
                            CodecFailure::new(error.to_string(), data, rule)
                        })? else {
                            return Err(CodecFailure::new(
                                format!("{path}.{internal_name}: invalid skip_serializing_if policy"),
                                data,
                                rule,
                            ));
                        };
                        if arity != 1 {
                            return Err(CodecFailure::new(
                                format!("{path}.{internal_name}: skip_serializing_if predicate must accept one argument, got {arity}"),
                                data,
                                rule,
                            ));
                        }
                        Ok(SkipPolicy::Function(rule))
                    }
                }
            })
            .transpose()?;
        let config_rule = flatten_rule
            .or_else(|| field.attributes.get("std/json.rename").copied())
            .unwrap_or(field.rule);
        let (external_name, flattened) = if flatten {
            let CodecKind::Struct(nested_fields) = &field_schema.kind else {
                return Err(CodecFailure::new(
                    format!("{path}.{internal_name}: flatten requires Struct metadata"),
                    data,
                    flatten_rule.unwrap_or(field.rule),
                ));
            };
            let nested = plan_struct(
                &field_schema,
                nested_fields,
                data,
                &format!("{path}.{internal_name}"),
                view,
            )?;
            for (name, rule) in struct_plan_external_names(&nested) {
                if external_names.insert(name.clone(), rule).is_some() {
                    return Err(CodecFailure::new(
                        format!("{path}.{name}: duplicate external field name"),
                        data,
                        rule,
                    ));
                }
            }
            (None, Some(Box::new(nested)))
        } else {
            let external = rename.unwrap_or_else(|| {
                if rename_all {
                    lower_camel_case(internal_name)
                } else {
                    internal_name.clone()
                }
            });
            if external_names
                .insert(external.clone(), config_rule)
                .is_some()
            {
                return Err(CodecFailure::new(
                    format!("{path}.{external}: duplicate external field name"),
                    data,
                    config_rule,
                ));
            }
            (Some(external), None)
        };
        planned.push(StructFieldPlan {
            internal_name: internal_name.clone(),
            external_name,
            schema: field_schema,
            flattened,
            default,
            skip,
            config_rule,
        });
    }
    Ok(StructPlan { fields: planned })
}

fn resolve_codec_type_once(
    schema: &CodecType,
    data: Val,
    view: &HeapView<'_>,
) -> Result<CodecType, CodecFailure> {
    let (resolved, owner) = match schema.kind {
        CodecKind::TypeSlot(handle) => (
            view.type_slot(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), data, schema.rule))?
                .ok_or_else(|| {
                    CodecFailure::new("recursive type link is not initialized", data, schema.rule)
                })?,
            None,
        ),
        CodecKind::TypeRef(handle) => {
            let Object::DeclaredType { body, .. } = view
                .object(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), data, schema.rule))?
            else {
                return Err(CodecFailure::new(
                    "type ref is not sealed",
                    data,
                    schema.rule,
                ));
            };
            (
                *body,
                Some(Val::unknown(DecodedValue::DeclaredType(handle))),
            )
        }
        _ => return Ok(schema.clone()),
    };
    let mut resolved = decode_runtime_type(
        resolved,
        view.current,
        view.background.expect("codec views have a background heap"),
    )
    .map_err(|message| CodecFailure::new(message, data, schema.rule))?;
    resolved.declared_owner = owner;
    resolved.attributes.extend(schema.attributes.clone());
    Ok(resolved)
}

fn struct_plan_external_names(plan: &StructPlan) -> Vec<(String, Val)> {
    plan.fields
        .iter()
        .flat_map(|field| {
            if let Some(nested) = &field.flattened {
                struct_plan_external_names(nested)
            } else {
                vec![(
                    field.external_name.clone().expect("ordinary field name"),
                    field.config_rule,
                )]
            }
        })
        .collect()
}

fn lower_camel_case(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    let mut uppercase = false;
    for (index, character) in name.chars().enumerate() {
        if character == '_' {
            uppercase = true;
        } else if uppercase {
            output.extend(character.to_uppercase());
            uppercase = false;
        } else if index == 0 {
            output.extend(character.to_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

#[derive(Clone, Debug)]
struct EnumVariantPlan {
    internal_name: String,
    external_name: String,
    payload: Option<CodecType>,
    rule: Val,
}

#[derive(Clone, Debug)]
struct EnumPlan {
    variants: Vec<EnumVariantPlan>,
    untagged: bool,
}

fn plan_enum(
    schema: &CodecType,
    variants: &BTreeMap<String, CodecEnumVariant>,
    data: Val,
    path: &str,
    view: &HeapView<'_>,
) -> Result<EnumPlan, CodecFailure> {
    let untagged = match schema.attributes.get("std/json.untagged").copied() {
        Some(rule) => {
            if view
                .atom_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .is_none_or(|atom| atom != "True")
            {
                return Err(CodecFailure::new(
                    format!("{path}: untagged must be 'True"),
                    data,
                    rule,
                ));
            }
            true
        }
        None => false,
    };
    let rename_all = match schema.attributes.get("std/json.rename_all").copied() {
        Some(rule) => {
            if untagged {
                return Err(CodecFailure::new(
                    format!("{path}: rename_all is not meaningful on an untagged Enum"),
                    data,
                    rule,
                ));
            }
            if view
                .atom_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .is_none_or(|atom| atom != "CamelCase")
            {
                return Err(CodecFailure::new(
                    format!("{path}: rename_all must be 'CamelCase"),
                    data,
                    rule,
                ));
            }
            true
        }
        None => false,
    };
    let mut names = BTreeMap::new();
    let mut planned = Vec::with_capacity(variants.len());
    for (internal_name, variant) in variants {
        let rename_rule = variant.attributes.get("std/json.rename").copied();
        if let (true, Some(rule)) = (untagged, rename_rule) {
            return Err(CodecFailure::new(
                format!("{path}.{internal_name}: rename is not meaningful in an untagged Enum"),
                data,
                rule,
            ));
        }
        if untagged && variant.payload.is_none() {
            return Err(CodecFailure::new(
                format!("{path}.{internal_name}: untagged variants require payloads"),
                data,
                variant.rule,
            ));
        }
        let external_name = if let Some(rule) = rename_rule {
            view.string_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .map(|text| text.as_str().to_owned())
                .ok_or_else(|| {
                    CodecFailure::new(
                        format!("{path}.{internal_name}: rename must be a String"),
                        data,
                        rule,
                    )
                })?
        } else if rename_all {
            lower_camel_case(internal_name)
        } else {
            internal_name.clone()
        };
        if !untagged && names.insert(external_name.clone(), variant.rule).is_some() {
            return Err(CodecFailure::new(
                format!("{path}.{external_name}: duplicate external variant name"),
                data,
                rename_rule.unwrap_or(variant.rule),
            ));
        }
        planned.push(EnumVariantPlan {
            internal_name: internal_name.clone(),
            external_name,
            payload: variant.payload.as_deref().cloned(),
            rule: variant.rule,
        });
    }
    Ok(EnumPlan {
        variants: planned,
        untagged,
    })
}

#[allow(clippy::too_many_arguments)]
fn transform_codec_enum(
    schema: &CodecType,
    variants: &BTreeMap<String, CodecEnumVariant>,
    value: Val,
    direction: CodecDirection,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let plan = plan_enum(schema, variants, value, path, &view)?;
    if plan.untagged {
        return transform_untagged_enum(
            &plan,
            value,
            direction,
            path,
            predicate_decisions,
            current,
            background,
        );
    }
    match direction {
        CodecDirection::Decode => {
            if let Some(tag) = view
                .string_text(value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
            {
                let Some(variant) = plan
                    .variants
                    .iter()
                    .find(|variant| variant.external_name == tag)
                else {
                    return Err(CodecFailure::new(
                        format!("{path}: unknown Enum variant {tag:?}"),
                        value,
                        schema.rule,
                    ));
                };
                if variant.payload.is_some() {
                    return Err(CodecFailure::new(
                        format!("{path}: variant {tag:?} requires a payload"),
                        value,
                        variant.rule,
                    ));
                }
                return Ok(CodecNode::NamedAtom(
                    variant.internal_name.clone(),
                    value.loc(),
                ));
            }
            let DecodedValue::Dict(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected an Enum tag String or single-entry Dict"),
                    value,
                    schema.rule,
                ));
            };
            let (names, values) = view
                .dict_parts(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            if names.len() != 1 {
                return Err(CodecFailure::new(
                    format!("{path}: externally tagged Enum object must have one field"),
                    value,
                    schema.rule,
                ));
            }
            let tag = view
                .text(names[0])
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            let Some(variant) = plan
                .variants
                .iter()
                .find(|variant| variant.external_name == tag)
            else {
                return Err(CodecFailure::new(
                    format!("{path}: unknown Enum variant {tag:?}"),
                    value,
                    schema.rule,
                ));
            };
            let Some(payload) = &variant.payload else {
                return Err(CodecFailure::new(
                    format!("{path}: unit variant {tag:?} must be a String"),
                    value,
                    variant.rule,
                ));
            };
            Ok(CodecNode::Tagged {
                tag: Box::new(CodecNode::NamedAtom(
                    variant.internal_name.clone(),
                    value.loc(),
                )),
                payload: Box::new(transform_codec(
                    payload,
                    values[0],
                    direction,
                    &format!("{path}.{tag}"),
                    predicate_decisions,
                    current,
                    background,
                )?),
                loc: value.loc(),
            })
        }
        CodecDirection::Encode => {
            if let Some(tag) = view
                .atom_text(value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
            {
                let Some(variant) = plan
                    .variants
                    .iter()
                    .find(|variant| variant.internal_name == tag)
                else {
                    return Err(CodecFailure::new(
                        format!("{path}: unknown Enum tag '{tag}"),
                        value,
                        schema.rule,
                    ));
                };
                if variant.payload.is_some() {
                    return Err(CodecFailure::new(
                        format!("{path}: variant '{tag} requires a payload"),
                        value,
                        variant.rule,
                    ));
                }
                return Ok(CodecNode::String(
                    variant.external_name.clone(),
                    value.loc(),
                ));
            }
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected canonical Enum value"),
                    value,
                    schema.rule,
                ));
            };
            let (tag_value, payload_value) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            let tag = view
                .atom_text(tag_value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .ok_or_else(|| {
                    CodecFailure::new(
                        format!("{path}: Enum tuple tag must be an Atom"),
                        value,
                        schema.rule,
                    )
                })?;
            let Some(variant) = plan
                .variants
                .iter()
                .find(|variant| variant.internal_name == tag)
            else {
                return Err(CodecFailure::new(
                    format!("{path}: unknown Enum tag '{tag}"),
                    value,
                    schema.rule,
                ));
            };
            let Some(payload) = &variant.payload else {
                return Err(CodecFailure::new(
                    format!("{path}: unit variant '{tag} must not have a payload"),
                    value,
                    variant.rule,
                ));
            };
            Ok(CodecNode::Dict(
                vec![(
                    variant.external_name.clone(),
                    transform_codec(
                        payload,
                        payload_value,
                        direction,
                        &format!("{path}.{tag}"),
                        predicate_decisions,
                        current,
                        background,
                    )?,
                )],
                value.loc(),
            ))
        }
    }
}

fn transform_untagged_enum(
    plan: &EnumPlan,
    value: Val,
    direction: CodecDirection,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    match direction {
        CodecDirection::Decode => {
            let mut matches = Vec::new();
            let mut errors = Vec::new();
            for variant in &plan.variants {
                let payload = variant.payload.as_ref().expect("planned untagged payload");
                match transform_codec(
                    payload,
                    value,
                    direction,
                    path,
                    predicate_decisions,
                    current,
                    background,
                ) {
                    Ok(node) => matches.push((variant, node)),
                    Err(failure) if failure.predicate.is_some() => return Err(failure),
                    Err(failure) => errors.push(failure.message),
                }
            }
            match matches.as_slice() {
                [(variant, node)] => Ok(CodecNode::Tagged {
                    tag: Box::new(CodecNode::NamedAtom(
                        variant.internal_name.clone(),
                        value.loc(),
                    )),
                    payload: Box::new(node.clone()),
                    loc: value.loc(),
                }),
                [] => Err(CodecFailure::new(
                    format!(
                        "{path}: value matches no untagged Enum variant ({})",
                        errors.join("; ")
                    ),
                    value,
                    plan.variants
                        .first()
                        .map(|variant| variant.rule)
                        .unwrap_or(value),
                )),
                _ => Err(CodecFailure::new(
                    format!("{path}: value ambiguously matches multiple untagged Enum variants"),
                    value,
                    matches[1].0.rule,
                )),
            }
        }
        CodecDirection::Encode => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected ('Variant, payload)"),
                    value,
                    plan.variants
                        .first()
                        .map(|variant| variant.rule)
                        .unwrap_or(value),
                ));
            };
            let (tag_value, payload_value) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?;
            let tag = view
                .atom_text(tag_value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                .ok_or_else(|| {
                    CodecFailure::new(
                        format!("{path}: Enum tuple tag must be an Atom"),
                        value,
                        value,
                    )
                })?;
            let variant = plan
                .variants
                .iter()
                .find(|variant| variant.internal_name == tag)
                .ok_or_else(|| {
                    CodecFailure::new(format!("{path}: unknown Enum tag '{tag}"), value, value)
                })?;
            transform_codec(
                variant.payload.as_ref().expect("planned untagged payload"),
                payload_value,
                direction,
                path,
                predicate_decisions,
                current,
                background,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_struct_fields(
    plan: &StructPlan,
    input: &BTreeMap<String, Val>,
    consumed: &mut HashSet<String>,
    container: Val,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
) -> Result<Vec<(String, CodecNode)>, CodecFailure> {
    let mut output = Vec::with_capacity(plan.fields.len());
    for field in &plan.fields {
        let internal_path = format!("{path}.{}", field.internal_name);
        let node = if let Some(nested) = &field.flattened {
            CodecNode::Dict(
                decode_struct_fields(
                    nested,
                    input,
                    consumed,
                    container,
                    &internal_path,
                    predicate_decisions,
                    current,
                    background,
                )?,
                container.loc(),
            )
        } else {
            let external = field.external_name.as_ref().expect("ordinary field name");
            let external_path = format!("{path}.{external}");
            if let Some(value) = input.get(external).copied() {
                if !consumed.insert(external.clone()) {
                    return Err(CodecFailure::new(
                        format!("{external_path}: field was consumed more than once"),
                        value,
                        field.config_rule,
                    ));
                }
                transform_codec_field(
                    &field.schema,
                    value,
                    CodecDirection::Decode,
                    &external_path,
                    predicate_decisions,
                    current,
                    background,
                )?
            } else if let Some(default) = field.default {
                // A default is already canonical Telora data. Validate it through the
                // encode direction before retaining the original rich value.
                validate_codec_value_without_skipping(
                    &field.schema,
                    default,
                    &internal_path,
                    current,
                    background,
                )
                .map_err(|failure| CodecFailure::new(failure.message, default, default))?;
                CodecNode::Existing(default)
            } else if option_item(&field.schema).is_some() {
                CodecNode::Atom(BuiltinAtom::None, container.loc())
            } else {
                return Err(CodecFailure::new(
                    format!("{external_path}: missing required field"),
                    container,
                    field.schema.rule,
                ));
            }
        };
        output.push((field.internal_name.clone(), node));
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn encode_struct_fields(
    plan: &StructPlan,
    input: &BTreeMap<String, Val>,
    emitted: &mut BTreeMap<String, CodecNode>,
    container: Val,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
) -> Result<(), CodecFailure> {
    let expected = plan
        .fields
        .iter()
        .map(|field| field.internal_name.as_str())
        .collect::<HashSet<_>>();
    if let Some(unknown) = input.keys().find(|name| !expected.contains(name.as_str())) {
        return Err(CodecFailure::new(
            format!("{path}.{unknown}: unknown internal field"),
            input[unknown],
            plan.fields
                .first()
                .map(|field| field.schema.rule)
                .unwrap_or(container),
        ));
    }
    for field in &plan.fields {
        let field_path = format!("{path}.{}", field.internal_name);
        let Some(value) = input.get(&field.internal_name).copied() else {
            return Err(CodecFailure::new(
                format!("{field_path}: missing required field"),
                container,
                field.schema.rule,
            ));
        };
        if let Some(policy) = field.skip {
            let skip = match policy {
                SkipPolicy::Function(callee) => {
                    let Some(skip) = predicate_decisions.get(&field_path) else {
                        return Err(CodecFailure::predicate(
                            field_path.clone(),
                            callee,
                            value,
                            callee,
                        ));
                    };
                    *skip
                }
                policy => codec_should_skip(policy, value, current, background),
            };
            if skip {
                continue;
            }
        }
        if let Some(nested) = &field.flattened {
            let DecodedValue::Dict(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{field_path}: expected Dict"),
                    value,
                    field.schema.rule,
                ));
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let (names, values) = view
                .dict_parts(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, field.schema.rule))?;
            let nested_input = names
                .iter()
                .zip(values)
                .map(|(name, value)| Ok((view.text(*name)?.to_owned(), *value)))
                .collect::<Result<BTreeMap<_, _>, crate::heap::HeapError>>()
                .map_err(|error| CodecFailure::new(error.to_string(), value, field.schema.rule))?;
            encode_struct_fields(
                nested,
                &nested_input,
                emitted,
                value,
                &field_path,
                predicate_decisions,
                current,
                background,
            )?;
        } else {
            let external = field.external_name.as_ref().expect("ordinary field name");
            let node = transform_codec_field(
                &field.schema,
                value,
                CodecDirection::Encode,
                &field_path,
                predicate_decisions,
                current,
                background,
            )?;
            if emitted.insert(external.clone(), node).is_some() {
                return Err(CodecFailure::new(
                    format!("{path}.{external}: duplicate encoded field"),
                    value,
                    field.config_rule,
                ));
            }
        }
    }
    Ok(())
}

fn transform_codec_field(
    schema: &CodecType,
    value: Val,
    direction: CodecDirection,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    let Some(item) = option_item(schema) else {
        return transform_codec(
            schema,
            value,
            direction,
            path,
            predicate_decisions,
            current,
            background,
        );
    };
    if value.value() == DecodedValue::BuiltinAtom(BuiltinAtom::None) {
        return Ok(CodecNode::Atom(BuiltinAtom::None, value.loc()));
    }
    match direction {
        CodecDirection::Decode => Ok(CodecNode::Tagged {
            tag: Box::new(CodecNode::Atom(BuiltinAtom::Some, value.loc())),
            payload: Box::new(transform_codec(
                item,
                value,
                direction,
                path,
                predicate_decisions,
                current,
                background,
            )?),
            loc: value.loc(),
        }),
        CodecDirection::Encode => {
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected Option"),
                    value,
                    schema.rule,
                ));
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let (tag, payload) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            if view
                .atom_text(tag)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .is_none_or(|tag| tag != "Some")
            {
                return Err(CodecFailure::new(
                    format!("{path}: expected Option"),
                    value,
                    schema.rule,
                ));
            }
            transform_codec(
                item,
                payload,
                direction,
                path,
                predicate_decisions,
                current,
                background,
            )
        }
    }
}

fn codec_should_skip(policy: SkipPolicy, value: Val, current: &Heap, background: &Heap) -> bool {
    match policy {
        SkipPolicy::None => value.value() == DecodedValue::BuiltinAtom(BuiltinAtom::None),
        SkipPolicy::False => value.value() == DecodedValue::BuiltinAtom(BuiltinAtom::False),
        SkipPolicy::Empty => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            match value.value() {
                DecodedValue::InlineString(_) | DecodedValue::ShortString(_) => view
                    .string_text(value)
                    .ok()
                    .flatten()
                    .is_some_and(|text| text.as_str().is_empty()),
                DecodedValue::Array(handle) => view
                    .sequence(handle, false)
                    .is_ok_and(|values| values.is_empty()),
                DecodedValue::Dict(handle) => view
                    .dict_parts(handle)
                    .is_ok_and(|(names, _)| names.is_empty()),
                _ => false,
            }
        }
        SkipPolicy::Function(_) => unreachable!("function skip predicates suspend the codec"),
    }
}

fn schema_dict(fields: Vec<(&str, CodecNode)>, loc: Option<crate::Loc>) -> CodecNode {
    CodecNode::Dict(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
        loc,
    )
}

fn schema_string(value: &str, loc: Option<crate::Loc>) -> CodecNode {
    CodecNode::String(value.to_owned(), loc)
}

type SchemaProperties = Vec<(String, CodecNode)>;

fn generate_json_schema(
    schema: &CodecType,
    data: Val,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    let mut links = HashMap::new();
    let mut definitions = BTreeMap::new();
    let mut root = generate_json_schema_node(
        schema,
        data,
        current,
        background,
        &mut links,
        &mut definitions,
    )?;
    if !definitions.is_empty() {
        let CodecNode::Dict(fields, _) = &mut root else {
            unreachable!("every generated schema is an object")
        };
        fields.push((
            "$defs".into(),
            CodecNode::Dict(definitions.into_iter().collect(), schema.rule.loc()),
        ));
    }
    Ok(root)
}

#[allow(clippy::too_many_arguments)]
fn generate_json_schema_node(
    schema: &CodecType,
    data: Val,
    current: &Heap,
    background: &Heap,
    links: &mut HashMap<Handle, String>,
    definitions: &mut BTreeMap<String, CodecNode>,
) -> Result<CodecNode, CodecFailure> {
    let loc = schema.rule.loc();
    let view = HeapView {
        current,
        background: Some(background),
    };
    if !matches!(schema.kind, CodecKind::TypeSlot(_) | CodecKind::TypeRef(_))
        && text_codec_bridge(schema, &view)
            .map_err(|message| CodecFailure::new(message, data, schema.rule))?
    {
        return Ok(schema_dict(
            vec![("type", schema_string("string", loc))],
            loc,
        ));
    }
    if let Some(item) = option_item(schema) {
        return Ok(schema_dict(
            vec![(
                "anyOf",
                CodecNode::Array(
                    vec![
                        schema_dict(vec![("type", schema_string("null", loc))], loc),
                        generate_json_schema_node(
                            item,
                            data,
                            current,
                            background,
                            links,
                            definitions,
                        )?,
                    ],
                    loc,
                ),
            )],
            loc,
        ));
    }
    match &schema.kind {
        CodecKind::TypeSlot(handle) => {
            if let Some(name) = links.get(handle) {
                return Ok(schema_dict(
                    vec![("$ref", schema_string(&format!("#/$defs/{name}"), loc))],
                    loc,
                ));
            }
            let name = format!("Type{}", links.len());
            links.insert(*handle, name.clone());
            let view = HeapView {
                current,
                background: Some(background),
            };
            let resolved = view
                .type_slot(*handle)
                .map_err(|error| CodecFailure::new(error.to_string(), data, schema.rule))?
                .ok_or_else(|| {
                    CodecFailure::new("recursive type link is not initialized", data, schema.rule)
                })?;
            let resolved = decode_runtime_type(resolved, current, background)
                .map_err(|message| CodecFailure::new(message, data, schema.rule))?;
            let definition = generate_json_schema_node(
                &resolved,
                data,
                current,
                background,
                links,
                definitions,
            )?;
            definitions.insert(name.clone(), definition);
            Ok(schema_dict(
                vec![("$ref", schema_string(&format!("#/$defs/{name}"), loc))],
                loc,
            ))
        }
        CodecKind::TypeRef(handle) => {
            if let Some(name) = links.get(handle) {
                return Ok(schema_dict(
                    vec![("$ref", schema_string(&format!("#/$defs/{name}"), loc))],
                    loc,
                ));
            }
            let name = format!("Type{}", links.len());
            links.insert(*handle, name.clone());
            let view = HeapView {
                current,
                background: Some(background),
            };
            let Object::DeclaredType { body, .. } = view
                .object(*handle)
                .map_err(|error| CodecFailure::new(error.to_string(), data, schema.rule))?
            else {
                return Err(CodecFailure::new(
                    "type ref is not sealed",
                    data,
                    schema.rule,
                ));
            };
            let resolved = decode_runtime_type(*body, current, background)
                .map_err(|message| CodecFailure::new(message, data, schema.rule))?;
            let definition = generate_json_schema_node(
                &resolved,
                data,
                current,
                background,
                links,
                definitions,
            )?;
            definitions.insert(name.clone(), definition);
            Ok(schema_dict(
                vec![("$ref", schema_string(&format!("#/$defs/{name}"), loc))],
                loc,
            ))
        }
        CodecKind::Any => Ok(CodecNode::Dict(Vec::new(), loc)),
        CodecKind::Type => Err(CodecFailure::new(
            "JSON Schema cannot describe Type metadata",
            schema.rule,
            schema.rule,
        )),
        CodecKind::Dyn => Err(CodecFailure::new(
            "JSON Schema cannot describe Dyn",
            schema.rule,
            schema.rule,
        )),
        CodecKind::Int => Ok(schema_dict(
            vec![("type", schema_string("integer", loc))],
            loc,
        )),
        CodecKind::Float => Ok(schema_dict(
            vec![("type", schema_string("number", loc))],
            loc,
        )),
        CodecKind::String => Ok(schema_dict(
            vec![("type", schema_string("string", loc))],
            loc,
        )),
        CodecKind::Atom(tag) if tag == "None" => {
            Ok(schema_dict(vec![("type", schema_string("null", loc))], loc))
        }
        CodecKind::Atom(tag) => Ok(schema_dict(vec![("const", schema_string(tag, loc))], loc)),
        CodecKind::Array(item) => Ok(schema_dict(
            vec![
                ("type", schema_string("array", loc)),
                (
                    "items",
                    generate_json_schema_node(item, data, current, background, links, definitions)?,
                ),
            ],
            loc,
        )),
        CodecKind::Dict(item) => Ok(schema_dict(
            vec![
                ("type", schema_string("object", loc)),
                (
                    "additionalProperties",
                    generate_json_schema_node(item, data, current, background, links, definitions)?,
                ),
            ],
            loc,
        )),
        CodecKind::Tagged { payload, .. } => {
            generate_json_schema_node(payload, data, current, background, links, definitions)
        }
        CodecKind::Tuple(items) => {
            let schemas = items
                .iter()
                .map(|item| {
                    generate_json_schema_node(item, data, current, background, links, definitions)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let length = Val::unknown(DecodedValue::Int(items.len() as i64));
            Ok(schema_dict(
                vec![
                    ("type", schema_string("array", loc)),
                    ("prefixItems", CodecNode::Array(schemas, loc)),
                    ("minItems", CodecNode::Existing(length)),
                    ("maxItems", CodecNode::Existing(length)),
                ],
                loc,
            ))
        }
        CodecKind::Struct(fields) => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            let plan = plan_struct(schema, fields, data, "$", &view)?;
            let (properties, required) = generate_struct_schema_fields(
                &plan,
                data,
                current,
                background,
                links,
                definitions,
            )?;
            let mut fields = vec![
                ("type", schema_string("object", loc)),
                ("properties", CodecNode::Dict(properties, loc)),
                (
                    "additionalProperties",
                    CodecNode::Atom(BuiltinAtom::False, loc),
                ),
            ];
            if !required.is_empty() {
                fields.push((
                    "required",
                    CodecNode::Array(
                        required
                            .into_iter()
                            .map(|name| CodecNode::String(name, loc))
                            .collect(),
                        loc,
                    ),
                ));
            }
            Ok(schema_dict(fields, loc))
        }
        CodecKind::Enum(variants) if is_bool_enum(variants) => Ok(schema_dict(
            vec![("type", schema_string("boolean", loc))],
            loc,
        )),
        CodecKind::Enum(variants) => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            let plan = plan_enum(schema, variants, data, "$", &view)?;
            let branches = if plan.untagged {
                plan.variants
                    .iter()
                    .map(|variant| {
                        generate_json_schema_node(
                            variant.payload.as_ref().expect("planned untagged payload"),
                            data,
                            current,
                            background,
                            links,
                            definitions,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                plan.variants
                    .iter()
                    .map(|variant| {
                        if let Some(payload) = &variant.payload {
                            let property = generate_json_schema_node(
                                payload,
                                data,
                                current,
                                background,
                                links,
                                definitions,
                            )?;
                            Ok(schema_dict(
                                vec![
                                    ("type", schema_string("object", variant.rule.loc())),
                                    (
                                        "properties",
                                        CodecNode::Dict(
                                            vec![(variant.external_name.clone(), property)],
                                            variant.rule.loc(),
                                        ),
                                    ),
                                    (
                                        "required",
                                        CodecNode::Array(
                                            vec![schema_string(
                                                &variant.external_name,
                                                variant.rule.loc(),
                                            )],
                                            variant.rule.loc(),
                                        ),
                                    ),
                                    (
                                        "additionalProperties",
                                        CodecNode::Atom(BuiltinAtom::False, variant.rule.loc()),
                                    ),
                                ],
                                variant.rule.loc(),
                            ))
                        } else {
                            Ok(schema_dict(
                                vec![(
                                    "const",
                                    schema_string(&variant.external_name, variant.rule.loc()),
                                )],
                                variant.rule.loc(),
                            ))
                        }
                    })
                    .collect::<Result<Vec<_>, CodecFailure>>()?
            };
            Ok(schema_dict(
                vec![("oneOf", CodecNode::Array(branches, loc))],
                loc,
            ))
        }
        CodecKind::Union(variants) => Ok(schema_dict(
            vec![(
                "anyOf",
                CodecNode::Array(
                    variants
                        .iter()
                        .map(|variant| {
                            generate_json_schema_node(
                                variant,
                                data,
                                current,
                                background,
                                links,
                                definitions,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    loc,
                ),
            )],
            loc,
        )),
        CodecKind::Bytes | CodecKind::Opaque | CodecKind::Function => Err(CodecFailure::new(
            format!(
                "Type {} has no JSON Schema mapping",
                codec_type_name(schema)
            ),
            data,
            schema.rule,
        )),
    }
}

fn generate_struct_schema_fields(
    plan: &StructPlan,
    data: Val,
    current: &Heap,
    background: &Heap,
    links: &mut HashMap<Handle, String>,
    definitions: &mut BTreeMap<String, CodecNode>,
) -> Result<(SchemaProperties, Vec<String>), CodecFailure> {
    let mut properties = Vec::new();
    let mut required = Vec::new();
    for field in &plan.fields {
        if let Some(nested) = &field.flattened {
            let (nested_properties, nested_required) = generate_struct_schema_fields(
                nested,
                data,
                current,
                background,
                links,
                definitions,
            )?;
            properties.extend(nested_properties);
            required.extend(nested_required);
            continue;
        }
        let external = field.external_name.clone().expect("ordinary field name");
        let mut property = generate_json_schema_node(
            &field.schema,
            data,
            current,
            background,
            links,
            definitions,
        )?;
        if let Some(default) = field.default {
            let encoded = validate_codec_value_without_skipping(
                &field.schema,
                default,
                &format!("$.{}", field.internal_name),
                current,
                background,
            )
            .map_err(|failure| CodecFailure::new(failure.message, default, default))?;
            let CodecNode::Dict(fields, _) = &mut property else {
                unreachable!("every generated schema is an object")
            };
            fields.push(("default".into(), encoded));
        } else if option_item(&field.schema).is_none() {
            required.push(external.clone());
        }
        properties.push((external, property));
    }
    Ok((properties, required))
}

fn codec_type_name(schema: &CodecType) -> &'static str {
    match &schema.kind {
        CodecKind::TypeSlot(_) | CodecKind::TypeRef(_) => "recursive Type",
        CodecKind::Any => "Any",
        CodecKind::Type => "Type",
        CodecKind::Dyn => "Dyn",
        CodecKind::Int => "Int",
        CodecKind::Float => "Float",
        CodecKind::String => "String",
        CodecKind::Bytes => "Bytes",
        CodecKind::Opaque => "Opaque",
        CodecKind::Atom(_) => "Atom",
        CodecKind::Array(_) => "Array",
        CodecKind::Dict(_) => "Dict",
        CodecKind::Tagged { .. } => "Tagged",
        CodecKind::Tuple(_) => "Tuple",
        CodecKind::Struct(_) => "Struct",
        CodecKind::Enum(variants) => {
            let _ = variants;
            "Enum"
        }
        CodecKind::Union(_) => "Union",
        CodecKind::Function => "Func",
    }
}

fn codec_node_bytes(
    node: &CodecNode,
    current: &Heap,
    background: &Heap,
) -> Result<u64, NativeError> {
    match node {
        CodecNode::Existing(_) | CodecNode::Atom(_, _) => Ok(0),
        CodecNode::SemanticValue { raw, .. } => codec_node_bytes(raw, current, background)?
            .checked_add(semantic_codec_wrapper_bytes(raw, current, background)?)
            .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed")),
        CodecNode::Declared { payload, .. } => codec_node_bytes(payload, current, background),
        CodecNode::NamedAtom(value, _) | CodecNode::String(value, _) => Ok(value.len() as u64),
        CodecNode::Array(items, _) | CodecNode::Tuple(items, _) => {
            let own = logical_value_bytes(items.len())?;
            items.iter().try_fold(own, |total, item| {
                total
                    .checked_add(codec_node_bytes(item, current, background)?)
                    .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed"))
            })
        }
        CodecNode::Tagged { tag, payload, .. } => {
            let tag = codec_node_bytes(tag, current, background)?;
            let payload = codec_node_bytes(payload, current, background)?;
            logical_value_bytes(2)?
                .checked_add(tag)
                .and_then(|total| total.checked_add(payload))
                .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed"))
        }
        CodecNode::Dict(fields, _) => {
            let own = logical_value_bytes(fields.len())?;
            fields.iter().try_fold(own, |total, (name, value)| {
                let value_bytes = codec_node_bytes(value, current, background)?;
                total
                    .checked_add(name.len() as u64)
                    .and_then(|total| total.checked_add(value_bytes))
                    .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed"))
            })
        }
    }
}

fn semantic_codec_wrapper_bytes(
    node: &CodecNode,
    current: &Heap,
    background: &Heap,
) -> Result<u64, NativeError> {
    fn add(left: u64, right: u64) -> Result<u64, NativeError> {
        left.checked_add(right)
            .ok_or_else(|| NativeError::allocation_limit("semantic Value size overflowed"))
    }

    let tagged_bytes = logical_value_bytes(2)?;
    match node {
        CodecNode::Existing(value) => {
            semantic_value_wrapper_bytes(current, Some(background), *value)
                .map_err(|error| NativeError::new(error.to_string()))
        }
        CodecNode::Declared { payload, .. } => {
            semantic_codec_wrapper_bytes(payload, current, background)
        }
        CodecNode::Atom(BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False, _) => Ok(0),
        CodecNode::String(_, _) => Ok(tagged_bytes),
        CodecNode::Array(items, _) => {
            let mut bytes = add(logical_value_bytes(items.len())?, tagged_bytes)?;
            for item in items {
                bytes = add(
                    bytes,
                    semantic_codec_wrapper_bytes(item, current, background)?,
                )?;
            }
            Ok(bytes)
        }
        CodecNode::Dict(fields, _) => {
            let mut bytes = add(logical_value_bytes(fields.len())?, tagged_bytes)?;
            for (_, value) in fields {
                bytes = add(
                    bytes,
                    semantic_codec_wrapper_bytes(value, current, background)?,
                )?;
            }
            Ok(bytes)
        }
        CodecNode::Tagged { tag, payload, .. } => {
            let CodecNode::NamedAtom(tag, _) = tag.as_ref() else {
                return Err(NativeError::new("semantic temporal tag is not an Atom"));
            };
            if !matches!(
                tag.as_str(),
                "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
            ) || !matches!(
                payload.as_ref(),
                CodecNode::String(_, _) | CodecNode::Existing(_)
            ) {
                return Err(NativeError::new(
                    "raw data graph contains unsupported tagged value",
                ));
            }
            Ok(tagged_bytes)
        }
        CodecNode::SemanticValue { .. }
        | CodecNode::NamedAtom(_, _)
        | CodecNode::Tuple(_, _)
        | CodecNode::Atom(_, _) => Err(NativeError::new(
            "raw data graph contains an unsupported semantic Value",
        )),
    }
}

fn materialize_codec_node(node: CodecNode, current: &mut Heap, background: &Heap) -> Val {
    match node {
        CodecNode::Existing(value) => value,
        CodecNode::SemanticValue { owner, raw } => {
            let raw = materialize_codec_node(*raw, current, background);
            wrap_semantic_value(current, Some(background), raw, owner)
                .expect("codec Value owner and raw output were validated")
        }
        CodecNode::Declared {
            owner,
            payload,
            loc,
        } => {
            let payload = materialize_codec_node(*payload, current, background);
            let type_id = HeapView {
                current,
                background: Some(background),
            }
            .declared_type_id(owner)
            .expect("codec declared owner was decoded as a concrete declared Type");
            payload.with_type_id(type_id).with_loc(loc)
        }
        CodecNode::Atom(atom, loc) => Val::new(DecodedValue::BuiltinAtom(atom), loc),
        CodecNode::NamedAtom(value, loc) => Val::new(current.atom(Some(background), &value), loc),
        CodecNode::String(value, loc) => Val::new(current.string(Some(background), &value), loc),
        CodecNode::Array(items, loc) => {
            let items = items
                .into_iter()
                .map(|item| materialize_codec_node(item, current, background))
                .collect::<Box<_>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(items))),
                loc,
            )
        }
        CodecNode::Tuple(items, loc) => {
            let items = items
                .into_iter()
                .map(|item| materialize_codec_node(item, current, background))
                .collect::<Box<_>>();
            Val::new(
                DecodedValue::Tuple(current.allocate(Object::Tuple(items))),
                loc,
            )
        }
        CodecNode::Tagged { tag, payload, loc } => {
            let tag = materialize_codec_node(*tag, current, background);
            let payload = materialize_codec_node(*payload, current, background);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
        CodecNode::Dict(fields, loc) => {
            let mut fields = fields;
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            let (fields, values): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .map(|(name, value)| {
                    (
                        current.intern(&name),
                        materialize_codec_node(value, current, background),
                    )
                })
                .unzip();
            let shape = current.intern_shape(fields);
            Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: values.into(),
                })),
                loc,
            )
        }
    }
}

fn run_core_result(
    _operation: CoreResultFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<VmAction, RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let DecodedValue::Tagged(handle) = arguments[0].value() else {
        return Err(runtime_type_error(
            "'Ok(value) or 'Err(message)",
            &arguments[0],
            &view,
            function,
            pc,
        ));
    };
    let (tag, payload) = view
        .tagged(handle)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
    let tag = view.atom_text(tag).map_err(|heap_error| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            heap_error.to_string(),
            function,
            pc,
        )
    })?;
    match tag.as_ref().map(crate::TextRef::as_str) {
        Some("Ok") => Ok(VmAction::Return {
            value: payload,
            return_target,
        }),
        Some("Err") => {
            let (message, data_location, rule_location) = if let Some(message) =
                view.string_text(payload).map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        pc,
                    )
                })? {
                (message.as_str().to_owned(), payload.loc(), None)
            } else if let DecodedValue::Dict(handle) = payload.value() {
                let message = view
                    .dict_get_text(handle, "message")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .and_then(|message| view.string_text(message).ok().flatten())
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "structured Err payload message must be a String",
                            function,
                            pc,
                        )
                    })?
                    .as_str()
                    .to_owned();
                let data = view
                    .dict_get_text(handle, "data")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "structured Err payload is missing data",
                            function,
                            pc,
                        )
                    })?;
                let rule = view
                    .dict_get_text(handle, "rule")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "structured Err payload is missing rule",
                            function,
                            pc,
                        )
                    })?;
                (message, data.loc(), rule.loc())
            } else {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/result.unwrap Err payload must be a String or diagnostic Dict",
                    function,
                    pc,
                ));
            };
            let mut runtime_error = error(RuntimeErrorKind::TypeMismatch, message, function, pc);
            runtime_error.set_locations(data_location, rule_location);
            Err(runtime_error)
        }
        _ => Err(error(
            RuntimeErrorKind::TypeMismatch,
            "std/result.unwrap expects 'Ok(value) or 'Err(message)",
            function,
            pc,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_core_json(
    operation: CoreJsonFunction,
    arguments: &[Val],
    upvalues: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    if matches!(
        operation,
        CoreJsonFunction::Parse
            | CoreJsonFunction::ParseYaml
            | CoreJsonFunction::ParseToml
            | CoreJsonFunction::Decode
    ) {
        let input_index = 1;
        let view = HeapView {
            current,
            background: Some(background),
        };
        let Some(source) = ValueRef {
            value: arguments[input_index],
            view,
        }
        .as_str() else {
            return Err(runtime_type_error(
                "String",
                &arguments[input_index],
                &view,
                function,
                pc,
            ));
        };
        let parsed = match operation {
            CoreJsonFunction::Parse | CoreJsonFunction::Decode => {
                crate::json::parse_json("<json string>", source.as_str())
                    .map_err(|error| error.message)
            }
            CoreJsonFunction::ParseYaml => {
                let mut sources = SourceDatabase::default();
                let source_id = sources.add("<yaml string>", source.as_str());
                let parsed = crate::yaml::parse_yaml_registered(&sources, source_id);
                parsed.value.map(|value| value.value).ok_or_else(|| {
                    parsed.diagnostics.first().map_or_else(
                        || "invalid YAML".into(),
                        |diagnostic| diagnostic.message.clone(),
                    )
                })
            }
            CoreJsonFunction::ParseToml => {
                let mut sources = SourceDatabase::default();
                let source_id = sources.add("<toml string>", source.as_str());
                let parsed = crate::toml::parse_toml_registered(&sources, source_id);
                parsed.value.map(|value| value.value).ok_or_else(|| {
                    parsed.diagnostics.first().map_or_else(
                        || "invalid TOML".into(),
                        |diagnostic| diagnostic.message.clone(),
                    )
                })
            }
            _ => unreachable!(),
        };
        let parsed = match parsed {
            Ok(value) => {
                charge_allocation(account, source.len() as u64, function, pc)?;
                value
                    .relocate_into(current, background)
                    .map_err(|heap_error| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            heap_error.to_string(),
                            function,
                            pc,
                        )
                    })?
            }
            Err(parse_error) => {
                let rule = Val::new(
                    current.atom(
                        Some(background),
                        match operation {
                            CoreJsonFunction::ParseYaml => "Yaml",
                            CoreJsonFunction::ParseToml => "Toml",
                            _ => "Json",
                        },
                    ),
                    arguments[input_index].loc(),
                );
                return finish_codec_result(
                    Err(CodecFailure {
                        message: parse_error,
                        data: arguments[input_index],
                        rule,
                        predicate: None,
                    }),
                    arguments[input_index],
                    return_target,
                    function,
                    pc,
                    current,
                    background,
                    account,
                );
            }
        };
        if matches!(
            operation,
            CoreJsonFunction::Parse | CoreJsonFunction::ParseYaml | CoreJsonFunction::ParseToml
        ) {
            let wrapper_bytes = semantic_value_wrapper_bytes(current, Some(background), parsed)
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::TypeMismatch,
                        heap_error.to_string(),
                        function,
                        pc,
                    )
                })?;
            charge_allocation(account, wrapper_bytes, function, pc)?;
            let parsed = wrap_semantic_value(current, Some(background), parsed, arguments[0])
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::TypeMismatch,
                        heap_error.to_string(),
                        function,
                        pc,
                    )
                })?;
            return finish_codec_result(
                Ok(CodecNode::Existing(parsed)),
                arguments[input_index],
                return_target,
                function,
                pc,
                current,
                background,
                account,
            );
        }
        let schema = decode_runtime_type(arguments[0], current, background)
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        assert_codec_graph_ready(&schema, current, background).map_err(|graph_error| {
            match graph_error {
                CodecGraphError::Pending => error(
                    RuntimeErrorKind::UninitializedDefinition,
                    "codec was invoked before recursive type metadata was sealed",
                    function,
                    pc,
                ),
                CodecGraphError::Invalid(message) => {
                    error(RuntimeErrorKind::TypeMismatch, message, function, pc)
                }
            }
        })?;
        let result = transform_codec(
            &schema,
            parsed,
            CodecDirection::Decode,
            "$",
            &BTreeMap::new(),
            current,
            background,
        );
        return finish_codec_result(
            result,
            arguments[input_index],
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    if matches!(
        operation,
        CoreJsonFunction::Rename
            | CoreJsonFunction::RenameAll
            | CoreJsonFunction::Default
            | CoreJsonFunction::SkipSerializingIf
    ) {
        validate_json_attribute_configuration(
            operation,
            arguments[0],
            function,
            pc,
            current,
            background,
        )?;
        let configured = match operation {
            CoreJsonFunction::Rename => CoreJsonFunction::RenameDecorator,
            CoreJsonFunction::RenameAll => CoreJsonFunction::RenameAllDecorator,
            CoreJsonFunction::Default => CoreJsonFunction::DefaultDecorator,
            CoreJsonFunction::SkipSerializingIf => CoreJsonFunction::SkipSerializingIfDecorator,
            _ => unreachable!(),
        };
        charge_allocation(
            account,
            logical_value_bytes(1)
                .map_err(|error| allocation_error(error.message, function, pc))?,
            function,
            pc,
        )?;
        let value = Val::new(
            DecodedValue::Func(current.allocate(Object::Closure {
                identity: Arc::new(()),
                prototype: crate::heap::RuntimePrototype::Native(crate::NativeFunction::core_json(
                    configured,
                )),
                upvalues: vec![arguments[0]].into(),
            })),
            instruction_location(function, pc),
        );
        return Ok(VmAction::Return {
            value,
            return_target,
        });
    }
    if operation == CoreJsonFunction::Schema {
        let schema = decode_runtime_type(arguments[0], current, background)
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        assert_codec_graph_ready(&schema, current, background).map_err(|graph_error| {
            match graph_error {
                CodecGraphError::Pending => error(
                    RuntimeErrorKind::UninitializedDefinition,
                    "schema generation was invoked before recursive type metadata was sealed",
                    function,
                    pc,
                ),
                CodecGraphError::Invalid(message) => {
                    error(RuntimeErrorKind::TypeMismatch, message, function, pc)
                }
            }
        })?;
        let mut node = generate_json_schema(&schema, arguments[0], current, background).map_err(
            |failure| {
                let mut runtime = error(
                    RuntimeErrorKind::TypeMismatch,
                    failure.message,
                    function,
                    pc,
                );
                runtime.set_locations(failure.data.loc(), failure.rule.loc());
                runtime
            },
        )?;
        let CodecNode::Dict(fields, _) = &mut node else {
            unreachable!("root schema is always an object")
        };
        fields.push((
            "$schema".into(),
            CodecNode::String(
                "https://json-schema.org/draft/2020-12/schema".into(),
                arguments[0].loc(),
            ),
        ));
        let bytes = codec_node_bytes(&node, current, background)
            .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
        charge_allocation(account, bytes, function, pc)?;
        let value = materialize_codec_node(node, current, background);
        return Ok(VmAction::Return {
            value,
            return_target,
        });
    }
    if matches!(
        operation,
        CoreJsonFunction::Flatten
            | CoreJsonFunction::Untagged
            | CoreJsonFunction::RenameDecorator
            | CoreJsonFunction::RenameAllDecorator
            | CoreJsonFunction::DefaultDecorator
            | CoreJsonFunction::SkipSerializingIfDecorator
    ) {
        let (key, payload) = match operation {
            CoreJsonFunction::Flatten => (
                "std/json.flatten",
                Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::True),
                    instruction_location(function, pc),
                ),
            ),
            CoreJsonFunction::Untagged => (
                "std/json.untagged",
                Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::True),
                    instruction_location(function, pc),
                ),
            ),
            CoreJsonFunction::RenameDecorator => (
                "std/json.rename",
                configured_json_attribute(upvalues, function, pc)?,
            ),
            CoreJsonFunction::RenameAllDecorator => (
                "std/json.rename_all",
                configured_json_attribute(upvalues, function, pc)?,
            ),
            CoreJsonFunction::DefaultDecorator => (
                "std/json.default",
                configured_json_attribute(upvalues, function, pc)?,
            ),
            CoreJsonFunction::SkipSerializingIfDecorator => (
                "std/json.skip_serializing_if",
                configured_json_attribute(upvalues, function, pc)?,
            ),
            _ => unreachable!(),
        };
        let (inner, mut attributes) = flatten_attributes(
            arguments[1],
            "decorated value",
            function,
            pc,
            current,
            background,
        )?;
        attributes.insert(key.to_owned(), payload);
        let value = allocate_attributes_wrapper(
            inner,
            attributes,
            instruction_location(function, pc),
            function,
            pc,
            current,
            account,
        )?;
        return Ok(VmAction::Return {
            value,
            return_target,
        });
    }
    if operation == CoreJsonFunction::StringifyPretty {
        let DecodedValue::Int(indent) = arguments[0].value() else {
            let view = HeapView {
                current,
                background: Some(background),
            };
            return Err(runtime_type_error(
                "Int",
                &arguments[0],
                &view,
                function,
                pc,
            ));
        };
        if !(0..=16).contains(&indent) {
            return Err(error(
                RuntimeErrorKind::TypeMismatch,
                "std/json.stringify_pretty indent must be between 0 and 16",
                function,
                pc,
            ));
        }
        charge_allocation(
            account,
            logical_value_bytes(1).map_err(|e| allocation_error(e.message, function, pc))?,
            function,
            pc,
        )?;
        let closure = Val::new(
            DecodedValue::Func(current.allocate(Object::Closure {
                identity: Arc::new(()),
                prototype: crate::heap::RuntimePrototype::Native(crate::NativeFunction::core_json(
                    CoreJsonFunction::StringifyPrettyValue,
                )),
                upvalues: vec![Val::new(DecodedValue::Int(indent), arguments[0].loc())].into(),
            })),
            instruction_location(function, pc),
        );
        return Ok(VmAction::Return {
            value: closure,
            return_target,
        });
    }
    let indent = match operation {
        CoreJsonFunction::Stringify => None,
        CoreJsonFunction::StringifyPrettyValue => match upvalues {
            [value] if matches!(value.value(), DecodedValue::Int(_)) => {
                let DecodedValue::Int(indent) = value.value() else {
                    unreachable!()
                };
                Some(indent as usize)
            }
            _ => {
                return Err(error(
                    RuntimeErrorKind::InvalidBytecode,
                    "configured JSON formatter has invalid upvalues",
                    function,
                    pc,
                ));
            }
        },
        CoreJsonFunction::StringifyPretty
        | CoreJsonFunction::Parse
        | CoreJsonFunction::ParseYaml
        | CoreJsonFunction::ParseToml
        | CoreJsonFunction::Decode
        | CoreJsonFunction::Rename
        | CoreJsonFunction::RenameDecorator
        | CoreJsonFunction::RenameAll
        | CoreJsonFunction::RenameAllDecorator
        | CoreJsonFunction::Flatten
        | CoreJsonFunction::Untagged
        | CoreJsonFunction::Schema
        | CoreJsonFunction::Default
        | CoreJsonFunction::DefaultDecorator
        | CoreJsonFunction::SkipSerializingIf
        | CoreJsonFunction::SkipSerializingIfDecorator => unreachable!(),
    };
    let owner = {
        let view = HeapView {
            current,
            background: Some(background),
        };
        propagate_data_failures(&[arguments[0]], &view, function, pc)?;
        view.type_witness(arguments[0])
            .map_err(|heap_error| {
                error(
                    RuntimeErrorKind::TypeMismatch,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            })?
            .ok_or_else(|| {
                error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/json.stringify expects std/value.Value",
                    function,
                    pc,
                )
            })?
    };
    let unwrap_bytes = semantic_value_unwrap_bytes(current, Some(background), arguments[0], owner)
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::TypeMismatch,
                heap_error.to_string(),
                function,
                pc,
            )
        })?;
    charge_allocation(account, unwrap_bytes, function, pc)?;
    let raw = unwrap_semantic_value(current, Some(background), arguments[0], owner).map_err(
        |heap_error| {
            error(
                RuntimeErrorKind::TypeMismatch,
                heap_error.to_string(),
                function,
                pc,
            )
        },
    )?;
    let mut writer = JsonWriter::new(
        HeapView {
            current,
            background: Some(background),
        },
        indent,
    );
    writer
        .value(raw, 0)
        .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
    let output = writer.output;
    charge_allocation(account, output.len() as u64, function, pc)?;
    let value = Val::new(
        current.string(Some(background), &output),
        instruction_location(function, pc),
    );
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

fn configured_json_attribute(
    upvalues: &[Val],
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    match upvalues {
        [payload] => Ok(*payload),
        _ => Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "configured JSON decorator has invalid upvalues",
            function,
            pc,
        )),
    }
}

fn validate_json_attribute_configuration(
    operation: CoreJsonFunction,
    payload: Val,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<(), RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let valid = match operation {
        CoreJsonFunction::Rename => view
            .string_text(payload)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            .is_some(),
        CoreJsonFunction::RenameAll => view
            .atom_text(payload)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            .is_some_and(|atom| atom == "CamelCase"),
        CoreJsonFunction::Default => true,
        CoreJsonFunction::SkipSerializingIf => {
            view.atom_text(payload)
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                .is_some_and(|atom| matches!(atom.as_str(), "None" | "False" | "Empty"))
                || view
                    .resolved_function_arity(payload)
                    .is_ok_and(|arity| arity == Some(1))
        }
        _ => unreachable!(),
    };
    if valid {
        return Ok(());
    }
    let message = match operation {
        CoreJsonFunction::Rename => "std/json.rename expects a String",
        CoreJsonFunction::RenameAll => "std/json.rename_all currently expects 'CamelCase",
        CoreJsonFunction::SkipSerializingIf => {
            "std/json.skip_serializing_if expects 'None, 'False, 'Empty, or a unary Func"
        }
        _ => unreachable!(),
    };
    Err(error(RuntimeErrorKind::TypeMismatch, message, function, pc))
}

struct JsonWriter<'a> {
    view: HeapView<'a>,
    indent: Option<usize>,
    output: String,
    active: HashSet<Handle>,
}

impl<'a> JsonWriter<'a> {
    fn new(view: HeapView<'a>, indent: Option<usize>) -> Self {
        Self {
            view,
            indent,
            output: String::new(),
            active: HashSet::new(),
        }
    }

    fn value(&mut self, value: Val, depth: usize) -> Result<(), String> {
        match value.value() {
            DecodedValue::Failed(_) => {
                return Err("JSON cannot encode a failed evaluation node".into());
            }
            DecodedValue::Int(value) => self.output.push_str(&value.to_string()),
            DecodedValue::Float(value) if value.is_finite() => {
                self.output.push_str(&value.to_string())
            }
            DecodedValue::Float(_) => return Err("JSON cannot encode a non-finite Float".into()),
            DecodedValue::BuiltinAtom(BuiltinAtom::None) => self.output.push_str("null"),
            DecodedValue::BuiltinAtom(BuiltinAtom::True) => self.output.push_str("true"),
            DecodedValue::BuiltinAtom(BuiltinAtom::False) => self.output.push_str("false"),
            DecodedValue::InlineString(text) => self.string(text.as_str()),
            DecodedValue::ShortString(id) => {
                self.string(self.view.text(id).map_err(|e| e.to_string())?)
            }
            DecodedValue::Array(handle) => self.array(handle, depth)?,
            DecodedValue::Dict(handle) => self.dict(handle, depth)?,
            DecodedValue::BuiltinAtom(atom) => {
                return Err(format!("JSON cannot encode '{}", atom.name()));
            }
            DecodedValue::InlineAtom(text) => {
                return Err(format!("JSON cannot encode '{}", text.as_str()));
            }
            DecodedValue::Atom(id) => {
                return Err(format!(
                    "JSON cannot encode '{}",
                    self.view.text(id).map_err(|e| e.to_string())?
                ));
            }
            DecodedValue::Bytes(_) => return Err("JSON cannot encode Bytes".into()),
            DecodedValue::Opaque(_) => return Err("JSON cannot encode Opaque values".into()),
            DecodedValue::NativeType(_) => return Err("JSON cannot encode Type values".into()),
            DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_) => {
                return Err("JSON cannot encode Type values".into());
            }
            DecodedValue::Tuple(_) => {
                return Err("JSON cannot encode Tuple; use a codec first".into());
            }
            DecodedValue::Tagged(_) => {
                return Err("JSON cannot encode Tagged; use a codec first".into());
            }
            DecodedValue::Func(_) => return Err("JSON cannot encode Func".into()),
            DecodedValue::FuncRef(_) => return Err("JSON cannot encode Func".into()),
            DecodedValue::Dyn(_) => return Err("JSON cannot encode Dyn".into()),
            DecodedValue::Module(_) => return Err("JSON cannot encode Module".into()),
            DecodedValue::TypeSlot(_) => {
                return Err("JSON cannot encode an internal up-link".into());
            }
        }
        Ok(())
    }

    fn array(&mut self, handle: Handle, depth: usize) -> Result<(), String> {
        if !self.active.insert(handle) {
            return Err("JSON cannot encode cyclic values".into());
        }
        let values = self
            .view
            .sequence(handle, false)
            .map_err(|e| e.to_string())?
            .to_vec();
        self.output.push('[');
        for (index, value) in values.into_iter().enumerate() {
            self.separator(index, depth + 1);
            self.value(value, depth + 1)?;
        }
        self.close_collection(values_len_hint(handle, &self.view, false)?, depth, ']');
        self.active.remove(&handle);
        Ok(())
    }

    fn dict(&mut self, handle: Handle, depth: usize) -> Result<(), String> {
        if !self.active.insert(handle) {
            return Err("JSON cannot encode cyclic values".into());
        }
        let (fields, values) = self.view.dict_parts(handle).map_err(|e| e.to_string())?;
        let entries = fields
            .iter()
            .zip(values)
            .map(|(field, value)| Ok((self.view.text(*field)?.to_owned(), *value)))
            .collect::<Result<Vec<_>, crate::heap::HeapError>>()
            .map_err(|e| e.to_string())?;
        self.output.push('{');
        for (index, (field, value)) in entries.iter().enumerate() {
            self.separator(index, depth + 1);
            self.string(field);
            self.output.push(':');
            if self.indent.is_some() {
                self.output.push(' ');
            }
            self.value(*value, depth + 1)?;
        }
        self.close_collection(entries.len(), depth, '}');
        self.active.remove(&handle);
        Ok(())
    }

    fn separator(&mut self, index: usize, depth: usize) {
        if index > 0 {
            self.output.push(',');
        }
        if let Some(indent) = self.indent {
            self.output.push('\n');
            self.output
                .extend(std::iter::repeat_n(' ', indent.saturating_mul(depth)));
        }
    }

    fn close_collection(&mut self, len: usize, depth: usize, close: char) {
        if len > 0
            && let Some(indent) = self.indent
        {
            self.output.push('\n');
            self.output
                .extend(std::iter::repeat_n(' ', indent.saturating_mul(depth)));
        }
        self.output.push(close);
    }

    fn string(&mut self, value: &str) {
        self.output.push('"');
        for character in value.chars() {
            match character {
                '"' => self.output.push_str("\\\""),
                '\\' => self.output.push_str("\\\\"),
                '\u{08}' => self.output.push_str("\\b"),
                '\u{0c}' => self.output.push_str("\\f"),
                '\n' => self.output.push_str("\\n"),
                '\r' => self.output.push_str("\\r"),
                '\t' => self.output.push_str("\\t"),
                c if c <= '\u{1f}' => {
                    let _ = write!(self.output, "\\u{:04x}", c as u32);
                }
                c => self.output.push(c),
            }
        }
        self.output.push('"');
    }
}

fn values_len_hint(handle: Handle, view: &HeapView<'_>, tuple: bool) -> Result<usize, String> {
    view.sequence(handle, tuple)
        .map(|values| values.len())
        .map_err(|e| e.to_string())
}

const DEBUG_MAX_DEPTH: usize = 8;
const DEBUG_MAX_ITEMS: usize = 32;
const DEBUG_MAX_BYTES: usize = 4_096;
fn run_core_diagnostic(
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &Arc<BytecodeFunction>,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let message = view
        .string_text(arguments[0])
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                function,
                pc,
            )
        })?
        .ok_or_else(|| runtime_type_error("String", &arguments[0], &view, function, pc))?;
    let message = message.as_str().to_owned();
    let subjects = match arguments[1].value() {
        DecodedValue::Tuple(handle) => view
            .sequence(handle, true)
            .map_err(|heap_error| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            })?
            .iter()
            .filter_map(|value| value.loc())
            .collect::<Vec<_>>(),
        _ => arguments[1].loc().into_iter().collect(),
    };
    let fallback = instruction_location(function, pc);
    let primary = fallback.or_else(|| subjects.first().copied());
    let mut diagnostic = primary.map_or_else(
        || Diagnostic {
            severity: crate::source::Severity::Warning,
            message: message.clone(),
            labels: Vec::new(),
            notes: Vec::new(),
        },
        |location| Diagnostic::new(crate::source::Severity::Warning, &message, location),
    );
    for related in subjects {
        if primary != Some(related) {
            diagnostic = diagnostic.with_secondary("related value", related);
        }
    }
    account.diagnostics.push(diagnostic);
    Ok(VmAction::Return {
        value: Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
        return_target,
    })
}

struct DebugValueFormatter<'a> {
    view: HeapView<'a>,
    output: String,
    active: HashSet<Handle>,
    truncated: bool,
}

impl<'a> DebugValueFormatter<'a> {
    fn new(view: HeapView<'a>) -> Self {
        Self {
            view,
            output: String::new(),
            active: HashSet::new(),
            truncated: false,
        }
    }

    fn format(mut self, value: Val) -> Result<String, crate::heap::HeapError> {
        self.value(value, 0)?;
        if self.truncated {
            self.output.push_str("...");
        }
        Ok(self.output)
    }

    fn value(&mut self, value: Val, depth: usize) -> Result<(), crate::heap::HeapError> {
        if self.truncated {
            return Ok(());
        }
        match value.value() {
            DecodedValue::Failed(_) => self.push("<failed>"),
            DecodedValue::Int(value) => self.push(&value.to_string()),
            DecodedValue::Float(value) => self.push(&format!("{value:?}")),
            DecodedValue::BuiltinAtom(atom) => {
                self.push("'");
                self.push(atom.name());
            }
            DecodedValue::InlineAtom(text) => {
                self.push("'");
                self.push(text.as_str());
            }
            DecodedValue::Atom(id) => {
                self.push("'");
                self.push(self.view.text(id)?);
            }
            DecodedValue::InlineString(text) => self.quoted(text.as_str()),
            DecodedValue::ShortString(id) => self.quoted(self.view.text(id)?),
            DecodedValue::Bytes(handle) => match self.view.object(handle)? {
                Object::Bytes(value) => {
                    self.push("b\"");
                    for byte in value.iter().take(DEBUG_MAX_ITEMS) {
                        self.push(&format!("\\x{byte:02x}"));
                    }
                    if value.len() > DEBUG_MAX_ITEMS {
                        self.push("...");
                    }
                    self.push("\"");
                }
                _ => return Err(crate::heap::HeapError::new("invalid Bytes handle")),
            },
            DecodedValue::Opaque(handle) => match self.view.object(handle)? {
                Object::Opaque(value) => self.push(&format!("{value:?}")),
                _ => return Err(crate::heap::HeapError::new("invalid Opaque handle")),
            },
            DecodedValue::NativeType(id) => {
                self.push("<type ");
                self.push(self.view.native_type(id)?.qualified_name());
                self.push(">");
            }
            DecodedValue::DeclaredType(handle) => match self.view.object(handle)? {
                Object::DeclaredType { type_id, name, .. } => {
                    let canonical_name = self.view.canonical_type_name(*type_id)?;
                    self.push("<type ");
                    self.push(canonical_name.as_deref().unwrap_or(name));
                    self.push(">");
                }
                _ => return Err(crate::heap::HeapError::new("invalid DeclaredType handle")),
            },
            DecodedValue::SymbolicType(handle) => match self.view.object(handle)? {
                Object::SymbolicType { name, .. } => {
                    self.push("<symbolic-type ");
                    self.push(name);
                    self.push(">");
                }
                _ => return Err(crate::heap::HeapError::new("invalid SymbolicType handle")),
            },
            DecodedValue::Array(handle) => self.sequence(handle, false, depth, "[", "]")?,
            DecodedValue::Tuple(handle) => self.sequence(handle, true, depth, "(", ")")?,
            DecodedValue::Tagged(handle) => {
                if !self.enter(handle, depth) {
                    return Ok(());
                }
                let (tag, payload) = self.view.tagged(handle)?;
                self.push("'");
                let tag = self
                    .view
                    .atom_text(tag)?
                    .ok_or_else(|| crate::heap::HeapError::new("Tagged tag is not an Atom"))?;
                self.push(tag.as_str());
                self.push("(");
                self.value(payload, depth + 1)?;
                self.push(")");
                self.active.remove(&handle);
            }
            DecodedValue::Dict(handle) => self.dict(handle, depth)?,
            DecodedValue::Func(handle) => {
                let (prototype, _) = self.view.closure(handle)?;
                let name = match prototype {
                    crate::heap::RuntimePrototype::Native(function) => function.name(),
                    crate::heap::RuntimePrototype::Bytecode(prototype) => {
                        self.view.bytecode(prototype)?.0.name()
                    }
                };
                self.push("<fn ");
                self.push(name);
                self.push(">");
            }
            DecodedValue::FuncRef(id) => {
                self.push("<fn-ref ");
                self.push(&format!("{}:{}", id.module.raw(), id.local));
                self.push(">");
            }
            DecodedValue::Dyn(_) => self.push("<dyn>"),
            DecodedValue::Module(_) => self.push("<module>"),
            DecodedValue::TypeSlot(handle) => {
                if !self.enter(handle, depth) {
                    return Ok(());
                }
                match self.view.type_slot(handle)? {
                    Some(value) => self.value(value, depth + 1)?,
                    None => self.push("<uninitialized up-link>"),
                }
                self.active.remove(&handle);
            }
        }
        Ok(())
    }

    fn sequence(
        &mut self,
        handle: Handle,
        tuple: bool,
        depth: usize,
        open: &str,
        close: &str,
    ) -> Result<(), crate::heap::HeapError> {
        if !self.enter(handle, depth) {
            return Ok(());
        }
        self.push(open);
        let (value_count, values) = {
            let sequence = self.view.sequence(handle, tuple)?;
            (
                sequence.len(),
                sequence
                    .iter()
                    .take(DEBUG_MAX_ITEMS)
                    .copied()
                    .collect::<Vec<_>>(),
            )
        };
        for (index, value) in values.iter().take(DEBUG_MAX_ITEMS).enumerate() {
            if index > 0 {
                self.push(", ");
            }
            self.value(*value, depth + 1)?;
        }
        if value_count > DEBUG_MAX_ITEMS {
            if DEBUG_MAX_ITEMS > 0 {
                self.push(", ");
            }
            self.push("...");
        }
        self.push(close);
        self.active.remove(&handle);
        Ok(())
    }

    fn dict(&mut self, handle: Handle, depth: usize) -> Result<(), crate::heap::HeapError> {
        if !self.enter(handle, depth) {
            return Ok(());
        }
        self.push("{");
        let (fields, values) = self.view.dict_parts(handle)?;
        let entries = fields
            .iter()
            .zip(values)
            .take(DEBUG_MAX_ITEMS)
            .map(|(field, value)| Ok((self.view.text(*field)?.to_owned(), *value)))
            .collect::<Result<Vec<_>, crate::heap::HeapError>>()?;
        for (index, (field, value)) in entries.into_iter().enumerate() {
            if index > 0 {
                self.push(", ");
            }
            self.push(&field);
            self.push(": ");
            self.value(value, depth + 1)?;
        }
        if values.len() > DEBUG_MAX_ITEMS {
            if DEBUG_MAX_ITEMS > 0 {
                self.push(", ");
            }
            self.push("...");
        }
        self.push("}");
        self.active.remove(&handle);
        Ok(())
    }

    fn enter(&mut self, handle: Handle, depth: usize) -> bool {
        if depth >= DEBUG_MAX_DEPTH {
            self.push("...");
            return false;
        }
        if !self.active.insert(handle) {
            self.push("<cycle>");
            return false;
        }
        true
    }

    fn quoted(&mut self, text: &str) {
        self.push(&format!("{text:?}"));
    }

    fn push(&mut self, text: &str) {
        if self.truncated {
            return;
        }
        let content_limit = DEBUG_MAX_BYTES.saturating_sub(3);
        for character in text.chars() {
            if self.output.len() + character.len_utf8() > content_limit {
                self.truncated = true;
                return;
            }
            self.output.push(character);
        }
    }
}

fn decimal_length(value: i64) -> usize {
    let magnitude = value.unsigned_abs();
    let digits = if magnitude == 0 {
        1
    } else {
        magnitude.ilog10() as usize + 1
    };
    digits + usize::from(value.is_negative())
}

fn read_register<'a>(
    registers: &'a [Option<Val>],
    register: Register,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<&'a Val, RuntimeError> {
    registers
        .get(register.0)
        .ok_or_else(|| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                format!("register {} is out of bounds", register.0),
                function,
                pc,
            )
        })?
        .as_ref()
        .ok_or_else(|| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                format!("register {} is uninitialized", register.0),
                function,
                pc,
            )
        })
}

fn write_register(
    registers: &mut [Option<Val>],
    register: Register,
    value: Val,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    let slot = registers.get_mut(register.0).ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            format!("register {} is out of bounds", register.0),
            function,
            pc,
        )
    })?;
    *slot = Some(value);
    Ok(())
}

fn read_many(
    registers: &[Option<Val>],
    items: &[Register],
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Vec<Val>, RuntimeError> {
    items
        .iter()
        .map(|register| read_register(registers, *register, function, pc).copied())
        .collect()
}

fn read_call_arguments(
    registers: &[Option<Val>],
    base: Register,
    argument_count: usize,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Vec<Val>, RuntimeError> {
    let start = base.0.checked_add(1).ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "call window overflows",
            function,
            pc,
        )
    })?;
    let end = start.checked_add(argument_count).ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "call window overflows",
            function,
            pc,
        )
    })?;
    let arguments = registers.get(start..end).ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "call window is out of bounds",
            function,
            pc,
        )
    })?;
    arguments
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_ref().copied().ok_or_else(|| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    format!("call argument register {} is uninitialized", start + index),
                    function,
                    pc,
                )
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
enum NumericOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Clone, Copy)]
enum BitwiseOperation {
    And,
    Or,
    Xor,
}

fn bitwise_binary(
    left: &Val,
    right: &Val,
    operation: BitwiseOperation,
    view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    let (DecodedValue::Int(left), DecodedValue::Int(right)) = (left.value(), right.value()) else {
        let invalid = if !matches!(left.value(), DecodedValue::Int(_)) {
            left
        } else {
            right
        };
        return Err(runtime_type_error("Int", invalid, view, function, pc));
    };
    let value = match operation {
        BitwiseOperation::And => left & right,
        BitwiseOperation::Or => left | right,
        BitwiseOperation::Xor => left ^ right,
    };
    Ok(DecodedValue::Int(value).into())
}

fn numeric_binary(
    left: &Val,
    right: &Val,
    operation: NumericOperation,
    view: &HeapView<'_>,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    match (left.value(), right.value()) {
        (DecodedValue::Int(left), DecodedValue::Int(right)) => {
            let value = match operation {
                NumericOperation::Add => left.checked_add(right),
                NumericOperation::Subtract => left.checked_sub(right),
                NumericOperation::Multiply => left.checked_mul(right),
                NumericOperation::Divide => left.checked_div(right),
                NumericOperation::Remainder => left.checked_rem(right),
            };
            let Some(value) = value else {
                let (kind, message) = match (operation, right) {
                    (NumericOperation::Divide, 0) => {
                        (RuntimeErrorKind::DivisionByZero, "integer division by zero")
                    }
                    (NumericOperation::Remainder, 0) => (
                        RuntimeErrorKind::DivisionByZero,
                        "integer remainder by zero",
                    ),
                    _ => (
                        RuntimeErrorKind::IntegerOverflow,
                        "integer arithmetic overflowed",
                    ),
                };
                return Err(error(kind, message, function, pc));
            };
            Ok(DecodedValue::Int(value).into())
        }
        (DecodedValue::Float(left_value), DecodedValue::Float(right_value)) => {
            let value = match operation {
                NumericOperation::Add => left_value + right_value,
                NumericOperation::Subtract => left_value - right_value,
                NumericOperation::Multiply => left_value * right_value,
                NumericOperation::Divide => left_value / right_value,
                NumericOperation::Remainder => left_value % right_value,
            };
            if !value.is_finite() {
                return Err(non_finite_float_error(account, left, right, function, pc));
            }
            Ok(DecodedValue::Float(value).into())
        }
        _ => Err(runtime_numeric_type_error(left, right, view, function, pc)),
    }
}

fn ordered_comparison(
    left: &Val,
    right: &Val,
    inclusive: bool,
    view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<bool, RuntimeError> {
    match (left.value(), right.value()) {
        (DecodedValue::Int(left), DecodedValue::Int(right)) => Ok(if inclusive {
            left <= right
        } else {
            left < right
        }),
        (DecodedValue::Float(left), DecodedValue::Float(right)) => Ok(if inclusive {
            left <= right
        } else {
            left < right
        }),
        (
            DecodedValue::InlineString(_) | DecodedValue::ShortString(_),
            DecodedValue::InlineString(_) | DecodedValue::ShortString(_),
        ) => {
            let left = view.string_text(*left).map_err(|heap_error| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            })?;
            let right = view.string_text(*right).map_err(|heap_error| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            })?;
            let (Some(left), Some(right)) = (left, right) else {
                unreachable!("String runtime values have text")
            };
            Ok(if inclusive {
                left.as_bytes() <= right.as_bytes()
            } else {
                left.as_bytes() < right.as_bytes()
            })
        }
        _ => Err(runtime_ordered_type_error(left, right, view, function, pc)),
    }
}

fn runtime_bool(value: bool) -> Val {
    DecodedValue::BuiltinAtom(if value {
        BuiltinAtom::True
    } else {
        BuiltinAtom::False
    })
    .into()
}

fn instruction_location(function: &BytecodeFunction, pc: usize) -> Option<crate::Loc> {
    match function.origin_at(pc) {
        Some(Origin::Source(location)) => Some(location),
        Some(Origin::Synthetic { derived_from }) => derived_from,
        None => None,
    }
}

fn runtime_type_error(
    expected: &str,
    actual: &Val,
    _view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    if let DecodedValue::Failed(failure) = actual.value() {
        return propagated_failure_error(failure, actual.loc(), function, pc);
    }
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        format!("expected {expected}, got {}", runtime_value_kind(*actual)),
        function,
        pc,
    );
    runtime_error.set_data_location(actual.loc());
    runtime_error
}

fn propagate_direct_failure(
    value: &Val,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    if let DecodedValue::Failed(failure) = value.value() {
        return Err(propagated_failure_error(failure, value.loc(), function, pc));
    }
    Ok(())
}

fn propagate_data_failures(
    values: &[Val],
    view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    for value in values {
        if let Some(failure) = view.first_data_failure(*value).map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                function,
                pc,
            )
        })? {
            return Err(propagated_failure_error(failure, value.loc(), function, pc));
        }
    }
    Ok(())
}

fn runtime_shallow_type_error(
    expected: &str,
    actual: Val,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    let location = actual.loc();
    if let DecodedValue::Failed(failure) = actual.value() {
        return propagated_failure_error(failure, location, function, pc);
    }
    let actual_kind = runtime_value_kind(actual);
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        format!("expected {expected}, got {actual_kind}"),
        function,
        pc,
    );
    runtime_error.set_data_location(location);
    runtime_error
}

fn runtime_value_kind(actual: Val) -> &'static str {
    match actual.value() {
        DecodedValue::Failed(_) => unreachable!(),
        DecodedValue::Int(_) => "Int",
        DecodedValue::Float(_) => "Float",
        DecodedValue::BuiltinAtom(_) | DecodedValue::InlineAtom(_) | DecodedValue::Atom(_) => {
            "Atom"
        }
        DecodedValue::InlineString(_) | DecodedValue::ShortString(_) => "String",
        DecodedValue::Bytes(_) => "Bytes",
        DecodedValue::Opaque(_) => "Opaque",
        DecodedValue::NativeType(_) => "Type",
        DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_) => "Type",
        DecodedValue::Array(_) => "Array",
        DecodedValue::Tuple(_) => "Tuple",
        DecodedValue::Tagged(_) => "Tagged",
        DecodedValue::Dict(_) => "Dict",
        DecodedValue::Func(_) => "Func",
        DecodedValue::FuncRef(_) => "Func",
        DecodedValue::Dyn(_) => "Dyn",
        DecodedValue::Module(_) => "Module",
        DecodedValue::TypeSlot(_) => "internal up-link",
    }
}

fn runtime_numeric_type_error(
    left: &Val,
    right: &Val,
    _view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    if let Some(failure) = [left, right]
        .into_iter()
        .find_map(|value| match value.value() {
            DecodedValue::Failed(failure) => Some(failure),
            _ => None,
        })
    {
        return propagated_failure_error(failure, left.loc().or(right.loc()), function, pc);
    }
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        format!(
            "numeric operands must have the same type, got {} and {}",
            runtime_value_kind(*left),
            runtime_value_kind(*right)
        ),
        function,
        pc,
    );
    runtime_error.set_data_location(left.loc().or(right.loc()));
    runtime_error
}

fn runtime_ordered_type_error(
    left: &Val,
    right: &Val,
    _view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    if let Some(failure) = [left, right]
        .into_iter()
        .find_map(|value| match value.value() {
            DecodedValue::Failed(failure) => Some(failure),
            _ => None,
        })
    {
        return propagated_failure_error(failure, left.loc().or(right.loc()), function, pc);
    }
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        format!(
            "ordered operands must be matching Int, Float, or String values, got {} and {}",
            runtime_value_kind(*left),
            runtime_value_kind(*right)
        ),
        function,
        pc,
    );
    runtime_error.set_data_location(left.loc().or(right.loc()));
    runtime_error
}

fn propagated_failure_error(
    failure: u32,
    location: Option<crate::Loc>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        "dependent computation received a failed evaluation node",
        function,
        pc,
    );
    runtime_error.propagated_failure = Some(failure);
    runtime_error.set_data_location(location);
    runtime_error
}

fn logical_value_bytes(count: usize) -> Result<u64, NativeError> {
    let count = u64::try_from(count)
        .map_err(|_| NativeError::allocation_limit("allocation item count overflowed"))?;
    let value_size = u64::try_from(std::mem::size_of::<Val>())
        .map_err(|_| NativeError::allocation_limit("Value size overflowed"))?;
    count
        .checked_mul(value_size)
        .ok_or_else(|| NativeError::allocation_limit("allocation size overflowed"))
}

fn allocation_error(
    message: impl Into<String>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    error(
        RuntimeErrorKind::AllocationQuotaExceeded,
        message,
        function,
        pc,
    )
}

fn out_of_range_error(
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    // Equivalent to allocating the two-subject Tuple and three-field BlameError
    // produced by fail!("OutOfRange", receiver, index).
    let bytes = logical_value_bytes(5)
        .and_then(|bytes| {
            bytes
                .checked_add(15) // "data", "message", and "rule"
                .ok_or_else(|| NativeError::allocation_limit("allocation size overflowed"))
        })
        .map_err(|native_error| allocation_error(native_error.message, function, pc))
        .and_then(|bytes| charge_allocation(account, bytes, function, pc));
    if let Err(error) = bytes {
        return error;
    }
    let location = instruction_location(function, pc);
    let mut runtime = error(RuntimeErrorKind::RaisedBlame, "OutOfRange", function, pc);
    runtime.set_locations(location, location);
    runtime
}

fn non_finite_float_error(
    account: &mut QuotaAccount,
    left: &Val,
    right: &Val,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    // Equivalent to allocating the two-subject Tuple and three-field BlameError
    // produced by fail!("NonFiniteFloat", left, right).
    let bytes = logical_value_bytes(5)
        .and_then(|bytes| {
            bytes
                .checked_add(15) // "data", "message", and "rule"
                .ok_or_else(|| NativeError::allocation_limit("allocation size overflowed"))
        })
        .map_err(|native_error| allocation_error(native_error.message, function, pc))
        .and_then(|bytes| charge_allocation(account, bytes, function, pc));
    if let Err(error) = bytes {
        return error;
    }
    let mut runtime = error(
        RuntimeErrorKind::RaisedBlame,
        "NonFiniteFloat",
        function,
        pc,
    );
    runtime.set_locations(
        left.loc().or(right.loc()),
        instruction_location(function, pc),
    );
    runtime
}

fn charge_allocation(
    account: &mut QuotaAccount,
    bytes: u64,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    account.charge_allocation(bytes).map_err(|()| {
        allocation_error(
            format!(
                "allocation quota of {} bytes exceeded",
                account.quota.allocation_bytes
            ),
            function,
            pc,
        )
    })
}

fn consume_fuel(
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    if let Some(query) = &account.query {
        query.check().map_err(|query_error| {
            error(
                RuntimeErrorKind::Cancelled,
                query_error.to_string(),
                function,
                pc,
            )
        })?;
    }
    if account.remaining_fuel == 0 {
        return Err(error(
            RuntimeErrorKind::FuelExhausted,
            "evaluation fuel exhausted",
            function,
            pc,
        ));
    }
    account.remaining_fuel -= 1;
    Ok(())
}

fn validate_jump(
    target: usize,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    if target >= function.instructions().len() {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            format!("jump target {target} is out of bounds"),
            function,
            pc,
        ));
    }
    Ok(())
}

fn error(
    kind: RuntimeErrorKind,
    message: impl Into<String>,
    function: &BytecodeFunction,
    instruction: usize,
) -> RuntimeError {
    RuntimeError {
        kind,
        message: message.into(),
        function: function.name().to_owned(),
        instruction,
        trace: vec![RuntimeFrame {
            function: function.name().to_owned(),
            instruction,
            origin: function.origin_at(instruction),
        }],
        locations: None,
        rendered: None,
        trace_includes_active_frame: true,
        propagated_failure: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::Constant;
    use crate::{Atom, BytecodeFunction, Instruction, NativeFunction, Register};

    fn run(
        vm: &mut Vm,
        registers: usize,
        constants: Vec<Constant>,
        instructions: Vec<Instruction>,
    ) -> Result<ExecutionWorld, RuntimeError> {
        vm.execute(
            &BytecodeFunction::new("test", registers, constants, instructions),
            1_000,
        )
    }

    #[test]
    fn semantic_value_measurement_does_not_report_invalid_graphs_as_quota_failures() {
        let current = Heap::work();
        let background = Heap::main();
        let error = semantic_codec_wrapper_bytes(
            &CodecNode::Tuple(Vec::new(), None),
            &current,
            &background,
        )
        .unwrap_err();
        assert_eq!(error.limit(), None);
        assert!(error.message.contains("unsupported semantic Value"));
    }

    #[test]
    fn executes_arithmetic_and_branching() {
        let result = run(
            &mut Vm::new(),
            4,
            vec![Constant::Int(20), Constant::Int(22), Constant::Int(0)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Add {
                    dst: Register(2),
                    left: Register(0),
                    right: Register(1),
                },
                Instruction::LoadConst {
                    dst: Register(3),
                    constant: 2,
                },
                Instruction::LessThan {
                    dst: Register(3),
                    left: Register(3),
                    right: Register(2),
                },
                Instruction::JumpIfFalse {
                    condition: Register(3),
                    target: 7,
                },
                Instruction::Return { src: Register(2) },
                Instruction::Return { src: Register(0) },
            ],
        )
        .unwrap();
        assert_eq!(result.value().as_int(), Some(42));
    }

    #[test]
    fn canonicalizes_and_interns_dict_shapes() {
        let result = run(
            &mut Vm::new(),
            4,
            vec![Constant::Int(1), Constant::Int(2)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::MakeDict {
                    dst: Register(2),
                    fields: vec![("b".into(), Register(1)), ("a".into(), Register(0))],
                },
                Instruction::MakeDict {
                    dst: Register(3),
                    fields: vec![("a".into(), Register(1)), ("b".into(), Register(0))],
                },
                Instruction::MakeTuple {
                    dst: Register(0),
                    items: vec![Register(2), Register(3)],
                },
                Instruction::Return { src: Register(0) },
            ],
        )
        .unwrap();
        let tuple = result.value();
        let left = tuple.sequence_get(0).expect("left Dict");
        let right = tuple.sequence_get(1).expect("right Dict");
        assert_eq!(left.dict_fields(), Some(vec!["a", "b"]));
        let (DecodedValue::Dict(left_handle), DecodedValue::Dict(right_handle)) =
            (left.value.value(), right.value.value())
        else {
            panic!("expected Dict values");
        };
        let Object::Dict {
            shape: left_shape, ..
        } = left.view.object(left_handle).unwrap()
        else {
            unreachable!()
        };
        let Object::Dict {
            shape: right_shape, ..
        } = right.view.object(right_handle).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(left_shape, right_shape);
        assert_eq!(left.dict_get("a").unwrap().as_int(), Some(1));
    }

    #[test]
    fn constructs_and_reads_structured_values() {
        let result = run(
            &mut Vm::new(),
            5,
            vec![
                Constant::Atom(Atom::builtin(BuiltinAtom::Ok)),
                Constant::Int(42),
            ],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::MakeTuple {
                    dst: Register(2),
                    items: vec![Register(0), Register(1)],
                },
                Instruction::MakeArray {
                    dst: Register(3),
                    items: vec![Register(1), Register(2)],
                },
                Instruction::MakeDict {
                    dst: Register(4),
                    fields: vec![("result".into(), Register(3))],
                },
                Instruction::GetField {
                    dst: Register(0),
                    dict: Register(4),
                    field: "result".into(),
                },
                Instruction::Return { src: Register(0) },
            ],
        )
        .unwrap();
        assert_eq!(result.to_string(), "[42, ('Ok, 42)]");
    }

    #[test]
    fn reports_integer_errors_consistently() {
        let overflow = run(
            &mut Vm::new(),
            3,
            vec![Constant::Int(i64::MAX), Constant::Int(1)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Add {
                    dst: Register(2),
                    left: Register(0),
                    right: Register(1),
                },
                Instruction::Return { src: Register(2) },
            ],
        )
        .unwrap_err();
        assert_eq!(overflow.kind, RuntimeErrorKind::IntegerOverflow);

        let division = run(
            &mut Vm::new(),
            3,
            vec![Constant::Int(1), Constant::Int(0)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Divide {
                    dst: Register(2),
                    left: Register(0),
                    right: Register(1),
                },
                Instruction::Return { src: Register(2) },
            ],
        )
        .unwrap_err();
        assert_eq!(division.kind, RuntimeErrorKind::DivisionByZero);
    }

    #[test]
    fn runtime_error_recoverability_is_typed_and_exhaustive() {
        use crate::evaluation::FailureClass;

        let function = BytecodeFunction::new("classification", 0, vec![], vec![]);
        for kind in [
            RuntimeErrorKind::DivisionByZero,
            RuntimeErrorKind::IntegerOverflow,
            RuntimeErrorKind::MissingField,
            RuntimeErrorKind::NoPatternMatched,
            RuntimeErrorKind::TypeMismatch,
            RuntimeErrorKind::UninitializedDefinition,
            RuntimeErrorKind::DuplicateDefinition,
        ] {
            assert_eq!(
                error(kind, "recoverable", &function, 0).failure_class(),
                FailureClass::Recoverable
            );
        }
        for kind in [
            RuntimeErrorKind::Cancelled,
            RuntimeErrorKind::FuelExhausted,
            RuntimeErrorKind::AllocationQuotaExceeded,
            RuntimeErrorKind::CallDepthExceeded,
            RuntimeErrorKind::InvalidBytecode,
            RuntimeErrorKind::StackLimitExceeded,
        ] {
            assert_eq!(
                error(kind, "terminal", &function, 0).failure_class(),
                FailureClass::Terminal
            );
        }
    }

    #[test]
    fn rejects_non_boolean_conditions() {
        let error = run(
            &mut Vm::new(),
            1,
            vec![Constant::Int(1)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::JumpIfFalse {
                    condition: Register(0),
                    target: 2,
                },
                Instruction::Return { src: Register(0) },
            ],
        )
        .unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
    }

    #[test]
    fn enforces_fuel_and_rejects_malformed_bytecode() {
        let loop_function =
            BytecodeFunction::new("loop", 0, vec![], vec![Instruction::Jump { target: 0 }]);
        let error = Vm::new().execute(&loop_function, 5).unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::FuelExhausted);

        let invalid = BytecodeFunction::new(
            "invalid",
            0,
            vec![],
            vec![Instruction::Return { src: Register(9) }],
        );
        let error = Vm::new().execute(&invalid, 5).unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::InvalidBytecode);

        let invalid_call_window = BytecodeFunction::new(
            "invalid-call-window",
            1,
            vec![Constant::Native(NativeFunction::new(
                "identity",
                1,
                native_identity,
            ))],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::TailCall {
                    base: Register(0),
                    argument_count: 1,
                },
            ],
        );
        let error = Vm::new().execute(&invalid_call_window, 5).unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::InvalidBytecode);
    }

    #[test]
    fn straight_line_and_forward_control_flow_need_no_fuel() {
        let straight = BytecodeFunction::new(
            "straight",
            1,
            vec![Constant::Int(42)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&straight, 0).unwrap().value().as_int(),
            Some(42)
        );

        let forward = BytecodeFunction::new(
            "forward",
            1,
            vec![Constant::Int(42)],
            vec![
                Instruction::Jump { target: 2 },
                Instruction::Fail {
                    message: "skipped".into(),
                },
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&forward, 0).unwrap().value().as_int(),
            Some(42)
        );
    }

    #[test]
    fn only_taken_back_edges_consume_fuel() {
        let untaken = BytecodeFunction::new(
            "untaken",
            1,
            vec![Constant::Atom(Atom::builtin(BuiltinAtom::True))],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::JumpIfFalse {
                    condition: Register(0),
                    target: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert!(Vm::new().execute(&untaken, 0).is_ok());

        let one_back_edge = BytecodeFunction::new(
            "one-back-edge",
            1,
            vec![
                Constant::Atom(Atom::builtin(BuiltinAtom::False)),
                Constant::Atom(Atom::builtin(BuiltinAtom::True)),
            ],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Jump { target: 3 },
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 1,
                },
                Instruction::JumpIfFalse {
                    condition: Register(0),
                    target: 2,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        let exhausted = Vm::new().execute(&one_back_edge, 0).unwrap_err();
        assert_eq!(exhausted.kind, RuntimeErrorKind::FuelExhausted);
        assert!(Vm::new().execute(&one_back_edge, 1).is_ok());
    }

    #[test]
    fn bytecode_and_native_calls_each_consume_one_fuel() {
        let callee = Arc::new(BytecodeFunction::new(
            "callee",
            1,
            vec![Constant::Int(42)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        ));
        let bytecode = BytecodeFunction::new(
            "bytecode-call",
            2,
            vec![],
            vec![
                Instruction::MakeClosure {
                    dst: Register(0),
                    function: callee,
                    captures: vec![],
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&bytecode, 0).unwrap_err().kind,
            RuntimeErrorKind::FuelExhausted
        );
        assert!(Vm::new().execute(&bytecode, 1).is_ok());

        let nested = BytecodeFunction::new(
            "nested-call",
            2,
            vec![],
            vec![
                Instruction::MakeClosure {
                    dst: Register(0),
                    function: Arc::new(bytecode),
                    captures: vec![],
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&nested, 1).unwrap_err().kind,
            RuntimeErrorKind::FuelExhausted
        );
        assert!(Vm::new().execute(&nested, 2).is_ok());

        let native = NativeFunction::new("identity", 1, native_identity);
        let native = BytecodeFunction::new(
            "native-call",
            3,
            vec![Constant::Native(native), Constant::Int(2)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 1,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&native, 0).unwrap_err().kind,
            RuntimeErrorKind::FuelExhausted
        );
        assert!(Vm::new().execute(&native, 1).is_ok());
    }

    fn native_identity(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
        let value = context
            .value(context.argument(0)?)?
            .as_int()
            .ok_or_else(|| NativeError::new("expected Int argument"))?;
        context.set_int(context.result(), value)
    }

    fn native_non_finite_float(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
        context.set_float(context.result(), f64::INFINITY)
    }

    #[test]
    fn native_non_finite_float_result_raises_blame_at_the_call() {
        let native = NativeFunction::new("non_finite_float", 0, native_non_finite_float);
        let function = BytecodeFunction::new(
            "native-float-call",
            1,
            vec![Constant::Native(native)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        let error = Vm::new().execute(&function, 1).unwrap_err();
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "NonFiniteFloat");
    }

    #[test]
    fn tail_calls_native_functions_and_replace_bytecode_frames() {
        let native = NativeFunction::new("identity", 1, native_identity);
        let native_tail = BytecodeFunction::new(
            "native-tail",
            2,
            vec![Constant::Native(native), Constant::Int(42)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::TailCall {
                    base: Register(0),
                    argument_count: 1,
                },
            ],
        );
        assert_eq!(
            Vm::new().execute(&native_tail, 0).unwrap_err().kind,
            RuntimeErrorKind::FuelExhausted
        );
        assert_eq!(
            Vm::new().execute(&native_tail, 1).unwrap().value().as_int(),
            Some(42)
        );

        let large = Arc::new(BytecodeFunction::new(
            "large-frame",
            100,
            vec![Constant::Int(7)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        ));
        let replace = BytecodeFunction::new(
            "small-frame",
            1,
            vec![],
            vec![
                Instruction::MakeClosure {
                    dst: Register(0),
                    function: large,
                    captures: vec![],
                },
                Instruction::TailCall {
                    base: Register(0),
                    argument_count: 0,
                },
            ],
        );
        assert_eq!(
            Vm::new()
                .execute_with_quota(&replace, Quota::new(1, 100, u64::MAX))
                .unwrap()
                .value()
                .as_int(),
            Some(7)
        );
        assert_eq!(
            Vm::new()
                .execute_with_quota(&replace, Quota::new(1, 99, u64::MAX))
                .unwrap_err()
                .kind,
            RuntimeErrorKind::StackLimitExceeded
        );
    }

    #[test]
    fn native_closures_use_register_context() {
        let native = NativeFunction::new("identity", 1, native_identity);
        let function = BytecodeFunction::new(
            "test",
            3,
            vec![Constant::Native(native), Constant::Int(2)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Call {
                    base: Register(0),
                    argument_count: 1,
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        assert_eq!(
            Vm::new().execute(&function, 20).unwrap().value().as_int(),
            Some(2)
        );
    }

    #[test]
    fn nested_calls_use_explicit_vm_frames() {
        let mut function = Arc::new(BytecodeFunction::new(
            "leaf",
            1,
            vec![Constant::Int(7)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        ));
        for depth in 0..512 {
            function = Arc::new(BytecodeFunction::new(
                format!("frame{depth}"),
                2,
                vec![],
                vec![
                    Instruction::MakeClosure {
                        dst: Register(0),
                        function,
                        captures: vec![],
                    },
                    Instruction::Call {
                        base: Register(0),
                        argument_count: 0,
                    },
                    Instruction::Return { src: Register(0) },
                ],
            ));
        }
        assert_eq!(
            Vm::new()
                .execute(&function, 2_000)
                .unwrap()
                .value()
                .as_int(),
            Some(7)
        );
    }

    #[test]
    fn enforces_independent_call_depth_and_stack_slot_limits() {
        let mut function = Arc::new(BytecodeFunction::new(
            "leaf",
            1,
            vec![Constant::Int(7)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::Return { src: Register(0) },
            ],
        ));
        for _ in 0..MAX_CALL_DEPTH {
            function = Arc::new(BytecodeFunction::new(
                "recursive-shape",
                2,
                vec![],
                vec![
                    Instruction::MakeClosure {
                        dst: Register(0),
                        function,
                        captures: vec![],
                    },
                    Instruction::Call {
                        base: Register(0),
                        argument_count: 0,
                    },
                    Instruction::Return { src: Register(0) },
                ],
            ));
        }
        let depth = Vm::new().execute(&function, usize::MAX).unwrap_err();
        assert_eq!(depth.kind, RuntimeErrorKind::CallDepthExceeded);

        let oversized = BytecodeFunction::new(
            "oversized",
            MAX_STACK_SLOTS + 1,
            vec![],
            vec![Instruction::Return { src: Register(0) }],
        );
        let stack = Vm::new().execute(&oversized, usize::MAX).unwrap_err();
        assert_eq!(stack.kind, RuntimeErrorKind::StackLimitExceeded);
    }

    #[test]
    fn trace_does_not_deduplicate_equal_function_names_and_pcs() {
        let leaf = Arc::new(BytecodeFunction::new(
            "same",
            3,
            vec![Constant::Int(1), Constant::Int(0)],
            vec![
                Instruction::LoadConst {
                    dst: Register(0),
                    constant: 0,
                },
                Instruction::LoadConst {
                    dst: Register(1),
                    constant: 1,
                },
                Instruction::Divide {
                    dst: Register(2),
                    left: Register(0),
                    right: Register(1),
                },
                Instruction::Return { src: Register(2) },
            ],
        ));
        let mut function = leaf;
        for _ in 0..2 {
            function = Arc::new(BytecodeFunction::new(
                "same",
                2,
                vec![],
                vec![
                    Instruction::MakeClosure {
                        dst: Register(0),
                        function,
                        captures: vec![],
                    },
                    Instruction::Call {
                        base: Register(0),
                        argument_count: 0,
                    },
                    Instruction::Return { src: Register(0) },
                ],
            ));
        }
        let error = Vm::new().execute(&function, 100).unwrap_err();
        assert_eq!(error.trace.len(), 3);
        assert!(error.trace.iter().all(|frame| frame.function == "same"));
    }

    #[test]
    fn dict_allocation_charge_does_not_depend_on_shape_cache_hits() {
        let function = crate::compile_source("test", "{answer: 42}")
            .unwrap()
            .into_function();
        let mut vm = Vm::new();
        let mut account = QuotaAccount::new(Quota::new(0, 100, u64::MAX));
        vm.execute_with_account(&function, &[], &mut account)
            .unwrap();
        let first = account.requested_allocation_bytes();
        vm.execute_with_account(&function, &[], &mut account)
            .unwrap();
        let second = account.requested_allocation_bytes() - first;
        assert_eq!(first, second);
        assert!(first > 0);
    }

    #[test]
    fn debug_formatter_is_cycle_safe_and_bounded() {
        let background = Heap::main();
        let mut current = Heap::work();
        let cycle = current.reserve();
        current
            .initialize(
                cycle,
                Object::Array(vec![Val::unknown(DecodedValue::Array(cycle))].into()),
            )
            .unwrap();
        let cycle_text = DebugValueFormatter::new(HeapView {
            current: &current,
            background: Some(&background),
        })
        .format(DecodedValue::Array(cycle).into())
        .unwrap();
        assert_eq!(cycle_text, "[<cycle>]");

        let long = current.string(None, &"x".repeat(DEBUG_MAX_BYTES * 2));
        let long_text = DebugValueFormatter::new(HeapView {
            current: &current,
            background: Some(&background),
        })
        .format(long.into())
        .unwrap();
        assert_eq!(long_text.len(), DEBUG_MAX_BYTES);
        assert!(long_text.ends_with("..."));

        let bytes = DecodedValue::Bytes(current.allocate(Object::Bytes(
            (0..64).map(|value| value as u8).collect::<Vec<_>>().into(),
        )));
        let bytes_text = DebugValueFormatter::new(HeapView {
            current: &current,
            background: Some(&background),
        })
        .format(bytes.into())
        .unwrap();
        assert!(bytes_text.starts_with("b\"\\x00\\x01"));
        assert!(bytes_text.contains("..."));
    }

    #[test]
    fn json_writer_rejects_internal_cycles() {
        let background = Heap::main();
        let mut current = Heap::work();
        let cycle = current.reserve();
        current
            .initialize(
                cycle,
                Object::Array(vec![Val::unknown(DecodedValue::Array(cycle))].into()),
            )
            .unwrap();
        let mut writer = JsonWriter::new(
            HeapView {
                current: &current,
                background: Some(&background),
            },
            None,
        );
        assert_eq!(
            writer
                .value(DecodedValue::Array(cycle).into(), 0)
                .unwrap_err(),
            "JSON cannot encode cyclic values"
        );
    }

    #[test]
    fn reducer_transition_audits_the_complete_effect_batch() {
        let background = Heap::main();
        let mut heap = Heap::work();
        let failed_payload = Val::unknown(DecodedValue::Array(heap.allocate(Object::Array(
            vec![Val::unknown(DecodedValue::Failed(0))].into(),
        ))));
        let effects = Val::unknown(DecodedValue::Array(heap.allocate(Object::Array(
            vec![Val::unknown(DecodedValue::Int(1)), failed_payload].into(),
        ))));
        let root = Val::unknown(DecodedValue::Tuple(heap.allocate(Object::Tuple(
            vec![Val::unknown(DecodedValue::Int(0)), effects].into(),
        ))));
        let error = match (WorkWorld { heap, root }).into_reducer_transition(&background) {
            Ok(_) => panic!("failed effect batch crossed the Host boundary"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "failed evaluation node cannot cross the SystemEffect boundary"
        );
    }
}
