impl Heap {
    fn new(storage: Storage, types: crate::type_store::SharedTypeStore) -> Self {
        Self {
            storage,
            types,
            objects: Vec::new(),
            text: TextTable::default(),
            native_types: HashMap::new(),
            shapes: Vec::new(),
            shape_slots: HashMap::new(),
            bootstrap_root: None,
            functions: HashMap::new(),
            declared_types: HashMap::new(),
            properties: BTreeMap::new(),
            property_attr_type: None,
            memoized_interpreters: HashMap::new(),
        }
    }

    pub(crate) fn memoized_interpreter(
        &self,
        identity: usize,
        arguments: &[crate::TypeId],
    ) -> Option<Val> {
        self.memoized_interpreters
            .get(&identity)?
            .get(arguments)
            .copied()
    }

    pub(crate) fn memoize_interpreter(
        &mut self,
        identity: usize,
        arguments: Vec<crate::TypeId>,
        value: Val,
    ) {
        self.memoized_interpreters
            .entry(identity)
            .or_default()
            .entry(arguments)
            .or_insert(value);
    }

    #[cfg(test)]
    pub(crate) fn memoized_interpreter_count(&self) -> usize {
        self.memoized_interpreters.values().map(HashMap::len).sum()
    }

    #[cfg(test)]
    pub(crate) fn allocation_count(&self) -> usize {
        self.objects.len()
    }

    pub(crate) fn preallocate_func(&mut self, id: crate::FuncId) -> Result<(), HeapError> {
        if self.storage != Storage::Main {
            return Err(HeapError(
                "static function slots must be preallocated in Main world",
            ));
        }
        if self.functions.insert(id, None).is_some() {
            return Err(HeapError("duplicate static function slot"));
        }
        Ok(())
    }

    pub(crate) fn seal_static_func(
        &mut self,
        id: crate::FuncId,
        value: Val,
    ) -> Result<(), HeapError> {
        if !matches!(
            value.value(),
            DecodedValue::Func(_) | DecodedValue::FuncRef(_)
        ) {
            return Err(HeapError(
                "static function definition did not produce a closure",
            ));
        }
        match self.functions.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) if self.storage == Storage::Work => {
                entry.insert(Some(value));
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(mut entry) if entry.get().is_none() => {
                entry.insert(Some(value));
                Ok(())
            }
            std::collections::hash_map::Entry::Vacant(_) => {
                Err(HeapError("unknown static function slot"))
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(HeapError("static function slot is already sealed"))
            }
        }
    }

    pub(crate) fn static_func(&self, id: crate::FuncId) -> Option<Val> {
        self.functions.get(&id).copied().flatten()
    }

    pub(crate) fn bootstrap_root(&self) -> Option<PersistentValue> {
        self.bootstrap_root
    }

    pub(crate) fn set_bootstrap_root(&mut self, root: PersistentValue) {
        debug_assert!(self.bootstrap_root.is_none());
        self.bootstrap_root = Some(root);
    }

    pub(crate) fn module(
        &mut self,
        entries: impl IntoIterator<Item = (String, Val)>,
    ) -> Result<Val, HeapError> {
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(HeapError("Module exports contain a duplicate field"));
        }
        let mut fields = Vec::with_capacity(entries.len());
        let mut values = Vec::with_capacity(entries.len());
        for (field, value) in entries {
            fields.push(self.intern(&field));
            values.push(value);
        }
        let shape = self.intern_shape(fields);
        let handle = self.allocate(Object::Module {
            exports: ExportTable {
                shape,
                values: values.into_boxed_slice(),
            },
        });
        Ok(Val::unknown(DecodedValue::Module(handle)))
    }

    pub(crate) fn seal_module(&mut self, root: Val) -> Result<Val, HeapError> {
        if matches!(root.value(), DecodedValue::Module(_)) {
            return Ok(root);
        }
        let DecodedValue::Dict(handle) = root.value() else {
            return Err(HeapError(
                "module evaluation must produce a Dict of exports",
            ));
        };
        if handle.storage != self.storage {
            return Err(HeapError("module exports Dict belongs to another world"));
        }
        let object = self
            .objects
            .get_mut(handle.slot as usize)
            .ok_or(HeapError("module exports Dict is out of bounds"))?;
        let Object::Dict { shape, values } = std::mem::replace(object, Object::Reserved) else {
            return Err(HeapError("module exports handle has another object kind"));
        };
        *object = Object::Module {
            exports: ExportTable { shape, values },
        };
        Ok(root.with_value(DecodedValue::Module(handle)))
    }

    pub(crate) fn work() -> Self {
        Self::new(Storage::Work, crate::type_store::shared_type_store())
    }

    pub(crate) fn main() -> Self {
        Self::new(Storage::Main, crate::type_store::shared_type_store())
    }

    pub(crate) fn work_for(background: &Self) -> Self {
        Self::new(Storage::Work, Arc::clone(&background.types))
    }

    pub(crate) fn canonical_declared_type_id(
        &self,
        declared: &crate::value::DeclaredTypeId,
    ) -> Result<crate::TypeId, HeapError> {
        let mut types = self
            .types
            .lock()
            .map_err(|_| HeapError("type store poisoned"))?;
        let arguments = declared
            .arguments()
            .iter()
            .map(|argument| types.intern_descriptor(argument))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                HeapError::owned(format!("declared type argument is not canonical: {error}"))
            })?;
        Ok(match types.begin(declared.constructor(), arguments) {
            crate::type_store::InternType::Existing(id)
            | crate::type_store::InternType::Reserved(id) => id,
        })
    }

    pub(crate) fn canonical_descriptor_type_id(
        &self,
        descriptor: &crate::types::TypeDescriptor,
    ) -> Result<crate::TypeId, HeapError> {
        self.types
            .lock()
            .map_err(|_| HeapError("type store poisoned"))?
            .intern_descriptor(descriptor)
            .map_err(HeapError::owned)
    }

    pub(crate) fn canonical_type_value_id(
        &self,
        value: crate::ValueRef<'_>,
        path: &str,
    ) -> Result<crate::TypeId, HeapError> {
        crate::types::canonical_type_ref_id(value, path, &self.types).map_err(HeapError::owned)
    }

    pub(crate) fn canonical_type_name(
        &self,
        type_id: crate::TypeId,
    ) -> Result<Option<String>, HeapError> {
        let types = self
            .types
            .lock()
            .map_err(|_| HeapError("type store poisoned"))?;
        Ok(types.get(type_id).map(|data| data.name.clone()))
    }

    pub(crate) fn property_attr_value(&mut self, type_id: crate::TypeId, bits: u32) -> Val {
        let field = self.intern("bits");
        let shape = self.intern_shape(vec![field]);
        Val::unknown(DecodedValue::Dict(self.allocate(Object::Dict {
            shape,
            values: Box::new([self.int(i64::from(bits))]),
        })))
        .with_type_id(type_id)
    }

    pub(crate) fn option_value(&mut self, value: Option<Val>) -> Val {
        let Some(value) = value else {
            return Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None));
        };
        Val::new(
            DecodedValue::Tagged(self.allocate(Object::Tagged {
                tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), value.loc()),
                payload: value,
            })),
            value.loc(),
        )
    }

    pub(crate) fn stage_property(
        &mut self,
        key: PropertyKey,
        value: Val,
    ) -> Result<(), HeapError> {
        if self.storage != Storage::Work {
            return Err(HeapError("property staging requires a Work world"));
        }
        if value.type_id() != Some(key.property_type()) {
            return Err(HeapError(
                "staged property runtime witness does not match its property TypeId",
            ));
        }
        self.properties.insert(key, value);
        Ok(())
    }

    pub(crate) fn property_attr_type(&self) -> Option<crate::TypeId> {
        self.property_attr_type
    }

    pub(crate) fn establish_property_attr_type(
        &mut self,
        type_id: crate::TypeId,
    ) -> Result<(), HeapError> {
        if self.storage != Storage::Work {
            return Err(HeapError(
                "PropertyAttr staging requires a Work world",
            ));
        }
        match self.property_attr_type {
            Some(existing) if existing != type_id => {
                Err(HeapError("PropertyAttr TypeId is already established"))
            }
            _ => {
                self.property_attr_type = Some(type_id);
                Ok(())
            }
        }
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

    pub(crate) fn allocate_declared_type(&mut self, object: Object) -> Handle {
        let Object::DeclaredType { type_id, .. } = &object else {
            panic!("allocate_declared_type requires declared type metadata")
        };
        let type_id = *type_id;
        let handle = self.allocate(object);
        self.declared_types
            .entry(type_id)
            .or_insert_with(|| Val::unknown(DecodedValue::DeclaredType(handle)));
        handle
    }

    fn intern_native_type(&mut self, value: crate::NativeType) -> crate::value::NativeTypeId {
        let id = value.id();
        self.native_types.entry(id).or_insert(value);
        id
    }

    fn native_type(&self, id: crate::value::NativeTypeId) -> Result<&crate::NativeType, HeapError> {
        self.native_types
            .get(&id)
            .ok_or(HeapError("native type ID is not registered in this world"))
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

    pub(crate) fn initialize_type_slot(
        &mut self,
        handle: Handle,
        value: Val,
    ) -> Result<(), HeapError> {
        if handle.storage != Storage::Work {
            return Err(HeapError("Main up-links are read-only"));
        }
        let Object::TypeSlot { value: slot } = self.object_mut(handle)? else {
            return Err(HeapError("handle is not an up-link"));
        };
        if slot.is_some() {
            return Err(HeapError("up-link is already initialized"));
        }
        *slot = Some(value);
        Ok(())
    }

    pub(crate) fn seal_local_func(
        &mut self,
        target: Handle,
        source: Handle,
    ) -> Result<(), HeapError> {
        if target.storage != Storage::Work || source.storage != Storage::Work {
            return Err(HeapError(
                "function refs can only be sealed in their Work world",
            ));
        }
        let closure = match self.object(source)? {
            Object::Closure { .. } => self.object(source)?.clone(),
            _ => return Err(HeapError("function ref source is not a sealed function")),
        };
        let slot = self.object_mut(target)?;
        if !matches!(slot, Object::OpenFunc) {
            return Err(HeapError("function ref is already sealed"));
        }
        *slot = closure;
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

    pub(crate) fn string(&mut self, background: Option<&Heap>, text: &str) -> DecodedValue {
        if let Some(text) = InlineText::new(text) {
            DecodedValue::InlineString(text)
        } else {
            if let Some(id) = background.and_then(|heap| heap.find_text(text)) {
                DecodedValue::ShortString(id)
            } else {
                DecodedValue::ShortString(self.intern(text))
            }
        }
    }

    pub(crate) fn atom(&mut self, background: Option<&Heap>, text: &str) -> DecodedValue {
        if let Some(builtin) = builtin_atom(text) {
            DecodedValue::BuiltinAtom(builtin)
        } else if let Some(text) = InlineText::new(text) {
            DecodedValue::InlineAtom(text)
        } else if let Some(id) = background.and_then(|heap| heap.find_text(text)) {
            DecodedValue::Atom(id)
        } else {
            DecodedValue::Atom(self.intern(text))
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

    pub(crate) fn link_bytecode_resolved(
        &mut self,
        background: Option<&Heap>,
        function: &BytecodeFunction,
        externals: &HashMap<String, Val>,
    ) -> Result<Handle, HeapError> {
        self.link_bytecode_with(background, function, externals, &mut HashMap::new())
    }

    fn link_bytecode_with(
        &mut self,
        background: Option<&Heap>,
        function: &BytecodeFunction,
        externals: &HashMap<String, Val>,
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
                    let resolved = externals
                        .get(key)
                        .copied()
                        .ok_or(HeapError("external value link is unresolved"))?;
                    if key.starts_with("\0declared-owner:")
                        && !matches!(resolved.value(), DecodedValue::DeclaredType(_))
                    {
                        return Err(HeapError(
                            "declared owner external link did not resolve to a TypeRef",
                        ));
                    }
                    return Ok(resolved);
                }
                Ok(match value {
                    Constant::Placeholder => {
                        return Err(HeapError("unresolved bytecode constant placeholder"));
                    }
                    Constant::Int(value) => Val::unknown(DecodedValue::Int(*value)),
                    Constant::Float(value) if value.is_finite() => {
                        Val::unknown(DecodedValue::Float(*value))
                    }
                    Constant::Float(_) => return Err(HeapError("Telora Float must be finite")),
                    Constant::String(value) => Val::unknown(self.string(background, value)),
                    Constant::Bytes(value) => Val::unknown(DecodedValue::Bytes(
                        self.allocate(Object::Bytes(value.as_ref().into())),
                    )),
                    Constant::Atom(value) => Val::unknown(self.atom(background, value.name())),
                    Constant::Native(function) => self.native_closure(*function, []),
                })
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
