use crate::json::{SourcedValue, ValuePath, ValuePathSegment};
use crate::source::Loc;
use crate::value::DynValue;
use crate::{
    Atom, BuiltinAtom, BytecodeFunction, Closure, Dict, FuncByteCode, NativeFunction, Prototype,
    Shape, Value,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};

const SHORT_TEXT_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Storage {
    Work,
    Main,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Handle {
    storage: Storage,
    slot: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InternId {
    storage: Storage,
    slot: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ShapeId {
    storage: Storage,
    slot: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RuntimeValue {
    Failed(u32),
    Int(i64),
    Float(f64),
    BuiltinAtom(BuiltinAtom),
    Atom(InternId),
    ShortString(InternId),
    String(Handle),
    Bytes(Handle),
    NativeType(Handle),
    Opaque(Handle),
    Array(Handle),
    Tuple(Handle),
    Tagged(Handle),
    Dict(Handle),
    Func(Handle),
    Dyn(Handle),
    UpLink(Handle),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RichValue {
    pub(crate) value: RuntimeValue,
    provenance: Provenance,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Provenance {
    Unknown,
    Original(Loc),
    Generated(Loc),
}

impl RichValue {
    pub(crate) const fn new(value: RuntimeValue, loc: Option<Loc>) -> Self {
        Self {
            value,
            provenance: match loc {
                Some(loc) => Provenance::Generated(loc),
                None => Provenance::Unknown,
            },
        }
    }

    pub(crate) const fn original(value: RuntimeValue, loc: Option<Loc>) -> Self {
        Self {
            value,
            provenance: match loc {
                Some(loc) => Provenance::Original(loc),
                None => Provenance::Unknown,
            },
        }
    }

    pub(crate) const fn unknown(value: RuntimeValue) -> Self {
        Self::new(value, None)
    }

    pub(crate) fn with_loc(self, loc: Option<Loc>) -> Self {
        if self.loc() == loc {
            self
        } else {
            Self::new(self.value, loc)
        }
    }

    pub(crate) const fn rebase_generated(self, call_site: Option<Loc>) -> Self {
        match self.provenance {
            Provenance::Original(_) => self,
            Provenance::Unknown | Provenance::Generated(_) => Self::new(self.value, call_site),
        }
    }

    pub(crate) const fn loc(self) -> Option<Loc> {
        match self.provenance {
            Provenance::Unknown => None,
            Provenance::Original(loc) | Provenance::Generated(loc) => Some(loc),
        }
    }

    pub(crate) const fn with_value(self, value: RuntimeValue) -> Self {
        Self { value, ..self }
    }

    #[cfg(test)]
    const fn is_original(self) -> bool {
        matches!(self.provenance, Provenance::Original(_))
    }
}

impl PartialEq for RichValue {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl From<RuntimeValue> for RichValue {
    fn from(value: RuntimeValue) -> Self {
        Self::unknown(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PersistentValue(RichValue);

impl PersistentValue {
    pub(crate) fn dict_get(self, heap: &Heap, name: &str) -> Result<Option<Self>, HeapError> {
        if heap.storage != Storage::Main {
            return Err(HeapError("persistent values require a Main world"));
        }
        let RuntimeValue::Dict(handle) = self.0.value else {
            return Err(HeapError("persistent value is not a Dict"));
        };
        let Object::Dict { shape, values } = heap.object(handle)? else {
            return Err(HeapError("persistent Dict handle has another object kind"));
        };
        for (field, value) in heap.shape(*shape)?.iter().zip(values) {
            if heap.resolve_text(*field)? == name {
                return Ok(Some(Self(*value)));
            }
        }
        Ok(None)
    }
}

impl PersistentValue {
    pub(crate) const fn runtime(self) -> RichValue {
        self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RuntimePrototype {
    Bytecode(Handle),
    Native(NativeFunction),
}

#[derive(Clone, Debug)]
pub(crate) enum Object {
    Reserved,
    String(Box<str>),
    Bytes(Box<[u8]>),
    NativeType(crate::NativeType),
    Opaque(crate::value::OpaqueValue),
    Array(Box<[RichValue]>),
    Tuple(Box<[RichValue]>),
    Tagged {
        tag: RichValue,
        payload: RichValue,
    },
    Dict {
        shape: ShapeId,
        values: Box<[RichValue]>,
    },
    Closure {
        identity: Arc<()>,
        prototype: RuntimePrototype,
        upvalues: Box<[RichValue]>,
    },
    Dyn {
        identity: Arc<()>,
        descriptor: RichValue,
        value: RichValue,
        scheme: Option<crate::TypeScheme>,
        origin: Option<Arc<str>>,
    },
    UpLink {
        value: Option<RichValue>,
    },
    ByteCodeProto {
        code: Arc<FuncByteCode>,
        values: Box<[RichValue]>,
        text: Box<[InternId]>,
        prototypes: Box<[RuntimePrototype]>,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct HeapError(&'static str);

impl HeapError {
    pub(crate) const fn new(message: &'static str) -> Self {
        Self(message)
    }

    pub(crate) fn is_legacy_cycle(&self) -> bool {
        matches!(
            self.0,
            "cyclic heap values cannot cross the legacy Value boundary"
        )
    }
}

impl fmt::Display for HeapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

#[derive(Default)]
struct TextTable {
    values: Vec<Arc<str>>,
    slots: HashMap<Arc<str>, u32>,
}

impl TextTable {
    fn find(&self, text: &str) -> Option<u32> {
        self.slots.get(text).copied()
    }

    fn resolve(&self, slot: u32) -> Option<&str> {
        self.values.get(slot as usize).map(AsRef::as_ref)
    }

    fn insert(&mut self, text: &str) -> u32 {
        if let Some(slot) = self.find(text) {
            return slot;
        }
        let slot = self.values.len() as u32;
        let value: Arc<str> = text.into();
        self.values.push(value.clone());
        self.slots.insert(value, slot);
        slot
    }
}

pub(crate) struct Heap {
    storage: Storage,
    objects: Vec<Object>,
    text: TextTable,
    shapes: Vec<Box<[InternId]>>,
    shape_slots: HashMap<Vec<InternId>, u32>,
    exported_shapes: Mutex<HashMap<u32, Arc<Shape>>>,
}

impl Heap {
    fn new(storage: Storage) -> Self {
        Self {
            storage,
            objects: Vec::new(),
            text: TextTable::default(),
            shapes: Vec::new(),
            shape_slots: HashMap::new(),
            exported_shapes: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn work() -> Self {
        Self::new(Storage::Work)
    }

    pub(crate) fn main() -> Self {
        Self::new(Storage::Main)
    }

    #[cfg(test)]
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        (
            self.objects.len(),
            self.text.values.len(),
            self.shapes.len(),
        )
    }

    pub(crate) fn allocate(&mut self, object: Object) -> Handle {
        let handle = Handle {
            storage: self.storage,
            slot: self.objects.len() as u32,
        };
        self.objects.push(object);
        handle
    }

    pub(crate) fn export_persistent(&self, value: PersistentValue) -> Result<Value, HeapError> {
        HeapView {
            current: self,
            background: None,
        }
        .export_value(value.runtime())
    }

    pub(crate) fn export_persistent_projecting_up_links(
        &self,
        value: PersistentValue,
        projection: &Value,
    ) -> Result<Value, HeapError> {
        HeapView {
            current: self,
            background: None,
        }
        .export_value_projecting_up_links(value.runtime(), projection)
    }

    pub(crate) fn persistent_contains_up_link(
        &self,
        value: PersistentValue,
    ) -> Result<bool, HeapError> {
        let mut pending = vec![value.runtime().value];
        let mut visited = HashSet::new();
        while let Some(value) = pending.pop() {
            let handle = match value {
                RuntimeValue::Failed(_) => continue,
                RuntimeValue::UpLink(_) => return Ok(true),
                RuntimeValue::String(handle)
                | RuntimeValue::Bytes(handle)
                | RuntimeValue::NativeType(handle)
                | RuntimeValue::Opaque(handle)
                | RuntimeValue::Array(handle)
                | RuntimeValue::Tuple(handle)
                | RuntimeValue::Tagged(handle)
                | RuntimeValue::Dict(handle)
                | RuntimeValue::Func(handle)
                | RuntimeValue::Dyn(handle) => handle,
                RuntimeValue::Int(_)
                | RuntimeValue::Float(_)
                | RuntimeValue::BuiltinAtom(_)
                | RuntimeValue::Atom(_)
                | RuntimeValue::ShortString(_) => continue,
            };
            if !visited.insert(handle) {
                continue;
            }
            match self.object(handle)? {
                Object::Array(values) | Object::Tuple(values) => {
                    pending.extend(values.iter().map(|value| value.value));
                }
                Object::Tagged { tag, payload } => {
                    pending.push(tag.value);
                    pending.push(payload.value);
                }
                Object::Dict { values, .. } => {
                    pending.extend(values.iter().map(|value| value.value));
                }
                Object::Closure {
                    prototype,
                    upvalues,
                    ..
                } => {
                    pending.extend(upvalues.iter().map(|value| value.value));
                    if let RuntimePrototype::Bytecode(handle) = prototype {
                        pending.push(RuntimeValue::Func(*handle));
                    }
                }
                Object::Dyn {
                    descriptor, value, ..
                } => {
                    pending.push(descriptor.value);
                    pending.push(value.value);
                }
                Object::ByteCodeProto {
                    values, prototypes, ..
                } => {
                    pending.extend(values.iter().map(|value| value.value));
                    pending.extend(prototypes.iter().filter_map(|prototype| match prototype {
                        RuntimePrototype::Bytecode(handle) => Some(RuntimeValue::Func(*handle)),
                        RuntimePrototype::Native(_) => None,
                    }));
                }
                Object::Reserved => return Err(HeapError("heap object is uninitialized")),
                Object::UpLink { .. } => return Ok(true),
                Object::String(_)
                | Object::Bytes(_)
                | Object::NativeType(_)
                | Object::Opaque(_) => {}
            }
        }
        Ok(false)
    }

    pub(crate) fn reserve(&mut self) -> Handle {
        self.allocate(Object::Reserved)
    }

    pub(crate) fn initialize(&mut self, handle: Handle, object: Object) -> Result<(), HeapError> {
        let slot = self.object_mut(handle)?;
        if !matches!(slot, Object::Reserved) {
            return Err(HeapError("heap slot is already initialized"));
        }
        *slot = object;
        Ok(())
    }

    pub(crate) fn initialize_up_link(
        &mut self,
        handle: Handle,
        value: RichValue,
    ) -> Result<(), HeapError> {
        if handle.storage != Storage::Work {
            return Err(HeapError("Main up-links are read-only"));
        }
        let Object::UpLink { value: slot } = self.object_mut(handle)? else {
            return Err(HeapError("handle is not an up-link"));
        };
        if slot.is_some() {
            return Err(HeapError("up-link is already initialized"));
        }
        *slot = Some(value);
        Ok(())
    }

    pub(crate) fn object(&self, handle: Handle) -> Result<&Object, HeapError> {
        if handle.storage != self.storage {
            return Err(HeapError("object handle belongs to another heap"));
        }
        self.objects
            .get(handle.slot as usize)
            .ok_or(HeapError("object handle is out of bounds"))
    }

    fn object_mut(&mut self, handle: Handle) -> Result<&mut Object, HeapError> {
        if handle.storage != self.storage {
            return Err(HeapError("object handle belongs to another heap"));
        }
        self.objects
            .get_mut(handle.slot as usize)
            .ok_or(HeapError("object handle is out of bounds"))
    }

    pub(crate) fn intern(&mut self, text: &str) -> InternId {
        InternId {
            storage: self.storage,
            slot: self.text.insert(text),
        }
    }

    pub(crate) fn find_text(&self, text: &str) -> Option<InternId> {
        self.text.find(text).map(|slot| InternId {
            storage: self.storage,
            slot,
        })
    }

    pub(crate) fn resolve_text(&self, id: InternId) -> Result<&str, HeapError> {
        if id.storage != self.storage {
            return Err(HeapError("intern ID belongs to another heap"));
        }
        self.text
            .resolve(id.slot)
            .ok_or(HeapError("intern ID is out of bounds"))
    }

    pub(crate) fn string(&mut self, background: Option<&Heap>, text: &str) -> RuntimeValue {
        if text.len() <= SHORT_TEXT_BYTES {
            if let Some(id) = background.and_then(|heap| heap.find_text(text)) {
                RuntimeValue::ShortString(id)
            } else {
                RuntimeValue::ShortString(self.intern(text))
            }
        } else {
            RuntimeValue::String(self.allocate(Object::String(text.into())))
        }
    }

    pub(crate) fn atom(&mut self, background: Option<&Heap>, text: &str) -> RuntimeValue {
        if let Some(builtin) = builtin_atom(text) {
            RuntimeValue::BuiltinAtom(builtin)
        } else if let Some(id) = background.and_then(|heap| heap.find_text(text)) {
            RuntimeValue::Atom(id)
        } else {
            RuntimeValue::Atom(self.intern(text))
        }
    }

    pub(crate) fn intern_shape(&mut self, fields: Vec<InternId>) -> ShapeId {
        if let Some(slot) = self.shape_slots.get(&fields) {
            return ShapeId {
                storage: self.storage,
                slot: *slot,
            };
        }
        let slot = self.shapes.len() as u32;
        self.shapes.push(fields.clone().into());
        self.shape_slots.insert(fields, slot);
        ShapeId {
            storage: self.storage,
            slot,
        }
    }

    fn shape(&self, id: ShapeId) -> Result<&[InternId], HeapError> {
        if id.storage != self.storage {
            return Err(HeapError("shape ID belongs to another heap"));
        }
        self.shapes
            .get(id.slot as usize)
            .map(AsRef::as_ref)
            .ok_or(HeapError("shape ID is out of bounds"))
    }

    pub(crate) fn import_value(
        &mut self,
        background: Option<&Heap>,
        value: &Value,
    ) -> Result<RichValue, HeapError> {
        let mut prototypes = HashMap::new();
        self.import_value_with(background, value, &HashMap::new(), &mut prototypes, None)
    }

    pub(crate) fn import_sourced_value(
        &mut self,
        background: Option<&Heap>,
        sourced: &SourcedValue,
    ) -> Result<RichValue, HeapError> {
        self.import_sourced_at(
            background,
            &sourced.value,
            &sourced.provenance,
            &mut Vec::new(),
        )
    }

    fn import_sourced_at(
        &mut self,
        background: Option<&Heap>,
        value: &Value,
        provenance: &crate::json::Provenance,
        path: &mut ValuePath,
    ) -> Result<RichValue, HeapError> {
        let loc = provenance.values.get(path).copied();
        let value = match value {
            Value::Int(value) => RuntimeValue::Int(*value),
            Value::Float(value) if value.is_finite() => RuntimeValue::Float(*value),
            Value::Float(_) => return Err(HeapError("Telora Float must be finite")),
            Value::String(value) => self.string(background, value),
            Value::Bytes(value) => {
                RuntimeValue::Bytes(self.allocate(Object::Bytes(value.as_ref().into())))
            }
            Value::NativeType(value) => {
                RuntimeValue::NativeType(self.allocate(Object::NativeType(value.clone())))
            }
            Value::Opaque(value) => {
                RuntimeValue::Opaque(self.allocate(Object::Opaque(value.clone())))
            }
            Value::Atom(Atom::Builtin(atom)) => RuntimeValue::BuiltinAtom(*atom),
            Value::Atom(Atom::Named(name)) => self.atom(background, name),
            Value::Tagged { tag, payload } => {
                let tag = match tag {
                    Atom::Builtin(atom) => RuntimeValue::BuiltinAtom(*atom),
                    Atom::Named(name) => self.atom(background, name),
                };
                path.push(ValuePathSegment::Index(0));
                let payload = self.import_sourced_at(background, payload, provenance, path)?;
                path.pop();
                RuntimeValue::Tagged(self.allocate(Object::Tagged {
                    tag: RichValue::original(tag, loc),
                    payload,
                }))
            }
            Value::Array(values) => {
                let mut imported = Vec::with_capacity(values.len());
                for (index, value) in values.iter().enumerate() {
                    path.push(ValuePathSegment::Index(index));
                    imported.push(self.import_sourced_at(background, value, provenance, path)?);
                    path.pop();
                }
                RuntimeValue::Array(self.allocate(Object::Array(imported.into())))
            }
            Value::Tuple(values) => {
                let mut imported = Vec::with_capacity(values.len());
                for (index, value) in values.iter().enumerate() {
                    path.push(ValuePathSegment::Index(index));
                    imported.push(self.import_sourced_at(background, value, provenance, path)?);
                    path.pop();
                }
                RuntimeValue::Tuple(self.allocate(Object::Tuple(imported.into())))
            }
            Value::Dict(dict) => {
                let mut fields = Vec::with_capacity(dict.values().len());
                let mut values = Vec::with_capacity(dict.values().len());
                for (field, value) in dict.shape().fields().iter().zip(dict.values()) {
                    fields.push(
                        background
                            .and_then(|heap| heap.find_text(field))
                            .unwrap_or_else(|| self.intern(field)),
                    );
                    path.push(ValuePathSegment::Key(field.clone()));
                    values.push(self.import_sourced_at(background, value, provenance, path)?);
                    path.pop();
                }
                let shape = self.intern_shape(fields);
                RuntimeValue::Dict(self.allocate(Object::Dict {
                    shape,
                    values: values.into(),
                }))
            }
            Value::Func(_) => {
                return Err(HeapError("sourced data cannot contain Func"));
            }
            Value::Dyn(_) => {
                return Err(HeapError("sourced data cannot contain Dyn"));
            }
        };
        Ok(RichValue::original(value, loc))
    }

    fn import_value_with(
        &mut self,
        background: Option<&Heap>,
        value: &Value,
        externals: &HashMap<String, PersistentValue>,
        prototypes: &mut HashMap<*const BytecodeFunction, Handle>,
        location: Option<crate::Loc>,
    ) -> Result<RichValue, HeapError> {
        Ok(RichValue::new(
            match value {
                Value::Int(value) => RuntimeValue::Int(*value),
                Value::Float(value) if value.is_finite() => RuntimeValue::Float(*value),
                Value::Float(_) => return Err(HeapError("Telora Float must be finite")),
                Value::String(value) => self.string(background, value),
                Value::Bytes(value) => {
                    RuntimeValue::Bytes(self.allocate(Object::Bytes(value.as_ref().into())))
                }
                Value::NativeType(value) => {
                    RuntimeValue::NativeType(self.allocate(Object::NativeType(value.clone())))
                }
                Value::Opaque(value) => {
                    RuntimeValue::Opaque(self.allocate(Object::Opaque(value.clone())))
                }
                Value::Atom(Atom::Builtin(atom)) => RuntimeValue::BuiltinAtom(*atom),
                Value::Atom(Atom::Named(name)) => self.atom(background, name),
                Value::Tagged { tag, payload } => {
                    let tag = match tag {
                        Atom::Builtin(atom) => RuntimeValue::BuiltinAtom(*atom),
                        Atom::Named(name) => self.atom(background, name),
                    };
                    let payload = self
                        .import_value_with(background, payload, externals, prototypes, location)?;
                    RuntimeValue::Tagged(self.allocate(Object::Tagged {
                        tag: RichValue::new(tag, location),
                        payload,
                    }))
                }
                Value::Array(values) => {
                    let values = values
                        .iter()
                        .map(|value| {
                            self.import_value_with(
                                background, value, externals, prototypes, location,
                            )
                        })
                        .collect::<Result<Box<[_]>, _>>()?;
                    RuntimeValue::Array(self.allocate(Object::Array(values)))
                }
                Value::Tuple(values) => {
                    let values = values
                        .iter()
                        .map(|value| {
                            self.import_value_with(
                                background, value, externals, prototypes, location,
                            )
                        })
                        .collect::<Result<Box<[_]>, _>>()?;
                    RuntimeValue::Tuple(self.allocate(Object::Tuple(values)))
                }
                Value::Dict(dict) => {
                    let fields = dict
                        .shape()
                        .fields()
                        .iter()
                        .map(|field| {
                            Ok(background
                                .and_then(|heap| heap.find_text(field))
                                .unwrap_or_else(|| self.intern(field)))
                        })
                        .collect::<Result<Vec<_>, HeapError>>()?;
                    let shape = self.intern_shape(fields);
                    let values = dict
                        .values()
                        .iter()
                        .map(|value| {
                            self.import_value_with(
                                background, value, externals, prototypes, location,
                            )
                        })
                        .collect::<Result<Box<[_]>, _>>()?;
                    RuntimeValue::Dict(self.allocate(Object::Dict { shape, values }))
                }
                Value::Func(closure) => {
                    let prototype = match closure.prototype() {
                        Prototype::Bytecode(function) => RuntimePrototype::Bytecode(
                            self.link_bytecode_with(background, function, externals, prototypes)?,
                        ),
                        Prototype::Native(function) => RuntimePrototype::Native(*function),
                    };
                    let upvalues = closure
                        .upvalues()
                        .iter()
                        .map(|value| {
                            self.import_value_with(
                                background, value, externals, prototypes, location,
                            )
                        })
                        .collect::<Result<Box<[_]>, _>>()?;
                    RuntimeValue::Func(self.allocate(Object::Closure {
                        identity: Arc::clone(closure.identity()),
                        prototype,
                        upvalues,
                    }))
                }
                Value::Dyn(dyn_value) => {
                    let descriptor = self.import_value_with(
                        background,
                        dyn_value.descriptor(),
                        externals,
                        prototypes,
                        location,
                    )?;
                    let value = self.import_value_with(
                        background,
                        dyn_value.value(),
                        externals,
                        prototypes,
                        location,
                    )?;
                    RuntimeValue::Dyn(self.allocate(Object::Dyn {
                        identity: Arc::clone(dyn_value.identity()),
                        descriptor,
                        value,
                        scheme: dyn_value.scheme().cloned(),
                        origin: dyn_value.origin().map(Arc::from),
                    }))
                }
            },
            location,
        ))
    }

    pub(crate) fn link_bytecode_resolved(
        &mut self,
        background: Option<&Heap>,
        function: &BytecodeFunction,
        externals: &HashMap<String, PersistentValue>,
    ) -> Result<Handle, HeapError> {
        self.link_bytecode_with(background, function, externals, &mut HashMap::new())
    }

    fn link_bytecode_with(
        &mut self,
        background: Option<&Heap>,
        function: &BytecodeFunction,
        externals: &HashMap<String, PersistentValue>,
        forwarded: &mut HashMap<*const BytecodeFunction, Handle>,
    ) -> Result<Handle, HeapError> {
        let identity = std::ptr::from_ref(function);
        if let Some(handle) = forwarded.get(&identity) {
            return Ok(*handle);
        }
        let handle = self.reserve();
        forwarded.insert(identity, handle);
        let values = function
            .links()
            .values()
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if let Some(key) = function.links().external_value(index) {
                    return externals
                        .get(key)
                        .copied()
                        .map(PersistentValue::runtime)
                        .ok_or(HeapError("external value link is unresolved"));
                }
                self.import_value_with(background, value, externals, forwarded, None)
            })
            .collect::<Result<Box<[_]>, _>>()?;
        let text = function
            .links()
            .text()
            .iter()
            .map(|text| {
                background
                    .and_then(|heap| heap.find_text(text))
                    .unwrap_or_else(|| self.intern(text))
            })
            .collect::<Box<[_]>>();
        let prototypes = function
            .links()
            .prototypes()
            .iter()
            .map(|prototype| {
                self.link_bytecode_with(background, prototype, externals, forwarded)
                    .map(RuntimePrototype::Bytecode)
            })
            .collect::<Result<Box<[_]>, _>>()?;
        self.initialize(
            handle,
            Object::ByteCodeProto {
                code: Arc::clone(function.code()),
                values,
                text,
                prototypes,
            },
        )?;
        Ok(handle)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct HeapView<'a> {
    pub(crate) current: &'a Heap,
    pub(crate) background: Option<&'a Heap>,
}

type BytecodeLinks<'a> = (
    &'a Arc<FuncByteCode>,
    &'a [RichValue],
    &'a [InternId],
    &'a [RuntimePrototype],
);

impl<'a> HeapView<'a> {
    fn heap(&self, storage: Storage) -> Result<&'a Heap, HeapError> {
        match storage {
            Storage::Work if self.current.storage == Storage::Work => Ok(self.current),
            Storage::Main if self.current.storage == Storage::Main => Ok(self.current),
            Storage::Main => self
                .background
                .filter(|heap| heap.storage == Storage::Main)
                .ok_or(HeapError("Main value has no Main world")),
            _ => Err(HeapError("value refers to a heap outside its view")),
        }
    }

    pub(crate) fn object(&self, handle: Handle) -> Result<&'a Object, HeapError> {
        self.heap(handle.storage)?.object(handle)
    }

    pub(crate) fn text(&self, id: InternId) -> Result<&'a str, HeapError> {
        self.heap(id.storage)?.resolve_text(id)
    }

    fn shape(&self, id: ShapeId) -> Result<&'a [InternId], HeapError> {
        self.heap(id.storage)?.shape(id)
    }

    pub(crate) fn bytecode(&self, handle: Handle) -> Result<BytecodeLinks<'a>, HeapError> {
        let Object::ByteCodeProto {
            code,
            values,
            text,
            prototypes,
        } = self.object(handle)?
        else {
            return Err(HeapError("handle is not a bytecode prototype"));
        };
        Ok((code, values, text, prototypes))
    }

    pub(crate) fn closure(
        &self,
        handle: Handle,
    ) -> Result<(RuntimePrototype, &'a [RichValue]), HeapError> {
        let Object::Closure {
            prototype,
            upvalues,
            ..
        } = self.object(handle)?
        else {
            return Err(HeapError("handle is not a closure"));
        };
        Ok((*prototype, upvalues))
    }

    pub(crate) fn function_arity(&self, handle: Handle) -> Result<usize, HeapError> {
        let (prototype, _) = self.closure(handle)?;
        match prototype {
            RuntimePrototype::Native(function) => Ok(function.arity()),
            RuntimePrototype::Bytecode(handle) => {
                let Object::ByteCodeProto { code, .. } = self.object(handle)? else {
                    return Err(HeapError("prototype handle refers to another object kind"));
                };
                Ok(code.parameter_count())
            }
        }
    }

    pub(crate) fn up_link(&self, handle: Handle) -> Result<Option<RichValue>, HeapError> {
        let Object::UpLink { value } = self.object(handle)? else {
            return Err(HeapError("handle is not an up-link"));
        };
        Ok(*value)
    }

    pub(crate) fn dyn_parts(
        &self,
        handle: Handle,
    ) -> Result<(&'a Arc<()>, RichValue, RichValue), HeapError> {
        let Object::Dyn {
            identity,
            descriptor,
            value,
            ..
        } = self.object(handle)?
        else {
            return Err(HeapError("handle is not a Dyn"));
        };
        Ok((identity, *descriptor, *value))
    }

    pub(crate) fn sequence(
        &self,
        handle: Handle,
        tuple: bool,
    ) -> Result<&'a [RichValue], HeapError> {
        match self.object(handle)? {
            Object::Array(values) if !tuple => Ok(values),
            Object::Tuple(values) if tuple => Ok(values),
            _ => Err(HeapError("handle is not the requested sequence kind")),
        }
    }

    pub(crate) fn tagged(&self, handle: Handle) -> Result<(RichValue, RichValue), HeapError> {
        let Object::Tagged { tag, payload } = self.object(handle)? else {
            return Err(HeapError("handle is not a Tagged value"));
        };
        Ok((*tag, *payload))
    }

    pub(crate) fn dict_get(
        &self,
        handle: Handle,
        field: InternId,
    ) -> Result<Option<RichValue>, HeapError> {
        let Object::Dict { shape, values } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        let wanted = self.text(field)?;
        let fields = self.shape(*shape)?;
        let index = fields
            .binary_search_by(|candidate| {
                if *candidate == field {
                    Ordering::Equal
                } else {
                    self.text(*candidate).unwrap_or("").cmp(wanted)
                }
            })
            .ok();
        Ok(index.and_then(|index| values.get(index).copied()))
    }

    pub(crate) fn dict_fields(&self, handle: Handle) -> Result<Vec<&'a str>, HeapError> {
        let Object::Dict { shape, .. } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        self.shape(*shape)?
            .iter()
            .map(|field| self.text(*field))
            .collect()
    }

    pub(crate) fn dict_parts(
        &self,
        handle: Handle,
    ) -> Result<(&'a [InternId], &'a [RichValue]), HeapError> {
        let Object::Dict { shape, values } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        Ok((self.shape(*shape)?, values))
    }

    pub(crate) fn dict_get_text(
        &self,
        handle: Handle,
        field: &str,
    ) -> Result<Option<RichValue>, HeapError> {
        let Object::Dict { shape, values } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        let fields = self.shape(*shape)?;
        let index = fields
            .binary_search_by(|candidate| self.text(*candidate).unwrap_or("").cmp(field))
            .ok();
        Ok(index.and_then(|index| values.get(index).copied()))
    }

    pub(crate) fn string_text(&self, value: RichValue) -> Result<Option<&'a str>, HeapError> {
        match value.value {
            RuntimeValue::ShortString(id) => Ok(Some(self.text(id)?)),
            RuntimeValue::String(handle) => match self.object(handle)? {
                Object::String(value) => Ok(Some(value)),
                _ => Err(HeapError("String handle refers to another object kind")),
            },
            _ => Ok(None),
        }
    }

    pub(crate) fn atom_text(&self, value: RichValue) -> Result<Option<&'a str>, HeapError> {
        match value.value {
            RuntimeValue::BuiltinAtom(atom) => Ok(Some(atom.name())),
            RuntimeValue::Atom(id) => Ok(Some(self.text(id)?)),
            _ => Ok(None),
        }
    }

    pub(crate) fn values_equal(
        &self,
        left: RichValue,
        right: RichValue,
    ) -> Result<bool, HeapError> {
        self.values_equal_with(left.value, right.value, &mut HashSet::new())
    }

    /// Returns the first failed node reachable through data containers.
    ///
    /// Closures and opaque/native values are intentionally atomic here: an
    /// operation only depends on their identity, not on captured internals.
    pub(crate) fn first_data_failure(&self, root: RichValue) -> Result<Option<u32>, HeapError> {
        let mut pending = vec![root.value];
        let mut visited = HashSet::new();
        while let Some(value) = pending.pop() {
            let handle = match value {
                RuntimeValue::Failed(failure) => return Ok(Some(failure)),
                RuntimeValue::Array(handle)
                | RuntimeValue::Tuple(handle)
                | RuntimeValue::Tagged(handle)
                | RuntimeValue::Dict(handle)
                | RuntimeValue::Dyn(handle) => handle,
                RuntimeValue::Int(_)
                | RuntimeValue::Float(_)
                | RuntimeValue::BuiltinAtom(_)
                | RuntimeValue::Atom(_)
                | RuntimeValue::ShortString(_)
                | RuntimeValue::String(_)
                | RuntimeValue::Bytes(_)
                | RuntimeValue::Opaque(_)
                | RuntimeValue::NativeType(_)
                | RuntimeValue::Func(_)
                | RuntimeValue::UpLink(_) => continue,
            };
            if !visited.insert(handle) {
                continue;
            }
            match self.object(handle)? {
                Object::Array(values) | Object::Tuple(values) => {
                    pending.extend(values.iter().rev().map(|value| value.value));
                }
                Object::Tagged { tag, payload } => {
                    pending.push(payload.value);
                    pending.push(tag.value);
                }
                Object::Dict { values, .. } => {
                    pending.extend(values.iter().rev().map(|value| value.value));
                }
                Object::Dyn {
                    descriptor, value, ..
                } => {
                    pending.push(value.value);
                    pending.push(descriptor.value);
                }
                Object::String(_)
                | Object::Bytes(_)
                | Object::Opaque(_)
                | Object::NativeType(_)
                | Object::Closure { .. }
                | Object::UpLink { .. }
                | Object::ByteCodeProto { .. }
                | Object::Reserved => {}
            }
        }
        Ok(None)
    }

    fn values_equal_with(
        &self,
        left: RuntimeValue,
        right: RuntimeValue,
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        match (left, right) {
            (RuntimeValue::Func(left), RuntimeValue::Func(right)) => {
                let Object::Closure { identity: left, .. } = self.object(left)? else {
                    return Err(HeapError("Func handle refers to another object kind"));
                };
                let Object::Closure {
                    identity: right, ..
                } = self.object(right)?
                else {
                    return Err(HeapError("Func handle refers to another object kind"));
                };
                Ok(Arc::ptr_eq(left, right))
            }
            (RuntimeValue::Dyn(left), RuntimeValue::Dyn(right)) => {
                let (left, _, _) = self.dyn_parts(left)?;
                let (right, _, _) = self.dyn_parts(right)?;
                Ok(Arc::ptr_eq(left, right))
            }
            (RuntimeValue::UpLink(_), _) | (_, RuntimeValue::UpLink(_)) => {
                Err(HeapError("up-link escaped into equality"))
            }
            (RuntimeValue::Int(left), RuntimeValue::Int(right)) => Ok(left == right),
            (RuntimeValue::Float(left), RuntimeValue::Float(right)) => Ok(left == right),
            (RuntimeValue::BuiltinAtom(left), RuntimeValue::BuiltinAtom(right)) => {
                Ok(left == right)
            }
            (RuntimeValue::Atom(left), RuntimeValue::Atom(right))
            | (RuntimeValue::ShortString(left), RuntimeValue::ShortString(right)) => {
                if left.storage == right.storage {
                    Ok(left == right)
                } else {
                    Ok(self.text(left)? == self.text(right)?)
                }
            }
            (left @ RuntimeValue::BuiltinAtom(_), right @ RuntimeValue::Atom(_))
            | (left @ RuntimeValue::Atom(_), right @ RuntimeValue::BuiltinAtom(_)) => {
                Ok(self.atom_text(left.into())? == self.atom_text(right.into())?)
            }
            (
                left @ (RuntimeValue::ShortString(_) | RuntimeValue::String(_)),
                right @ (RuntimeValue::ShortString(_) | RuntimeValue::String(_)),
            ) => {
                if let (RuntimeValue::String(left), RuntimeValue::String(right)) = (left, right)
                    && left == right
                {
                    return Ok(true);
                }
                Ok(self.string_text(left.into())? == self.string_text(right.into())?)
            }
            (RuntimeValue::Bytes(left), RuntimeValue::Bytes(right)) => {
                if left == right {
                    return Ok(true);
                }
                let Object::Bytes(left) = self.object(left)? else {
                    return Err(HeapError("Bytes handle refers to another object kind"));
                };
                let Object::Bytes(right) = self.object(right)? else {
                    return Err(HeapError("Bytes handle refers to another object kind"));
                };
                Ok(left == right)
            }
            (RuntimeValue::Opaque(left), RuntimeValue::Opaque(right)) => {
                if left == right {
                    return Ok(true);
                }
                let Object::Opaque(left) = self.object(left)? else {
                    return Err(HeapError("Opaque handle refers to another object kind"));
                };
                let Object::Opaque(right) = self.object(right)? else {
                    return Err(HeapError("Opaque handle refers to another object kind"));
                };
                Ok(left.logical_eq(right))
            }
            (RuntimeValue::NativeType(left), RuntimeValue::NativeType(right)) => {
                if left == right {
                    return Ok(true);
                }
                let Object::NativeType(left) = self.object(left)? else {
                    return Err(HeapError("NativeType handle refers to another object kind"));
                };
                let Object::NativeType(right) = self.object(right)? else {
                    return Err(HeapError("NativeType handle refers to another object kind"));
                };
                Ok(left == right)
            }
            (RuntimeValue::Array(left), RuntimeValue::Array(right))
            | (RuntimeValue::Tuple(left), RuntimeValue::Tuple(right)) => {
                self.sequence_handles_equal(left, right, visited)
            }
            (RuntimeValue::Tagged(left), RuntimeValue::Tagged(right)) => {
                if left == right || !visited.insert((left, right)) {
                    return Ok(true);
                }
                let (left_tag, left_payload) = self.tagged(left)?;
                let (right_tag, right_payload) = self.tagged(right)?;
                Ok(
                    self.values_equal_with(left_tag.value, right_tag.value, visited)?
                        && self.values_equal_with(
                            left_payload.value,
                            right_payload.value,
                            visited,
                        )?,
                )
            }
            (RuntimeValue::Dict(left), RuntimeValue::Dict(right)) => {
                self.dict_handles_equal(left, right, visited)
            }
            _ => Ok(false),
        }
    }

    fn sequence_handles_equal(
        &self,
        left: Handle,
        right: Handle,
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        if left == right {
            return Ok(true);
        }
        if !visited.insert((left, right)) {
            return Ok(true);
        }
        let left_values = match self.object(left)? {
            Object::Array(values) | Object::Tuple(values) => values,
            _ => return Err(HeapError("sequence handle refers to another object kind")),
        };
        let right_values = match self.object(right)? {
            Object::Array(values) | Object::Tuple(values) => values,
            _ => return Err(HeapError("sequence handle refers to another object kind")),
        };
        self.value_slices_equal(left_values, right_values, visited)
    }

    fn dict_handles_equal(
        &self,
        left: Handle,
        right: Handle,
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        if left == right {
            return Ok(true);
        }
        if !visited.insert((left, right)) {
            return Ok(true);
        }
        let Object::Dict {
            shape: left_shape,
            values: left_values,
        } = self.object(left)?
        else {
            return Err(HeapError("Dict handle refers to another object kind"));
        };
        let Object::Dict {
            shape: right_shape,
            values: right_values,
        } = self.object(right)?
        else {
            return Err(HeapError("Dict handle refers to another object kind"));
        };
        let left_fields = self.shape(*left_shape)?;
        let right_fields = self.shape(*right_shape)?;
        if left_fields.len() != right_fields.len() {
            return Ok(false);
        }
        for (left, right) in left_fields.iter().zip(right_fields) {
            if self.text(*left)? != self.text(*right)? {
                return Ok(false);
            }
        }
        self.value_slices_equal(left_values, right_values, visited)
    }

    fn value_slices_equal(
        &self,
        left: &[RichValue],
        right: &[RichValue],
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(right) {
            if !self.values_equal_with(left.value, right.value, visited)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn export_value(&self, value: RichValue) -> Result<Value, HeapError> {
        self.export_value_with(value.value, &mut HashSet::new(), None)
    }

    fn export_value_projecting_up_links(
        &self,
        value: RichValue,
        projection: &Value,
    ) -> Result<Value, HeapError> {
        self.export_value_with(value.value, &mut HashSet::new(), Some(projection))
    }

    fn export_value_with(
        &self,
        value: RuntimeValue,
        visiting: &mut HashSet<Handle>,
        up_link_projection: Option<&Value>,
    ) -> Result<Value, HeapError> {
        Ok(match value {
            RuntimeValue::Failed(_) => {
                return Err(HeapError(
                    "failed evaluation node cannot cross a value boundary",
                ));
            }
            RuntimeValue::Int(value) => Value::Int(value),
            RuntimeValue::Float(value) if value.is_finite() => Value::Float(value),
            RuntimeValue::Float(_) => return Err(HeapError("Telora Float must be finite")),
            RuntimeValue::BuiltinAtom(atom) => Value::Atom(Atom::builtin(atom)),
            RuntimeValue::Atom(id) => Value::atom(self.text(id)?),
            RuntimeValue::ShortString(id) => Value::string(self.text(id)?),
            RuntimeValue::String(handle) => {
                let Object::String(value) = self.enter_object(handle, visiting)? else {
                    return Err(HeapError("String handle refers to another object kind"));
                };
                let value = Value::string(value.as_ref());
                visiting.remove(&handle);
                value
            }
            RuntimeValue::Bytes(handle) => {
                let Object::Bytes(value) = self.enter_object(handle, visiting)? else {
                    return Err(HeapError("Bytes handle refers to another object kind"));
                };
                let value = Value::Bytes(value.clone().into());
                visiting.remove(&handle);
                value
            }
            RuntimeValue::Opaque(handle) => {
                let Object::Opaque(value) = self.enter_object(handle, visiting)? else {
                    return Err(HeapError("Opaque handle refers to another object kind"));
                };
                let value = Value::Opaque(value.clone());
                visiting.remove(&handle);
                value
            }
            RuntimeValue::NativeType(handle) => {
                let Object::NativeType(value) = self.enter_object(handle, visiting)? else {
                    return Err(HeapError("NativeType handle refers to another object kind"));
                };
                let value = Value::NativeType(value.clone());
                visiting.remove(&handle);
                value
            }
            RuntimeValue::Array(handle) | RuntimeValue::Tuple(handle) => {
                let tuple = matches!(value, RuntimeValue::Tuple(_));
                let object = self.enter_object(handle, visiting)?;
                let values = match object {
                    Object::Array(values) if !tuple => values,
                    Object::Tuple(values) if tuple => values,
                    _ => return Err(HeapError("sequence handle refers to another object kind")),
                };
                let values = values
                    .iter()
                    .map(|value| self.export_value_with(value.value, visiting, up_link_projection))
                    .collect::<Result<Vec<_>, _>>()?;
                visiting.remove(&handle);
                if tuple {
                    Value::Tuple(values.into())
                } else {
                    Value::Array(values.into())
                }
            }
            RuntimeValue::Tagged(handle) => {
                let Object::Tagged { tag, payload } = self.enter_object(handle, visiting)? else {
                    return Err(HeapError("Tagged handle refers to another object kind"));
                };
                let tag = match self.export_value_with(tag.value, visiting, up_link_projection)? {
                    Value::Atom(tag) => tag,
                    _ => return Err(HeapError("Tagged tag is not an Atom")),
                };
                let payload =
                    self.export_value_with(payload.value, visiting, up_link_projection)?;
                visiting.remove(&handle);
                Value::tagged(tag, payload)
            }
            RuntimeValue::Dict(handle) => {
                let Object::Dict { shape, values } = self.enter_object(handle, visiting)? else {
                    return Err(HeapError("Dict handle refers to another object kind"));
                };
                let fields = self
                    .shape(*shape)?
                    .iter()
                    .map(|field| self.text(*field).map(str::to_owned))
                    .collect::<Result<Vec<_>, _>>()?;
                let values = values
                    .iter()
                    .map(|value| self.export_value_with(value.value, visiting, up_link_projection))
                    .collect::<Result<Vec<_>, _>>()?;
                visiting.remove(&handle);
                let owner = self.heap(shape.storage)?;
                let mut exported_shapes = owner
                    .exported_shapes
                    .lock()
                    .map_err(|_| HeapError("exported shape cache is poisoned"))?;
                let shape = if let Some(shape) = exported_shapes.get(&shape.slot) {
                    Arc::clone(shape)
                } else {
                    let shape_value = Arc::new(Shape::from_sorted_fields(fields));
                    exported_shapes.insert(shape.slot, Arc::clone(&shape_value));
                    shape_value
                };
                Value::Dict(Dict::new(shape, values))
            }
            RuntimeValue::Func(handle) => {
                let Object::Closure {
                    identity,
                    prototype,
                    upvalues,
                } = self.enter_object(handle, visiting)?
                else {
                    return Err(HeapError("Func handle refers to another object kind"));
                };
                let prototype = self.export_prototype(prototype, visiting, up_link_projection)?;
                let upvalues = upvalues
                    .iter()
                    .map(|value| self.export_value_with(value.value, visiting, up_link_projection))
                    .collect::<Result<Vec<_>, _>>()?;
                visiting.remove(&handle);
                Value::Func(Arc::new(Closure::from_parts_with_identity(
                    Arc::clone(identity),
                    prototype,
                    upvalues,
                )))
            }
            RuntimeValue::Dyn(handle) => {
                let Object::Dyn {
                    identity,
                    descriptor,
                    value,
                    scheme,
                    origin,
                } = self.enter_object(handle, visiting)?
                else {
                    return Err(HeapError("Dyn handle refers to another object kind"));
                };
                let descriptor =
                    self.export_value_with(descriptor.value, visiting, up_link_projection)?;
                let value = self.export_value_with(value.value, visiting, up_link_projection)?;
                visiting.remove(&handle);
                Value::Dyn(Arc::new(DynValue::from_parts_with_metadata(
                    Arc::clone(identity),
                    descriptor,
                    value,
                    scheme.clone(),
                    origin.clone(),
                )))
            }
            RuntimeValue::UpLink(handle) => {
                let linked = self
                    .up_link(handle)?
                    .ok_or(HeapError("up-link is uninitialized"))?;
                if let Some(projection) = up_link_projection
                    && self.is_type_metadata_root(linked.value)?
                {
                    return Ok(projection.clone());
                }
                if !visiting.insert(handle) {
                    return Err(HeapError(
                        "cyclic heap values cannot cross the legacy Value boundary",
                    ));
                }
                let value = self.export_value_with(linked.value, visiting, up_link_projection)?;
                visiting.remove(&handle);
                value
            }
        })
    }

    fn is_type_metadata_root(&self, value: RuntimeValue) -> Result<bool, HeapError> {
        let RuntimeValue::Dict(handle) = value else {
            return Ok(false);
        };
        let Some(kind) = self.dict_get_text(handle, "kind")? else {
            return Ok(false);
        };
        let kind = match kind.value {
            RuntimeValue::BuiltinAtom(atom) => atom.name(),
            RuntimeValue::Atom(id) => self.text(id)?,
            _ => return Ok(false),
        };
        Ok(matches!(
            kind,
            "Any"
                | "Never"
                | "Type"
                | "Dyn"
                | "TypeOf"
                | "Int"
                | "Float"
                | "String"
                | "Bytes"
                | "Atom"
                | "Array"
                | "Dict"
                | "Tagged"
                | "Tuple"
                | "Struct"
                | "Enum"
                | "Union"
                | "Func"
                | "WithAttributes"
                | "Bound"
        ))
    }

    fn enter_object<'view>(
        &'view self,
        handle: Handle,
        visiting: &mut HashSet<Handle>,
    ) -> Result<&'view Object, HeapError> {
        if !visiting.insert(handle) {
            return Err(HeapError(
                "cyclic heap values cannot cross the legacy Value boundary",
            ));
        }
        self.object(handle)
    }

    fn export_prototype(
        &self,
        prototype: &RuntimePrototype,
        visiting: &mut HashSet<Handle>,
        up_link_projection: Option<&Value>,
    ) -> Result<Prototype, HeapError> {
        Ok(match prototype {
            RuntimePrototype::Native(function) => Prototype::Native(*function),
            RuntimePrototype::Bytecode(handle) => {
                let Object::ByteCodeProto {
                    code,
                    values,
                    text,
                    prototypes,
                } = self.enter_object(*handle, visiting)?
                else {
                    return Err(HeapError("prototype handle refers to another object kind"));
                };
                let values = values
                    .iter()
                    .map(|value| self.export_value_with(value.value, visiting, up_link_projection))
                    .collect::<Result<Vec<_>, _>>()?;
                let text = text
                    .iter()
                    .map(|id| self.text(*id).map(Arc::<str>::from))
                    .collect::<Result<Vec<_>, _>>()?;
                let prototypes = prototypes
                    .iter()
                    .map(|prototype| {
                        match self.export_prototype(prototype, visiting, up_link_projection)? {
                            Prototype::Bytecode(function) => Ok(function),
                            Prototype::Native(_) => Err(HeapError(
                                "native prototype cannot occupy a bytecode link slot",
                            )),
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                visiting.remove(handle);
                Prototype::Bytecode(Arc::new(BytecodeFunction::from_linked_parts(
                    Arc::clone(code),
                    values,
                    text,
                    prototypes,
                )))
            }
        })
    }
}

fn copy_roots(
    target: &mut Heap,
    source: HeapView<'_>,
    roots: &[RichValue],
) -> Result<Vec<RichValue>, HeapError> {
    let mut pending = PendingCopy::new(target, &source);
    let roots = roots
        .iter()
        .map(|root| pending.copy_value(target, &source, *root))
        .collect::<Result<Vec<_>, _>>()?;
    pending.validate()?;
    pending.commit(target);
    Ok(roots)
}

pub(crate) fn relocate_work_roots(
    target: &mut Heap,
    main: &Heap,
    source: &Heap,
    roots: &[RichValue],
) -> Result<Vec<RichValue>, HeapError> {
    if target.storage != Storage::Work
        || source.storage != Storage::Work
        || main.storage != Storage::Main
    {
        return Err(HeapError(
            "work relocation requires two Work worlds and one Main world",
        ));
    }
    copy_roots(
        target,
        HeapView {
            current: source,
            background: Some(main),
        },
        roots,
    )
}

pub(crate) fn publish_root(
    target: &mut Heap,
    current: &Heap,
    root: RichValue,
) -> Result<PersistentValue, HeapError> {
    if target.storage != Storage::Main || current.storage != Storage::Work {
        return Err(HeapError(
            "publication requires a Work world and Main world",
        ));
    }
    let roots = copy_roots(
        target,
        HeapView {
            current,
            background: None,
        },
        &[root],
    )?;
    Ok(PersistentValue(roots[0]))
}

pub(crate) fn publish_value(
    target: &mut Heap,
    value: &Value,
) -> Result<PersistentValue, HeapError> {
    let mut local = Heap::work();
    let root = local.import_value(Some(target), value)?;
    publish_root(target, &local, root)
}

struct PendingCopy {
    target_storage: Storage,
    source_storage: Storage,
    background_storage: Option<Storage>,
    object_base: u32,
    objects: Vec<Object>,
    text_base: u32,
    text: TextTable,
    shape_base: u32,
    shapes: Vec<Box<[InternId]>>,
    objects_forwarded: HashMap<Handle, Handle>,
    text_forwarded: HashMap<InternId, InternId>,
    shapes_forwarded: HashMap<ShapeId, ShapeId>,
}

impl PendingCopy {
    fn new(target: &Heap, source: &HeapView<'_>) -> Self {
        Self {
            target_storage: target.storage,
            source_storage: source.current.storage,
            background_storage: source.background.map(|heap| heap.storage),
            object_base: target.objects.len() as u32,
            objects: Vec::new(),
            text_base: target.text.values.len() as u32,
            text: TextTable::default(),
            shape_base: target.shapes.len() as u32,
            shapes: Vec::new(),
            objects_forwarded: HashMap::new(),
            text_forwarded: HashMap::new(),
            shapes_forwarded: HashMap::new(),
        }
    }

    fn copy_value(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        value: RichValue,
    ) -> Result<RichValue, HeapError> {
        let copied = match value.value {
            RuntimeValue::Failed(id) if self.target_storage == Storage::Work => {
                RuntimeValue::Failed(id)
            }
            RuntimeValue::Failed(_) => {
                return Err(HeapError("failed evaluation node cannot enter Main world"));
            }
            RuntimeValue::Int(_) | RuntimeValue::BuiltinAtom(_) => value.value,
            RuntimeValue::Float(float) if float.is_finite() => value.value,
            RuntimeValue::Float(_) => return Err(HeapError("Telora Float must be finite")),
            RuntimeValue::Atom(id) => RuntimeValue::Atom(self.copy_text(target, source, id)?),
            RuntimeValue::ShortString(id) => {
                RuntimeValue::ShortString(self.copy_text(target, source, id)?)
            }
            RuntimeValue::String(handle) => {
                RuntimeValue::String(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::Bytes(handle) => {
                RuntimeValue::Bytes(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::Opaque(handle) => {
                RuntimeValue::Opaque(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::NativeType(handle) => {
                RuntimeValue::NativeType(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::Array(handle) => {
                RuntimeValue::Array(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::Tuple(handle) => {
                RuntimeValue::Tuple(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::Tagged(handle) => {
                RuntimeValue::Tagged(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::Dict(handle) => {
                RuntimeValue::Dict(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::Func(handle) => {
                RuntimeValue::Func(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::Dyn(handle) => {
                RuntimeValue::Dyn(self.copy_object(target, source, handle)?)
            }
            RuntimeValue::UpLink(handle) => {
                RuntimeValue::UpLink(self.copy_object(target, source, handle)?)
            }
        };
        Ok(value.with_value(copied))
    }

    fn copy_object(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        handle: Handle,
    ) -> Result<Handle, HeapError> {
        if handle.storage != self.source_storage {
            if handle.storage == self.target_storage {
                target.object(handle)?;
            } else {
                source.object(handle)?;
            }
            return Ok(handle);
        }
        if let Some(forwarded) = self.objects_forwarded.get(&handle) {
            return Ok(*forwarded);
        }
        let object = source.object(handle)?;
        if matches!(object, Object::Reserved) {
            return Err(HeapError("cannot copy an uninitialized object"));
        }
        let copied = Handle {
            storage: self.target_storage,
            slot: self.object_base + self.objects.len() as u32,
        };
        self.objects_forwarded.insert(handle, copied);
        self.objects.push(Object::Reserved);
        let object = self.copy_object_data(target, source, object)?;
        self.objects[(copied.slot - self.object_base) as usize] = object;
        Ok(copied)
    }

    fn copy_object_data(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        object: &Object,
    ) -> Result<Object, HeapError> {
        let copy_values = |this: &mut Self, values: &[RichValue]| {
            values
                .iter()
                .map(|value| this.copy_value(target, source, *value))
                .collect::<Result<Box<[_]>, _>>()
        };
        Ok(match object {
            Object::Reserved => return Err(HeapError("cannot copy an uninitialized object")),
            Object::String(value) => Object::String(value.clone()),
            Object::Bytes(value) => Object::Bytes(value.clone()),
            Object::Opaque(value) => Object::Opaque(value.clone()),
            Object::NativeType(value) => Object::NativeType(value.clone()),
            Object::Array(values) => Object::Array(copy_values(self, values)?),
            Object::Tuple(values) => Object::Tuple(copy_values(self, values)?),
            Object::Tagged { tag, payload } => Object::Tagged {
                tag: self.copy_value(target, source, *tag)?,
                payload: self.copy_value(target, source, *payload)?,
            },
            Object::Dict { shape, values } => Object::Dict {
                shape: self.copy_shape(target, source, *shape)?,
                values: copy_values(self, values)?,
            },
            Object::Closure {
                identity,
                prototype,
                upvalues,
            } => Object::Closure {
                identity: Arc::clone(identity),
                prototype: self.copy_prototype(target, source, prototype)?,
                upvalues: copy_values(self, upvalues)?,
            },
            Object::Dyn {
                identity,
                descriptor,
                value,
                scheme,
                origin,
            } => Object::Dyn {
                identity: Arc::clone(identity),
                descriptor: self.copy_value(target, source, *descriptor)?,
                value: self.copy_value(target, source, *value)?,
                scheme: scheme.clone(),
                origin: origin.clone(),
            },
            Object::UpLink { value } => Object::UpLink {
                value: Some(self.copy_value(
                    target,
                    source,
                    value.ok_or(HeapError("cannot publish an uninitialized up-link"))?,
                )?),
            },
            Object::ByteCodeProto {
                code,
                values,
                text,
                prototypes,
            } => Object::ByteCodeProto {
                code: Arc::clone(code),
                values: copy_values(self, values)?,
                text: text
                    .iter()
                    .map(|id| self.copy_text(target, source, *id))
                    .collect::<Result<Box<[_]>, _>>()?,
                prototypes: prototypes
                    .iter()
                    .map(|prototype| self.copy_prototype(target, source, prototype))
                    .collect::<Result<Box<[_]>, _>>()?,
            },
        })
    }

    fn copy_prototype(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        prototype: &RuntimePrototype,
    ) -> Result<RuntimePrototype, HeapError> {
        Ok(match prototype {
            RuntimePrototype::Bytecode(handle) => {
                RuntimePrototype::Bytecode(self.copy_object(target, source, *handle)?)
            }
            RuntimePrototype::Native(function) => RuntimePrototype::Native(*function),
        })
    }

    fn copy_text(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        id: InternId,
    ) -> Result<InternId, HeapError> {
        if id.storage != self.source_storage {
            if id.storage == self.target_storage {
                target.resolve_text(id)?;
            } else {
                source.text(id)?;
            }
            return Ok(id);
        }
        if let Some(forwarded) = self.text_forwarded.get(&id) {
            return Ok(*forwarded);
        }
        let text = source.text(id)?;
        let copied = if let Some(id) = target.find_text(text) {
            id
        } else if let Some(slot) = self.text.find(text) {
            InternId {
                storage: self.target_storage,
                slot: self.text_base + slot,
            }
        } else {
            let local_slot = self.text.insert(text);
            InternId {
                storage: self.target_storage,
                slot: self.text_base + local_slot,
            }
        };
        self.text_forwarded.insert(id, copied);
        Ok(copied)
    }

    fn copy_shape(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        id: ShapeId,
    ) -> Result<ShapeId, HeapError> {
        if id.storage != self.source_storage {
            if id.storage == self.target_storage {
                target.shape(id)?;
            } else {
                source.shape(id)?;
            }
            return Ok(id);
        }
        if let Some(forwarded) = self.shapes_forwarded.get(&id) {
            return Ok(*forwarded);
        }
        let fields = source
            .shape(id)?
            .iter()
            .map(|field| self.copy_text(target, source, *field))
            .collect::<Result<Vec<_>, _>>()?;
        let copied = if let Some(slot) = target.shape_slots.get(&fields) {
            ShapeId {
                storage: self.target_storage,
                slot: *slot,
            }
        } else if let Some(index) = self.shapes.iter().position(|shape| **shape == fields) {
            ShapeId {
                storage: self.target_storage,
                slot: self.shape_base + index as u32,
            }
        } else {
            let copied = ShapeId {
                storage: self.target_storage,
                slot: self.shape_base + self.shapes.len() as u32,
            };
            self.shapes.push(fields.into());
            copied
        };
        self.shapes_forwarded.insert(id, copied);
        Ok(copied)
    }

    fn validate(&self) -> Result<(), HeapError> {
        if self.objects.iter().any(|object| {
            object_contains_disallowed(object, self.target_storage, self.background_storage)
        }) {
            return Err(HeapError(
                "copied object graph is not target-self-contained",
            ));
        }
        Ok(())
    }

    fn commit(self, target: &mut Heap) {
        target.objects.extend(self.objects);
        for value in self.text.values {
            target.text.insert(&value);
        }
        for shape in self.shapes {
            target.intern_shape(shape.into_vec());
        }
    }
}

#[cfg(test)]
fn value_contains_foreign(value: RuntimeValue, target: Storage) -> bool {
    match value {
        RuntimeValue::Atom(id) | RuntimeValue::ShortString(id) => id.storage != target,
        RuntimeValue::String(handle)
        | RuntimeValue::Bytes(handle)
        | RuntimeValue::Opaque(handle)
        | RuntimeValue::NativeType(handle)
        | RuntimeValue::Array(handle)
        | RuntimeValue::Tuple(handle)
        | RuntimeValue::Tagged(handle)
        | RuntimeValue::Dict(handle)
        | RuntimeValue::Func(handle)
        | RuntimeValue::Dyn(handle)
        | RuntimeValue::UpLink(handle) => handle.storage != target,
        RuntimeValue::Failed(_)
        | RuntimeValue::Int(_)
        | RuntimeValue::Float(_)
        | RuntimeValue::BuiltinAtom(_) => false,
    }
}

#[cfg(test)]
fn rich_value_contains_foreign(value: RichValue, target: Storage) -> bool {
    value_contains_foreign(value.value, target)
}

fn object_contains_disallowed(
    object: &Object,
    target: Storage,
    background: Option<Storage>,
) -> bool {
    let foreign = |storage| storage != target && Some(storage) != background;
    let value_foreign = |value: RichValue| match value.value {
        RuntimeValue::Atom(id) | RuntimeValue::ShortString(id) => foreign(id.storage),
        RuntimeValue::String(handle)
        | RuntimeValue::Bytes(handle)
        | RuntimeValue::Opaque(handle)
        | RuntimeValue::NativeType(handle)
        | RuntimeValue::Array(handle)
        | RuntimeValue::Tuple(handle)
        | RuntimeValue::Tagged(handle)
        | RuntimeValue::Dict(handle)
        | RuntimeValue::Func(handle)
        | RuntimeValue::Dyn(handle)
        | RuntimeValue::UpLink(handle) => foreign(handle.storage),
        RuntimeValue::Failed(_)
        | RuntimeValue::Int(_)
        | RuntimeValue::Float(_)
        | RuntimeValue::BuiltinAtom(_) => false,
    };
    match object {
        Object::Reserved => true,
        Object::Array(values) | Object::Tuple(values) => {
            values.iter().any(|value| value_foreign(*value))
        }
        Object::Tagged { tag, payload } => value_foreign(*tag) || value_foreign(*payload),
        Object::Dict { shape, values } => {
            foreign(shape.storage) || values.iter().any(|value| value_foreign(*value))
        }
        Object::Closure { upvalues, .. } => upvalues.iter().any(|value| value_foreign(*value)),
        Object::Dyn {
            descriptor, value, ..
        } => value_foreign(*descriptor) || value_foreign(*value),
        Object::UpLink { value } => value.is_none_or(value_foreign),
        Object::ByteCodeProto {
            values,
            text,
            prototypes,
            ..
        } => {
            values.iter().any(|value| value_foreign(*value))
                || text.iter().any(|id| foreign(id.storage))
                || prototypes.iter().any(|prototype| match prototype {
                    RuntimePrototype::Bytecode(handle) => foreign(handle.storage),
                    RuntimePrototype::Native(_) => false,
                })
        }
        Object::String(_) | Object::Bytes(_) | Object::Opaque(_) | Object::NativeType(_) => false,
    }
}

fn builtin_atom(text: &str) -> Option<BuiltinAtom> {
    match text {
        "None" => Some(BuiltinAtom::None),
        "Some" => Some(BuiltinAtom::Some),
        "Ok" => Some(BuiltinAtom::Ok),
        "Err" => Some(BuiltinAtom::Err),
        "True" => Some(BuiltinAtom::True),
        "False" => Some(BuiltinAtom::False),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(name: &str, range: std::ops::Range<usize>) -> Loc {
        let mut sources = crate::SourceDatabase::default();
        let source = sources.add(name, "0123456789");
        Loc::from_usize(source, range).unwrap()
    }

    fn rv(value: RuntimeValue) -> RichValue {
        value.into()
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn rich_value_is_compact_and_copy() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<RichValue>();
        assert_eq!(std::mem::size_of::<RichValue>(), 32);
    }

    #[test]
    fn rich_value_equality_ignores_location() {
        let left = RichValue::new(RuntimeValue::Int(42), Some(location("left", 1..2)));
        let right = RichValue::new(RuntimeValue::Int(42), Some(location("right", 3..4)));

        assert_eq!(left, right);
    }

    #[test]
    fn call_site_rebasing_preserves_original_values_only() {
        let original_loc = location("data", 1..2);
        let generated_loc = location("function", 3..4);
        let call_loc = location("caller", 5..6);

        let original = RichValue::original(RuntimeValue::Int(1), Some(original_loc));
        let generated = RichValue::new(RuntimeValue::Int(2), Some(generated_loc));

        let preserved = original.rebase_generated(Some(call_loc));
        assert!(preserved.is_original());
        assert_eq!(preserved.loc(), Some(original_loc));
        assert_eq!(
            generated.rebase_generated(Some(call_loc)).loc(),
            Some(call_loc)
        );
        assert_eq!(
            RichValue::unknown(RuntimeValue::Int(3))
                .rebase_generated(Some(call_loc))
                .loc(),
            Some(call_loc)
        );
    }

    #[test]
    fn sourced_import_marks_root_and_children_original() {
        let root_loc = location("data", 0..5);
        let item_loc = location("data-item", 1..2);
        let sourced = SourcedValue {
            value: Value::Array(vec![Value::Int(7)].into()),
            provenance: crate::json::Provenance {
                values: [
                    (Vec::new(), root_loc),
                    (vec![ValuePathSegment::Index(0)], item_loc),
                ]
                .into_iter()
                .collect(),
                keys: Default::default(),
            },
        };
        let mut heap = Heap::work();
        let root = heap.import_sourced_value(None, &sourced).unwrap();
        assert!(root.is_original());
        assert_eq!(root.loc(), Some(root_loc));
        let RuntimeValue::Array(handle) = root.value else {
            panic!("expected imported Array")
        };
        let Object::Array(items) = heap.object(handle).unwrap() else {
            panic!("expected Array object")
        };
        assert!(items[0].is_original());
        assert_eq!(items[0].loc(), Some(item_loc));
    }

    #[test]
    fn copy_preserves_root_and_collection_edge_locations() {
        let root_loc = location("root", 0..5);
        let item_loc = location("item", 6..7);
        let mut world = Heap::main();
        let mut current = Heap::work();
        let array = current.allocate(Object::Array(
            vec![RichValue::original(RuntimeValue::Int(42), Some(item_loc))].into(),
        ));
        let root = RichValue::original(RuntimeValue::Array(array), Some(root_loc));

        let copied = copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[root],
        )
        .unwrap()[0];

        assert_eq!(copied.loc(), Some(root_loc));
        assert!(copied.is_original());
        let RuntimeValue::Array(handle) = copied.value else {
            panic!("expected copied Array")
        };
        let Object::Array(items) = world.object(handle).unwrap() else {
            panic!("expected copied Array object")
        };
        assert_eq!(items[0].loc(), Some(item_loc));
        assert!(items[0].is_original());
    }

    #[test]
    fn copy_is_reachable_reinterning_and_target_self_contained() {
        let mut world = Heap::main();
        let shared = world.allocate(Object::Bytes(vec![9].into()));
        let mut current = Heap::work();
        let atom = current.atom(Some(&world), "Custom");
        let string = current.string(Some(&world), "Custom");
        let root = current.allocate(Object::Tuple(
            vec![rv(atom), rv(string), rv(RuntimeValue::Bytes(shared))].into(),
        ));
        current.allocate(Object::Bytes(vec![1, 2, 3].into()));

        let copied = copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[rv(RuntimeValue::Tuple(root))],
        )
        .unwrap();

        assert_eq!(world.counts(), (2, 1, 0));
        let RuntimeValue::Tuple(root) = copied[0].value else {
            panic!("expected tuple root")
        };
        let Object::Tuple(values) = world.object(root).unwrap() else {
            panic!("expected tuple object")
        };
        assert_eq!(values[2], rv(RuntimeValue::Bytes(shared)));
        assert!(
            !values
                .iter()
                .any(|value| rich_value_contains_foreign(*value, Storage::Main))
        );
    }

    #[test]
    fn copy_preserves_cycles_and_failure_is_atomic() {
        let mut world = Heap::main();
        let mut current = Heap::work();
        let cycle = current.reserve();
        current
            .initialize(
                cycle,
                Object::Array(vec![rv(RuntimeValue::Array(cycle))].into()),
            )
            .unwrap();
        copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[rv(RuntimeValue::Array(cycle))],
        )
        .unwrap();
        assert_eq!(world.counts().0, 1);

        let before = world.counts();
        let invalid = RuntimeValue::Array(Handle {
            storage: Storage::Work,
            slot: 99,
        });
        assert!(
            copy_roots(
                &mut world,
                HeapView {
                    current: &current,
                    background: None,
                },
                &[rv(invalid)],
            )
            .is_err()
        );
        assert_eq!(world.counts(), before);
    }

    #[test]
    fn multiple_roots_share_one_forwarding_context() {
        let mut target = Heap::main();
        let mut source = Heap::work();
        let shared = source.allocate(Object::Bytes(vec![1].into()));
        let roots = copy_roots(
            &mut target,
            HeapView {
                current: &source,
                background: None,
            },
            &[
                rv(RuntimeValue::Bytes(shared)),
                rv(RuntimeValue::Bytes(shared)),
            ],
        )
        .unwrap();
        assert_eq!(roots[0], roots[1]);
        assert_eq!(target.counts().0, 1);
    }

    #[test]
    fn work_relocation_copies_work_edges_and_retains_main_edges() {
        let mut main = Heap::main();
        let stable = main.allocate(Object::Bytes(vec![9].into()));
        let mut source = Heap::work();
        let shared = source.allocate(Object::Bytes(vec![1, 2, 3].into()));
        let cycle = source.reserve();
        source
            .initialize(
                cycle,
                Object::Array(vec![rv(RuntimeValue::Array(cycle))].into()),
            )
            .unwrap();
        let root = source.allocate(Object::Tuple(
            vec![
                rv(RuntimeValue::Bytes(shared)),
                rv(RuntimeValue::Bytes(shared)),
                rv(RuntimeValue::Bytes(stable)),
                rv(RuntimeValue::Array(cycle)),
            ]
            .into(),
        ));
        source.allocate(Object::Bytes(vec![4, 5, 6].into()));
        let mut target = Heap::work();
        target.allocate(Object::String("existing".into()));

        let relocated = relocate_work_roots(
            &mut target,
            &main,
            &source,
            &[rv(RuntimeValue::Tuple(root))],
        )
        .unwrap();

        assert_eq!(target.counts().0, 4);
        let RuntimeValue::Tuple(root) = relocated[0].value else {
            panic!("expected relocated tuple")
        };
        let Object::Tuple(values) = target.object(root).unwrap() else {
            panic!("expected relocated tuple object")
        };
        assert_eq!(values[0], values[1]);
        assert_eq!(values[2], rv(RuntimeValue::Bytes(stable)));
        let RuntimeValue::Array(cycle) = values[3].value else {
            panic!("expected relocated cycle")
        };
        let Object::Array(cycle_values) = target.object(cycle).unwrap() else {
            panic!("expected relocated cycle object")
        };
        assert_eq!(cycle_values[0], rv(RuntimeValue::Array(cycle)));
        assert_ne!(root.slot, 0);
    }

    #[test]
    fn failed_nodes_relocate_between_work_worlds_but_cannot_enter_main_or_value() {
        let main = Heap::main();
        let mut source = Heap::work();
        let root = RichValue::unknown(RuntimeValue::Array(source.allocate(Object::Array(
            vec![RichValue::unknown(RuntimeValue::Failed(7))].into(),
        ))));

        let mut target = Heap::work();
        let relocated = relocate_work_roots(&mut target, &main, &source, &[root]).unwrap();
        let RuntimeValue::Array(handle) = relocated[0].value else {
            panic!("expected relocated Array")
        };
        let Object::Array(items) = target.object(handle).unwrap() else {
            panic!("expected relocated Array object")
        };
        assert!(matches!(items[0].value, RuntimeValue::Failed(7)));
        assert!(
            HeapView {
                current: &target,
                background: Some(&main),
            }
            .export_value(relocated[0])
            .is_err()
        );

        let mut destination = Heap::main();
        assert!(publish_root(&mut destination, &source, root).is_err());
    }

    #[test]
    fn publication_preserves_main_edges_and_relocates_work_edges() {
        let mut main = Heap::main();
        let stable = publish_value(&mut main, &Value::Bytes(vec![1, 2, 3].into()))
            .unwrap()
            .runtime();
        let mut work = Heap::work();
        let work_root = work.allocate(Object::Array(vec![stable].into()));

        let published = publish_root(&mut main, &work, rv(RuntimeValue::Array(work_root)))
            .unwrap()
            .runtime();
        let RuntimeValue::Array(main_root) = published.value else {
            panic!("expected published Array")
        };
        assert_eq!(main_root.storage, Storage::Main);
        let Object::Array(items) = main.object(main_root).unwrap() else {
            panic!("expected Main Array")
        };
        let RuntimeValue::Bytes(stable_bytes) = items[0].value else {
            panic!("expected Main Bytes")
        };
        assert_eq!(stable_bytes.storage, Storage::Main);
        assert_eq!(main.counts(), (2, 0, 0));
    }

    #[test]
    fn scalar_equality_compares_contents_across_storage() {
        let value = Value::string("same string that is too long for the short string form");
        let mut world = Heap::main();
        let persistent = publish_value(&mut world, &value).unwrap().runtime();
        let mut local = Heap::work();
        let local_value = local.import_value(Some(&world), &value).unwrap();
        assert!(
            HeapView {
                current: &local,
                background: Some(&world),
            }
            .values_equal(local_value, persistent)
            .unwrap()
        );

        let persistent_atom = publish_value(&mut world, &Value::atom("custom"))
            .unwrap()
            .runtime();
        let local_atom = local.import_value(None, &Value::atom("custom")).unwrap();
        assert!(
            HeapView {
                current: &local,
                background: Some(&world),
            }
            .values_equal(local_atom, persistent_atom)
            .unwrap()
        );

        let bytes = Value::Bytes(Arc::from(&b"same bytes"[..]));
        let persistent_bytes = publish_value(&mut world, &bytes).unwrap().runtime();
        let local_bytes = local.import_value(None, &bytes).unwrap();
        assert!(
            HeapView {
                current: &local,
                background: Some(&world),
            }
            .values_equal(local_bytes, persistent_bytes)
            .unwrap()
        );
    }

    #[test]
    fn opaque_values_preserve_nominal_identity_and_logical_equality() {
        let module = crate::value::NativeModuleId(7);
        let token_type = crate::NativeType::bind(
            crate::value::NativeTypeId { module, local: 0 },
            "fixture#Token",
        );
        let other_native_type = crate::NativeType::bind(
            crate::value::NativeTypeId { module, local: 1 },
            "fixture#Other",
        );
        let token = Value::Opaque(crate::OpaqueValue::new(token_type.clone(), 42_u64));
        let other_type = Value::Opaque(crate::OpaqueValue::new(other_native_type.clone(), 42_u64));
        let mut main = Heap::main();
        let persistent = publish_value(&mut main, &token).unwrap().runtime();
        let mut work = Heap::work();
        let local = work.import_value(Some(&main), &token).unwrap();
        let other = work.import_value(Some(&main), &other_type).unwrap();
        let view = HeapView {
            current: &work,
            background: Some(&main),
        };
        assert!(view.values_equal(local, persistent).unwrap());
        assert!(!view.values_equal(local, other).unwrap());
        let exported = view.export_value(local).unwrap();
        let Value::Opaque(exported) = exported else {
            panic!("expected Opaque value")
        };
        assert_eq!(exported.native_type().qualified_name(), "fixture#Token");
        assert_eq!(exported.downcast_ref::<u64>(&token_type), Some(&42));
        assert!(exported.downcast_ref::<u64>(&other_native_type).is_none());
    }

    #[test]
    fn composite_equality_uses_function_identity_at_func_leaves() {
        let tagged =
            Value::Tuple(vec![Value::atom("Ok"), Value::Array(vec![Value::Int(42)].into())].into());
        let mut world = Heap::main();
        let persistent = publish_value(&mut world, &tagged).unwrap().runtime();
        let mut local = Heap::work();
        let local_tagged = local.import_value(Some(&world), &tagged).unwrap();
        let function = Arc::new(crate::compile_source("test", "fn() { 1 }").unwrap());
        let closure = Arc::new(Closure::new(function, Vec::new()));
        let with_function = Value::Dict(Dict::new(
            Arc::new(Shape::from_sorted_fields(vec!["value".into()])),
            vec![Value::Array(vec![Value::Func(Arc::clone(&closure))].into())],
        ));
        let with_function = local.import_value(None, &with_function).unwrap();
        let same_identity = local
            .import_value(None, &Value::Func(Arc::clone(&closure)))
            .unwrap();
        let same_identity_again = local
            .import_value(None, &Value::Func(Arc::clone(&closure)))
            .unwrap();
        let promoted = publish_root(&mut world, &local, same_identity)
            .unwrap()
            .runtime();
        let different_identity = local
            .import_value(
                None,
                &Value::Func(Arc::new(Closure::new(
                    Arc::new(crate::compile_source("test", "fn() { 1 }").unwrap()),
                    Vec::new(),
                ))),
            )
            .unwrap();
        let view = HeapView {
            current: &local,
            background: Some(&world),
        };
        assert!(view.values_equal(local_tagged, persistent).unwrap());
        assert!(view.values_equal(with_function, with_function).unwrap());
        assert!(
            view.values_equal(same_identity, same_identity_again)
                .unwrap()
        );
        assert!(view.values_equal(same_identity, promoted).unwrap());
        assert!(
            !view
                .values_equal(same_identity, different_identity)
                .unwrap()
        );
    }

    #[test]
    fn structural_equality_terminates_on_internal_cycles() {
        let mut local = Heap::work();
        let left = local.reserve();
        local
            .initialize(
                left,
                Object::Array(vec![rv(RuntimeValue::Array(left))].into()),
            )
            .unwrap();
        let right = local.reserve();
        local
            .initialize(
                right,
                Object::Array(vec![rv(RuntimeValue::Array(right))].into()),
            )
            .unwrap();
        let world = Heap::main();
        assert!(
            HeapView {
                current: &local,
                background: Some(&world),
            }
            .values_equal(
                rv(RuntimeValue::Array(left)),
                rv(RuntimeValue::Array(right))
            )
            .unwrap()
        );
    }

    #[test]
    fn promotion_copies_ready_up_links_and_rejects_uninitialized_links() {
        let mut local = Heap::work();
        let link = local.allocate(Object::UpLink { value: None });
        let array = local.allocate(Object::Array(vec![rv(RuntimeValue::UpLink(link))].into()));
        local
            .initialize_up_link(link, rv(RuntimeValue::Array(array)))
            .unwrap();
        let mut world = Heap::main();
        let RuntimeValue::UpLink(persistent_link) =
            publish_root(&mut world, &local, rv(RuntimeValue::UpLink(link)))
                .unwrap()
                .runtime()
                .value
        else {
            panic!("expected persistent up-link")
        };
        let reader = Heap::work();
        let view = HeapView {
            current: &reader,
            background: Some(&world),
        };
        let RuntimeValue::Array(array) = view
            .up_link(persistent_link)
            .unwrap()
            .expect("published up-link is ready")
            .value
        else {
            panic!("expected Array")
        };
        assert_eq!(
            view.sequence(array, false).unwrap(),
            &[rv(RuntimeValue::UpLink(persistent_link))]
        );

        let mut uninitialized = Heap::work();
        let link = uninitialized.allocate(Object::UpLink { value: None });
        assert!(publish_root(&mut world, &uninitialized, rv(RuntimeValue::UpLink(link))).is_err());
    }

    #[test]
    fn dict_lookup_binary_searches_across_storage() {
        let value = Value::Dict(Dict::new(
            Arc::new(Shape::from_sorted_fields(vec![
                "a".into(),
                "b".into(),
                "c".into(),
            ])),
            vec![Value::Int(1), Value::Int(2), Value::Int(3)],
        ));
        let mut world = Heap::main();
        let RuntimeValue::Dict(dict) = publish_value(&mut world, &value).unwrap().runtime().value
        else {
            panic!("expected persistent Dict")
        };
        let mut local = Heap::work();
        let field = local.intern("b");
        let view = HeapView {
            current: &local,
            background: Some(&world),
        };
        assert_eq!(
            view.dict_get(dict, field).unwrap(),
            Some(rv(RuntimeValue::Int(2)))
        );
        assert_eq!(
            view.dict_get_text(dict, "c").unwrap(),
            Some(rv(RuntimeValue::Int(3)))
        );
    }

    #[test]
    fn linked_prototype_copy_shares_code_and_rebuilds_links() {
        let function = Arc::new(crate::compile_source("test", "fn(value) { value }").unwrap());
        let closure = Value::Func(Arc::new(Closure::new(
            Arc::clone(&function),
            vec![Value::string("capture")],
        )));
        let mut current = Heap::work();
        let root = current.import_value(None, &closure).unwrap();
        let mut world = Heap::main();
        let copied = copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[root],
        )
        .unwrap()[0];
        let local = Heap::work();
        let exported = HeapView {
            current: &local,
            background: Some(&world),
        }
        .export_value(copied)
        .unwrap();
        let Value::Func(exported) = exported else {
            panic!("expected exported closure")
        };
        let Prototype::Bytecode(exported_function) = exported.prototype() else {
            panic!("expected bytecode prototype")
        };
        assert!(Arc::ptr_eq(function.code(), exported_function.code()));
        assert!(
            matches!(exported.upvalues(), [Value::String(value)] if value.as_ref() == "capture")
        );
    }

    #[test]
    fn legacy_value_boundary_round_trips_heap_values() {
        let value = Value::Tuple(
            vec![
                Value::Int(42),
                Value::string("short"),
                Value::Atom(Atom::named("Custom")),
                Value::Array(vec![Value::Float(1.5)].into()),
            ]
            .into(),
        );
        let mut heap = Heap::work();
        let runtime = heap.import_value(None, &value).unwrap();
        let exported = HeapView {
            current: &heap,
            background: None,
        }
        .export_value(runtime)
        .unwrap();
        assert_eq!(exported.to_string(), value.to_string());
    }

    #[test]
    fn legacy_value_boundary_rejects_non_finite_float() {
        let mut heap = Heap::work();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let error = heap.import_value(None, &Value::Float(value)).unwrap_err();
            assert_eq!(error.to_string(), "Telora Float must be finite");
        }

        let runtime = RichValue::unknown(RuntimeValue::Float(f64::NAN));
        let error = HeapView {
            current: &heap,
            background: None,
        }
        .export_value(runtime)
        .unwrap_err();
        assert_eq!(error.to_string(), "Telora Float must be finite");

        let mut world = Heap::main();
        let error = publish_root(&mut world, &heap, runtime).unwrap_err();
        assert_eq!(error.to_string(), "Telora Float must be finite");
    }
}
