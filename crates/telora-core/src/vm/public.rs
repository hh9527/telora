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

    pub fn into_semantic_json(mut self) -> Result<String, String> {
        let owner = HeapView {
            current: &self.work.heap,
            background: Some(&self.main),
        }
            .type_witness(self.work.root)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "eval result must be std/value.Value".to_owned())?;
        let raw = unwrap_semantic_value(
            &mut self.work.heap,
            Some(&self.main),
            self.work.root,
            owner,
        )
        .map_err(|error| error.to_string())?;
        let view = HeapView {
            current: &self.work.heap,
            background: Some(&self.main),
        };
        let mut writer = JsonWriter::new(view, None);
        writer.value(raw, 0)?;
        Ok(writer.output)
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

    pub(crate) fn local(value: Val, heap: &'a Heap) -> Self {
        Self {
            value,
            view: HeapView {
                current: heap,
                background: None,
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

    pub(crate) fn declared_type_id(self) -> Option<crate::TypeId> {
        self.view.declared_type_id(self.value).ok()
    }

    pub(crate) fn type_property(self, property: crate::TypeId) -> Option<ValueRef<'a>> {
        let target = self.declared_type_id()?;
        self.view
            .type_property(target, property)
            .map(|value| ValueRef {
                value,
                view: self.view,
            })
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
