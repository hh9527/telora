impl Heap {
    fn new(storage: Storage, types: crate::type_store::SharedTypeStore) -> Self {
        Self {
            storage,
            solved_types: None,
            solved_graph: None,
            solved_evaluation: None,
            solved_tasks: vec![],
            solved_failures: vec![],
            types,
            objects: Vec::new(),
            text: TextTable::default(),
            native_types: HashMap::new(),
            shapes: Vec::new(),
            shape_slots: HashMap::new(),
            declared_types: HashMap::new(),
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

    pub(crate) fn allocation_count(&self) -> usize {
        self.objects.len()
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
                    Constant::SolvedNative { function, signature, native_type } => {
                        let types = self.solved_types.as_ref().or_else(|| background.and_then(|h| h.solved_types.as_ref()));
                        if types.is_none_or(|types| signature.index() >= types.types.len()) {
                            return Err(HeapError("native signature is not in the solved type image"));
                        }
                        let mut captures = vec![];
                        if let Some(ty) = native_type {
                            captures.push(self.native_type_value(ty.clone()));
                        }
                        captures.push(Val::unknown(DecodedValue::SolvedType(*signature)));
                        self.native_closure(*function, captures)
                    }
                    Constant::SolvedType(id) => {
                        let types = self.solved_types.as_ref().or_else(|| background.and_then(|h| h.solved_types.as_ref()));
                        if types.is_none_or(|types| id.index() >= types.types.len()) {
                            return Err(HeapError("type metadata ID is not in the solved type image"));
                        }
                        Val::unknown(DecodedValue::SolvedType(*id))
                    }
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
