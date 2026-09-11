fn copy_roots(
    target: &mut Heap,
    source: HeapView<'_>,
    roots: &[Val],
) -> Result<Vec<Val>, HeapError> {
    let mut pending = PendingCopy::new(target, &source);
    let roots = roots
        .iter()
        .map(|root| pending.copy_value(target, &source, *root))
        .collect::<Result<Vec<_>, _>>()?;
    pending.validate()?;
    pending.commit(target);
    Ok(roots)
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
    objects_forwarded: HashMap<u32, u32>,
    text_forwarded: HashMap<InternId, InternId>,
    shapes_forwarded: HashMap<ShapeId, ShapeId>,
    native_types: HashMap<crate::value::NativeTypeId, crate::NativeType>,
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
            native_types: HashMap::new(),
        }
    }

    fn copy_value(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        value: Val,
    ) -> Result<Val, HeapError> {
        let copied = match value.value() {
            DecodedValue::SolvedType(_) => return Err(HeapError("solved type metadata cannot be copied through the legacy world publisher")),
            // Failure ids belong to the Main world's stable failure arena.
            // Work executions inherit that arena as a prefix and append new
            // roots, so the identity does not need relocation during copy.
            DecodedValue::Failed(id) => DecodedValue::Failed(id),
            DecodedValue::Int(_)
            | DecodedValue::BuiltinAtom(_)
            | DecodedValue::InlineAtom(_)
            | DecodedValue::InlineString(_) => value.value(),
            DecodedValue::Float(float) if float.is_finite() => value.value(),
            DecodedValue::Float(_) => return Err(HeapError("Telora Float must be finite")),
            DecodedValue::Atom(id) => DecodedValue::Atom(self.copy_text(target, source, id)?),
            DecodedValue::ShortString(id) => {
                DecodedValue::ShortString(self.copy_text(target, source, id)?)
            }
            DecodedValue::Bytes(handle) => {
                DecodedValue::Bytes(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Opaque(handle) => {
                DecodedValue::Opaque(self.copy_object(target, source, handle)?)
            }
            DecodedValue::NativeType(id) => {
                self.copy_native_type(target, source, id)?;
                DecodedValue::NativeType(id)
            }
            DecodedValue::DeclaredType(handle) => {
                DecodedValue::DeclaredType(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Array(handle) => {
                DecodedValue::Array(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Tuple(handle) => {
                DecodedValue::Tuple(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Tagged(handle) => {
                DecodedValue::Tagged(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Dict(handle) => {
                DecodedValue::Dict(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Func(handle) => {
                DecodedValue::Func(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Dyn(handle) => {
                DecodedValue::Dyn(self.copy_object(target, source, handle)?)
            }
        };
        let mut copied = value.with_value(copied).without_type_id();
        if let Some(type_id) = value.type_id() {
            if target.declared_types.contains_key(&type_id) {
                return Ok(copied.with_type_id(type_id));
            }
            let owner = source
                .type_witness(value)?
                .expect("value with a TypeId has registered metadata");
            let DecodedValue::DeclaredType(owner_handle) = owner.value() else {
                unreachable!("type metadata is a declared Type")
            };
            let Object::DeclaredType { id, .. } = source.object(owner_handle)? else {
                unreachable!("type metadata is a declared Type")
            };
            let copied_id = self.canonical_declared_type_id(target, id)?;
            self.copy_object(target, source, owner_handle)?;
            copied = copied.with_type_id(copied_id);
        }
        Ok(copied)
    }

    fn canonical_declared_type_id(
        &self,
        target: &Heap,
        id: &crate::value::DeclaredTypeId,
    ) -> Result<crate::TypeId, HeapError> {
        let id = id.clone();
        target.canonical_declared_type_id(&id)
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
        if let Some(forwarded) = self.objects_forwarded.get(&handle.slot) {
            return Ok(Handle {
                storage: self.target_storage,
                slot: *forwarded,
            });
        }
        let object = source.object(handle)?;
        if matches!(object, Object::Reserved) {
            return Err(HeapError("cannot copy an uninitialized object"));
        }
        let copied = Handle {
            storage: self.target_storage,
            slot: self.object_base + self.objects.len() as u32,
        };
        self.objects_forwarded.insert(handle.slot, copied.slot);
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
        let copy_values = |this: &mut Self, values: &[Val]| {
            values
                .iter()
                .map(|value| this.copy_value(target, source, *value))
                .collect::<Result<Box<[_]>, _>>()
        };
        Ok(match object {
            Object::Reserved | Object::OpenFunc => {
                return Err(HeapError("cannot copy an uninitialized object"));
            }
            Object::Bytes(value) => Object::Bytes(value.clone()),
            Object::Opaque(value) => {
                let mut value = value.clone();
                value.traced = value
                    .traced
                    .iter()
                    .map(|value| self.copy_value(target, source, *value))
                    .collect::<Result<_, _>>()?;
                Object::Opaque(value)
            }
            Object::DeclaredType {
                id,
                name,
                body,
                sealed,
                application_arguments,
                ..
            } => {
                if !sealed {
                    return Err(HeapError("cannot copy an unsealed type ref"));
                }
                let application_arguments = if let Some(arguments) = application_arguments {
                    Some(
                        arguments
                            .iter()
                            .map(|argument| self.copy_value(target, source, *argument))
                            .collect::<Result<Box<[_]>, _>>()?,
                    )
                } else {
                    None
                };
                let id = id.clone();
                let type_id = target.canonical_declared_type_id(&id)?;
                Object::DeclaredType {
                    type_id,
                    id,
                    name: Arc::clone(name),
                    body: self.copy_value(target, source, *body)?,
                    sealed: true,
                    application_arguments,
                }
            }
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
            } => Object::Dyn {
                identity: Arc::clone(identity),
                descriptor: self.copy_value(target, source, *descriptor)?,
                value: self.copy_value(target, source, *value)?,
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

    fn copy_native_type(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        id: crate::value::NativeTypeId,
    ) -> Result<(), HeapError> {
        if target.native_types.contains_key(&id) || self.native_types.contains_key(&id) {
            return Ok(());
        }
        self.native_types
            .insert(id, source.native_type(id)?.clone());
        Ok(())
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
        let declared_types = self
            .objects
            .iter()
            .enumerate()
            .filter_map(|(index, object)| match object {
                Object::DeclaredType { type_id, .. } => Some((
                    *type_id,
                    Val::unknown(DecodedValue::DeclaredType(Handle {
                        storage: self.target_storage,
                        slot: self.object_base + index as u32,
                    })),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        target.objects.extend(self.objects);
        target.native_types.extend(self.native_types);
        for (type_id, value) in declared_types {
            target.declared_types.entry(type_id).or_insert(value);
        }
        for value in self.text.values {
            target.text.insert(&value);
        }
        for shape in self.shapes {
            target.intern_shape(shape.into_vec());
        }
    }
}

#[cfg(test)]
fn value_contains_foreign(value: DecodedValue, target: Storage) -> bool {
    match value {
        DecodedValue::Atom(id) | DecodedValue::ShortString(id) => id.storage != target,
        DecodedValue::NativeType(_) | DecodedValue::SolvedType(_) => false,
        DecodedValue::Bytes(handle)
        | DecodedValue::Opaque(handle)
        | DecodedValue::DeclaredType(handle)
        | DecodedValue::Array(handle)
        | DecodedValue::Tuple(handle)
        | DecodedValue::Tagged(handle)
        | DecodedValue::Dict(handle)
        | DecodedValue::Func(handle)
        | DecodedValue::Dyn(handle) => handle.storage != target,
        DecodedValue::Failed(_)
        | DecodedValue::Int(_)
        | DecodedValue::Float(_)
        | DecodedValue::BuiltinAtom(_)
        | DecodedValue::InlineAtom(_)
        | DecodedValue::InlineString(_) => false,
    }
}

#[cfg(test)]
fn val_contains_foreign(value: Val, target: Storage) -> bool {
    value_contains_foreign(value.value(), target)
}

fn object_contains_disallowed(
    object: &Object,
    target: Storage,
    background: Option<Storage>,
) -> bool {
    let foreign = |storage| storage != target && Some(storage) != background;
    let value_foreign = |value: Val| {

        match value.value() {
            DecodedValue::Atom(id) | DecodedValue::ShortString(id) => foreign(id.storage),
            DecodedValue::NativeType(_) | DecodedValue::SolvedType(_) => false,
            DecodedValue::Bytes(handle)
            | DecodedValue::Opaque(handle)
            | DecodedValue::DeclaredType(handle)
            | DecodedValue::Array(handle)
            | DecodedValue::Tuple(handle)
            | DecodedValue::Tagged(handle)
            | DecodedValue::Dict(handle)
            | DecodedValue::Func(handle)
            | DecodedValue::Dyn(handle) => foreign(handle.storage),
            DecodedValue::Failed(_)
            | DecodedValue::Int(_)
            | DecodedValue::Float(_)
            | DecodedValue::BuiltinAtom(_)
            | DecodedValue::InlineAtom(_)
            | DecodedValue::InlineString(_) => false,
        }
    };
    match object {
        Object::Reserved | Object::OpenFunc => true,
        Object::DeclaredType { sealed: false, .. } => true,
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
        Object::DeclaredType { body, .. } => value_foreign(*body),
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
        Object::Opaque(value) => value.traced.iter().any(|value| value_foreign(*value)),
        Object::Bytes(_) => false,
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
