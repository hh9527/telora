#[derive(Clone, Copy)]
pub(crate) struct HeapView<'a> {
    pub(crate) current: &'a Heap,
    pub(crate) background: Option<&'a Heap>,
}

type BytecodeLinks<'a> = (
    &'a Arc<FuncByteCode>,
    &'a [Val],
    &'a [InternId],
    &'a [RuntimePrototype],
);

impl<'a> HeapView<'a> {
    pub(crate) fn static_func(&self, id: crate::FuncId) -> Option<Val> {
        self.current
            .static_func(id)
            .or_else(|| self.background.and_then(|heap| heap.static_func(id)))
    }

    pub(crate) fn resolve_func(&self, mut value: Val) -> Result<Option<Handle>, HeapError> {
        let mut visited = HashSet::new();
        loop {
            match value.value() {
                DecodedValue::Func(handle) => return Ok(Some(handle)),
                DecodedValue::FuncRef(id) => {
                    if !visited.insert(id) {
                        return Err(HeapError("cyclic static function alias"));
                    }
                    value = self
                        .static_func(id)
                        .ok_or(HeapError("static function slot is not sealed"))?;
                }
                _ => return Ok(None),
            }
        }
    }

    pub(crate) fn resolved_function_arity(&self, value: Val) -> Result<Option<usize>, HeapError> {
        self.resolve_func(value)?
            .map(|handle| self.function_arity(handle))
            .transpose()
    }

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

    pub(crate) fn unwrap_declared(&self, mut value: Val) -> Result<Val, HeapError> {
        if value.type_id().is_some() {
            self.type_witness(value)?;
            value = value.without_type_id();
        }
        Ok(value)
    }

    pub(crate) fn type_witness(&self, value: Val) -> Result<Option<Val>, HeapError> {
        let Some(type_id) = value.type_id() else {
            return Ok(None);
        };
        let owner = self
            .current
            .declared_types
            .get(&type_id)
            .or_else(|| self.background?.declared_types.get(&type_id))
            .copied();
        let owner = owner.ok_or(HeapError("canonical type ID has no metadata in this world"))?;
        let DecodedValue::DeclaredType(handle) = owner.value() else {
            return Err(HeapError("type metadata has another value kind"));
        };
        if !matches!(self.object(handle)?, Object::DeclaredType { .. }) {
            return Err(HeapError("type metadata refers to another object kind"));
        }
        Ok(Some(owner))
    }

    pub(crate) fn declared_type_id(&self, owner: Val) -> Result<crate::TypeId, HeapError> {
        let DecodedValue::DeclaredType(handle) = owner.value() else {
            return Err(HeapError("declared value owner is not a declared Type"));
        };
        let Object::DeclaredType {
            type_id, sealed, ..
        } = self.object(handle)?
        else {
            return Err(HeapError("declared value owner has another object kind"));
        };
        if !sealed {
            return Err(HeapError("declared value owner is not sealed"));
        }
        Ok(*type_id)
    }

    pub(crate) fn canonical_type_name(
        &self,
        type_id: crate::TypeId,
    ) -> Result<Option<String>, HeapError> {
        self.current.canonical_type_name(type_id)
    }

    pub(crate) fn text(&self, id: InternId) -> Result<&'a str, HeapError> {
        self.heap(id.storage)?.resolve_text(id)
    }

    pub(crate) fn native_type(
        &self,
        id: crate::value::NativeTypeId,
    ) -> Result<&'a crate::NativeType, HeapError> {
        self.current.native_type(id).or_else(|_| {
            self.background
                .ok_or(HeapError("native type ID is not registered in this view"))?
                .native_type(id)
        })
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
    ) -> Result<(RuntimePrototype, &'a [Val]), HeapError> {
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

    pub(crate) fn type_slot(&self, handle: Handle) -> Result<Option<Val>, HeapError> {
        let Object::TypeSlot { value } = self.object(handle)? else {
            return Err(HeapError("handle is not an up-link"));
        };
        Ok(*value)
    }

    pub(crate) fn dyn_parts(&self, handle: Handle) -> Result<(&'a Arc<()>, Val, Val), HeapError> {
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

    pub(crate) fn sequence(&self, handle: Handle, tuple: bool) -> Result<&'a [Val], HeapError> {
        match self.object(handle)? {
            Object::Array(values) if !tuple => Ok(values),
            Object::Tuple(values) if tuple => Ok(values),
            _ => Err(HeapError("handle is not the requested sequence kind")),
        }
    }

    pub(crate) fn tagged(&self, handle: Handle) -> Result<(Val, Val), HeapError> {
        let Object::Tagged { tag, payload } = self.object(handle)? else {
            return Err(HeapError("handle is not a Tagged value"));
        };
        Ok((*tag, *payload))
    }

    pub(crate) fn dict_get(
        &self,
        handle: Handle,
        field: InternId,
    ) -> Result<Option<Val>, HeapError> {
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

    pub(crate) fn exports_get(
        &self,
        handle: Handle,
        field: InternId,
    ) -> Result<Option<Val>, HeapError> {
        let Object::Module { exports, .. } = self.object(handle)? else {
            return Err(HeapError("handle is not a Module"));
        };
        let wanted = self.text(field)?;
        let fields = self.shape(exports.shape)?;
        let index = fields
            .binary_search_by(|candidate| {
                if *candidate == field {
                    Ordering::Equal
                } else {
                    self.text(*candidate).unwrap_or("").cmp(wanted)
                }
            })
            .ok();
        Ok(index.and_then(|index| exports.values.get(index).copied()))
    }

    pub(crate) fn exports_fields(&self, handle: Handle) -> Result<Vec<&'a str>, HeapError> {
        let Object::Module { exports, .. } = self.object(handle)? else {
            return Err(HeapError("handle is not a Module"));
        };
        self.shape(exports.shape)?
            .iter()
            .map(|field| self.text(*field))
            .collect()
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

    pub(crate) fn module_fields(&self, handle: Handle) -> Result<Vec<&'a str>, HeapError> {
        let Object::Module { exports } = self.object(handle)? else {
            return Err(HeapError("handle is not a Module"));
        };
        self.shape(exports.shape)?
            .iter()
            .map(|field| self.text(*field))
            .collect()
    }

    pub(crate) fn module_get_text(
        &self,
        handle: Handle,
        field: &str,
    ) -> Result<Option<Val>, HeapError> {
        let Object::Module { exports } = self.object(handle)? else {
            return Err(HeapError("handle is not a Module"));
        };
        let fields = self.shape(exports.shape)?;
        let index = fields
            .binary_search_by(|candidate| self.text(*candidate).unwrap_or("").cmp(field))
            .ok();
        Ok(index.and_then(|index| exports.values.get(index).copied()))
    }

    pub(crate) fn dict_parts(
        &self,
        handle: Handle,
    ) -> Result<(&'a [InternId], &'a [Val]), HeapError> {
        let Object::Dict { shape, values } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        Ok((self.shape(*shape)?, values))
    }

    pub(crate) fn dict_get_text(
        &self,
        handle: Handle,
        field: &str,
    ) -> Result<Option<Val>, HeapError> {
        let Object::Dict { shape, values } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        let fields = self.shape(*shape)?;
        let index = fields
            .binary_search_by(|candidate| self.text(*candidate).unwrap_or("").cmp(field))
            .ok();
        Ok(index.and_then(|index| values.get(index).copied()))
    }

    pub(crate) fn string_text(&self, value: Val) -> Result<Option<TextRef<'a>>, HeapError> {
        match value.value() {
            DecodedValue::InlineString(text) => Ok(Some(TextRef::inline(text))),
            DecodedValue::ShortString(id) => Ok(Some(TextRef::borrowed(self.text(id)?))),
            _ => Ok(None),
        }
    }

    pub(crate) fn atom_text(&self, value: Val) -> Result<Option<TextRef<'a>>, HeapError> {
        match value.value() {
            DecodedValue::BuiltinAtom(atom) => Ok(Some(TextRef::borrowed(atom.name()))),
            DecodedValue::InlineAtom(text) => Ok(Some(TextRef::inline(text))),
            DecodedValue::Atom(id) => Ok(Some(TextRef::borrowed(self.text(id)?))),
            _ => Ok(None),
        }
    }

    pub(crate) fn values_equal(&self, left: Val, right: Val) -> Result<bool, HeapError> {
        self.values_equal_with(left, right, &mut HashSet::new())
    }

    /// Returns the first failed node reachable through data containers.
    ///
    /// Closures and opaque/native values are intentionally atomic here: an
    /// operation only depends on their identity, not on captured internals.
    pub(crate) fn first_data_failure(&self, root: Val) -> Result<Option<u32>, HeapError> {
        let mut pending = vec![root];
        let mut visited = HashSet::new();
        while let Some(value) = pending.pop() {
            let handle = match value.value() {
                DecodedValue::Failed(failure) => return Ok(Some(failure)),
                DecodedValue::Array(handle)
                | DecodedValue::Tuple(handle)
                | DecodedValue::Tagged(handle)
                | DecodedValue::Dict(handle)
                | DecodedValue::Dyn(handle)
                | DecodedValue::Module(handle) => handle,
                DecodedValue::Int(_)
                | DecodedValue::Float(_)
                | DecodedValue::BuiltinAtom(_)
                | DecodedValue::InlineAtom(_)
                | DecodedValue::Atom(_)
                | DecodedValue::InlineString(_)
                | DecodedValue::ShortString(_)
                | DecodedValue::Bytes(_)
                | DecodedValue::Opaque(_)
                | DecodedValue::NativeType(_)
                | DecodedValue::DeclaredType(_)
                | DecodedValue::SymbolicType(_)
                | DecodedValue::Func(_)
                | DecodedValue::TypeSlot(_)
                | DecodedValue::FuncRef(_) => continue,
            };
            if !visited.insert(handle) {
                continue;
            }
            match self.object(handle)? {
                Object::Array(values) | Object::Tuple(values) => {
                    pending.extend(values.iter().rev().copied());
                }
                Object::Tagged { tag, payload } => {
                    pending.push(*payload);
                    pending.push(*tag);
                }
                Object::Dict { values, .. } => {
                    pending.extend(values.iter().rev().copied());
                }
                Object::Module { exports, .. } => {
                    pending.extend(exports.values.iter().rev().copied());
                }
                Object::Dyn {
                    descriptor, value, ..
                } => {
                    pending.push(*value);
                    pending.push(*descriptor);
                }
                Object::Bytes(_)
                | Object::Opaque(_)
                | Object::DeclaredType { .. }
                | Object::SymbolicType { .. }
                | Object::Closure { .. }
                | Object::TypeSlot { .. }
                | Object::ByteCodeProto { .. }
                | Object::OpenFunc
                | Object::Reserved => {}
            }
        }
        Ok(None)
    }

    fn values_equal_with(
        &self,
        left: Val,
        right: Val,
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        match (left.type_id(), right.type_id()) {
            (Some(left_id), Some(right_id)) => {
                if left_id != right_id {
                    return Ok(false);
                }
            }
            (Some(_), None) | (None, Some(_)) => {
                return Ok(false);
            }
            (None, None) => {}
        }
        let left = left.without_type_id();
        let right = right.without_type_id();
        if matches!(left.value(), DecodedValue::FuncRef(_))
            || matches!(right.value(), DecodedValue::FuncRef(_))
        {
            let Some(left) = self.resolve_func(left)? else {
                return Ok(false);
            };
            let Some(right) = self.resolve_func(right)? else {
                return Ok(false);
            };
            let Object::Closure { identity: left, .. } = self.object(left)? else {
                return Err(HeapError("Func handle refers to another object kind"));
            };
            let Object::Closure {
                identity: right, ..
            } = self.object(right)?
            else {
                return Err(HeapError("Func handle refers to another object kind"));
            };
            return Ok(Arc::ptr_eq(left, right));
        }
        match (left.value(), right.value()) {
            (DecodedValue::Func(left), DecodedValue::Func(right)) => {
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
            (DecodedValue::Dyn(left), DecodedValue::Dyn(right)) => {
                let (left, _, _) = self.dyn_parts(left)?;
                let (right, _, _) = self.dyn_parts(right)?;
                Ok(Arc::ptr_eq(left, right))
            }
            (DecodedValue::TypeSlot(_), _) | (_, DecodedValue::TypeSlot(_)) => {
                Err(HeapError("up-link escaped into equality"))
            }
            (DecodedValue::Int(left), DecodedValue::Int(right)) => Ok(left == right),
            (DecodedValue::Float(left), DecodedValue::Float(right)) => Ok(left == right),
            (
                left @ (DecodedValue::BuiltinAtom(_)
                | DecodedValue::InlineAtom(_)
                | DecodedValue::Atom(_)),
                right @ (DecodedValue::BuiltinAtom(_)
                | DecodedValue::InlineAtom(_)
                | DecodedValue::Atom(_)),
            ) => Ok(self.atom_text(left.into())? == self.atom_text(right.into())?),
            (
                left @ (DecodedValue::InlineString(_) | DecodedValue::ShortString(_)),
                right @ (DecodedValue::InlineString(_) | DecodedValue::ShortString(_)),
            ) => Ok(self.string_text(left.into())? == self.string_text(right.into())?),
            (DecodedValue::Bytes(left), DecodedValue::Bytes(right)) => {
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
            (DecodedValue::Opaque(left), DecodedValue::Opaque(right)) => {
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
            (DecodedValue::NativeType(left), DecodedValue::NativeType(right)) => Ok(left == right),
            (DecodedValue::DeclaredType(left), DecodedValue::DeclaredType(right)) => {
                let left_handle = left;
                let right_handle = right;
                let Object::DeclaredType { type_id: left, .. } = self.object(left_handle)? else {
                    return Err(HeapError(
                        "DeclaredType handle refers to another object kind",
                    ));
                };
                let Object::DeclaredType { type_id: right, .. } = self.object(right_handle)? else {
                    return Err(HeapError(
                        "DeclaredType handle refers to another object kind",
                    ));
                };
                Ok(left == right)
            }
            (DecodedValue::Array(left), DecodedValue::Array(right))
            | (DecodedValue::Tuple(left), DecodedValue::Tuple(right)) => {
                self.sequence_handles_equal(left, right, visited)
            }
            (DecodedValue::Tagged(left), DecodedValue::Tagged(right)) => {
                if left == right || !visited.insert((left, right)) {
                    return Ok(true);
                }
                let (left_tag, left_payload) = self.tagged(left)?;
                let (right_tag, right_payload) = self.tagged(right)?;
                Ok(self.values_equal_with(left_tag, right_tag, visited)?
                    && self.values_equal_with(left_payload, right_payload, visited)?)
            }
            (DecodedValue::Dict(left), DecodedValue::Dict(right)) => {
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
        left: &[Val],
        right: &[Val],
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(right) {
            if !self.values_equal_with(*left, *right, visited)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
