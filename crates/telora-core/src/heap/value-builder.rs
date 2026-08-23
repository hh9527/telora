impl Heap {
    fn record_value(
        &mut self,
        entries: impl IntoIterator<Item = (String, Val)>,
    ) -> Result<Val, HeapError> {
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let fields = entries
            .iter()
            .map(|(name, _)| self.intern(name))
            .collect::<Vec<_>>();
        let values = entries
            .into_iter()
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        let shape = self.intern_shape(fields);
        Ok(Val::unknown(DecodedValue::Dict(self.allocate(
            Object::Dict {
                shape,
                values: values.into_boxed_slice(),
            },
        ))))
    }

    pub(crate) fn normalized_bool_type_value(
        &mut self,
        background: Option<&Heap>,
    ) -> Result<Val, HeapError> {
        fn wrap(heap: &mut Heap, background: Option<&Heap>, inner: Val) -> Result<Val, HeapError> {
            let attributes = heap.record_value([])?;
            let kind = Val::unknown(heap.atom(background, "WithAttributes"));
            heap.record_value([
                ("attributes".into(), attributes),
                ("inner".into(), inner),
                ("kind".into(), kind),
            ])
        }

        let none = Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None));
        let false_variant = wrap(self, background, none)?;
        let true_variant = wrap(self, background, none)?;
        let variants = self.record_value([
            ("False".into(), false_variant),
            ("True".into(), true_variant),
        ])?;
        let kind = Val::unknown(self.atom(background, "Enum"));
        let metadata = self.record_value([("kind".into(), kind), ("variants".into(), variants)])?;
        wrap(self, background, metadata)
    }

    pub(crate) fn type_descriptor_value(
        &mut self,
        background: Option<&Heap>,
        descriptor: &crate::types::TypeDescriptor,
    ) -> Result<Val, HeapError> {
        fn record(
            heap: &mut Heap,
            entries: impl IntoIterator<Item = (String, Val)>,
        ) -> Result<Val, HeapError> {
            let mut entries = entries.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let fields = entries
                .iter()
                .map(|(name, _)| heap.intern(name))
                .collect::<Vec<_>>();
            let values = entries
                .into_iter()
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            let shape = heap.intern_shape(fields);
            Ok(Val::unknown(DecodedValue::Dict(heap.allocate(
                Object::Dict {
                    shape,
                    values: values.into_boxed_slice(),
                },
            ))))
        }

        fn build(
            heap: &mut Heap,
            background: Option<&Heap>,
            descriptor: &crate::types::TypeDescriptor,
            declared: &mut HashMap<crate::value::DeclaredTypeId, Val>,
        ) -> Result<Val, HeapError> {
            use crate::types::TypeDescriptor as T;

            let atom = |heap: &mut Heap, text: &str| Val::unknown(heap.atom(background, text));
            let kind = |heap: &mut Heap, name: &str| {
                let name = atom(heap, name);
                record(heap, [("kind".into(), name)])
            };
            match descriptor {
                T::Bound(parameter) => {
                    let kind = atom(heap, "Bound");
                    record(
                        heap,
                        [
                            ("kind".into(), kind),
                            (
                                "parameter".into(),
                                Val::unknown(DecodedValue::Int(i64::from(parameter.index()))),
                            ),
                        ],
                    )
                }
                T::Inference(_) => Err(HeapError(
                    "non-concrete type metadata cannot enter the runtime",
                )),
                T::Named(name) => {
                    let kind = atom(heap, "Named");
                    let name = Val::unknown(heap.string(background, name));
                    record(heap, [("kind".into(), kind), ("name".into(), name)])
                }
                T::Declared(value) => {
                    if let Some(existing) = declared.get(&value.id) {
                        return Ok(*existing);
                    }
                    let placeholder = kind(heap, "Any")?;
                    let owner = heap.reserve_type_metadata(
                        value.id.clone(),
                        value.name.as_str(),
                        placeholder,
                    )?;
                    declared.insert(value.id.clone(), owner);
                    let body = build(heap, background, &value.body, declared)?;
                    heap.seal_type_ref(owner, body)
                }
                T::Any => kind(heap, "Any"),
                T::Never => kind(heap, "Never"),
                T::Type => kind(heap, "Type"),
                T::Dyn => kind(heap, "Dyn"),
                T::Int => kind(heap, "Int"),
                T::Float => kind(heap, "Float"),
                T::String => kind(heap, "String"),
                T::Bytes => kind(heap, "Bytes"),
                T::TypeOf(instance) => {
                    let instance = build(heap, background, instance, declared)?;
                    let kind = atom(heap, "TypeOf");
                    record(heap, [("kind".into(), kind), ("instance".into(), instance)])
                }
                T::Opaque(native) => Ok(heap.native_type_value(native.clone())),
                T::Atom(tag) => {
                    let kind = atom(heap, "Atom");
                    let tag = atom(heap, tag.name());
                    record(heap, [("kind".into(), kind), ("tag".into(), tag)])
                }
                T::Array(item) | T::Dict(item) => {
                    let item = build(heap, background, item, declared)?;
                    let name = if matches!(descriptor, T::Array(_)) {
                        "Array"
                    } else {
                        "Dict"
                    };
                    let kind = atom(heap, name);
                    record(heap, [("kind".into(), kind), ("item".into(), item)])
                }
                T::Tagged { tag, payload } => {
                    let payload = build(heap, background, payload, declared)?;
                    let kind = atom(heap, "Tagged");
                    let tag = atom(heap, tag.name());
                    record(
                        heap,
                        [
                            ("kind".into(), kind),
                            ("tag".into(), tag),
                            ("payload".into(), payload),
                        ],
                    )
                }
                T::Tuple(items) | T::Union(items) => {
                    let items = items
                        .iter()
                        .map(|item| build(heap, background, item, declared))
                        .collect::<Result<Vec<_>, _>>()?;
                    let items = Val::unknown(DecodedValue::Array(
                        heap.allocate(Object::Array(items.into_boxed_slice())),
                    ));
                    let (name, field) = if matches!(descriptor, T::Tuple(_)) {
                        ("Tuple", "items")
                    } else {
                        ("Union", "variants")
                    };
                    let kind = atom(heap, name);
                    record(heap, [("kind".into(), kind), (field.into(), items)])
                }
                T::Struct(fields) => {
                    let fields = fields
                        .iter()
                        .map(|(name, value)| {
                            build(heap, background, value, declared)
                                .map(|value| (name.clone(), value))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let fields = record(heap, fields)?;
                    let kind = atom(heap, "Struct");
                    record(heap, [("kind".into(), kind), ("fields".into(), fields)])
                }
                T::Enum(variants) => {
                    let variants = variants
                        .iter()
                        .map(|(name, payload)| {
                            let value = match payload {
                                Some(payload) => build(heap, background, payload, declared)?,
                                None => Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
                            };
                            Ok((name.clone(), value))
                        })
                        .collect::<Result<Vec<_>, HeapError>>()?;
                    let variants = record(heap, variants)?;
                    let kind = atom(heap, "Enum");
                    record(heap, [("kind".into(), kind), ("variants".into(), variants)])
                }
                T::Function { parameters, result } => {
                    let parameters = parameters
                        .iter()
                        .map(|item| build(heap, background, item, declared))
                        .collect::<Result<Vec<_>, _>>()?;
                    let parameters = Val::unknown(DecodedValue::Array(
                        heap.allocate(Object::Array(parameters.into_boxed_slice())),
                    ));
                    let result = build(heap, background, result, declared)?;
                    let kind = atom(heap, "Func");
                    record(
                        heap,
                        [
                            ("kind".into(), kind),
                            ("parameters".into(), parameters),
                            ("result".into(), result),
                        ],
                    )
                }
            }
        }

        build(self, background, descriptor, &mut HashMap::new())
    }

    pub(crate) fn int(&self, value: i64) -> Val {
        Val::unknown(DecodedValue::Int(value))
    }

    pub(crate) fn native_closure(
        &mut self,
        function: NativeFunction,
        upvalues: impl Into<Box<[Val]>>,
    ) -> Val {
        let handle = self.allocate(Object::Closure {
            identity: Arc::new(()),
            prototype: RuntimePrototype::Native(function),
            upvalues: upvalues.into(),
        });
        Val::unknown(DecodedValue::Func(handle))
    }

    pub(crate) fn native_type_value(&mut self, value: crate::NativeType) -> Val {
        let id = self.intern_native_type(value);
        Val::unknown(DecodedValue::NativeType(id))
    }

    pub(crate) fn persistent(&self, value: Val) -> Result<PersistentValue, HeapError> {
        if self.storage != Storage::Main {
            return Err(HeapError("persistent value must belong to the Main world"));
        }
        Ok(PersistentValue(value))
    }

    pub(crate) fn declare_type(
        &mut self,
        body: Val,
        module: crate::ModuleId,
        declaration: u32,
        name: impl Into<Arc<str>>,
    ) -> Result<Val, HeapError> {
        let id = crate::value::DeclaredTypeId::concrete(module, declaration);
        let type_id = self.canonical_declared_type_id(&id)?;
        let handle = self.allocate_declared_type(Object::DeclaredType {
            type_id,
            id,
            name: name.into(),
            body,
            sealed: true,
            application_arguments: None,
        });
        Ok(Val::unknown(DecodedValue::DeclaredType(handle)))
    }

    pub(crate) fn reserve_type_ref(
        &mut self,
        module: crate::ModuleId,
        declaration: u32,
        name: impl Into<Arc<str>>,
        placeholder: Val,
    ) -> Result<Val, HeapError> {
        let id = crate::value::DeclaredTypeId::concrete(module, declaration);
        self.reserve_declared_type(id, name, placeholder)
    }

    fn reserve_declared_type(
        &mut self,
        id: crate::value::DeclaredTypeId,
        name: impl Into<Arc<str>>,
        placeholder: Val,
    ) -> Result<Val, HeapError> {
        let type_id = self.canonical_declared_type_id(&id)?;
        let handle = self.allocate_declared_type(Object::DeclaredType {
            type_id,
            id,
            name: name.into(),
            body: placeholder,
            sealed: false,
            application_arguments: None,
        });
        Ok(Val::unknown(DecodedValue::DeclaredType(handle)))
    }

    fn reserve_type_metadata(
        &mut self,
        id: crate::value::DeclaredTypeId,
        name: impl Into<Arc<str>>,
        placeholder: Val,
    ) -> Result<Val, HeapError> {
        match self.canonical_declared_type_id(&id) {
            Ok(_) => self.reserve_declared_type(id, name, placeholder),
            Err(_)
                if id
                    .arguments()
                    .iter()
                    .any(crate::types::type_identity_is_symbolic) =>
            {
                let handle = self.allocate(Object::SymbolicType {
                    id,
                    name: name.into(),
                    body: placeholder,
                    sealed: false,
                    application_arguments: None,
                });
                Ok(Val::unknown(DecodedValue::SymbolicType(handle)))
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn reserve_symbolic_type_ref(
        &mut self,
        id: crate::value::DeclaredTypeId,
        name: impl Into<Arc<str>>,
        placeholder: Val,
    ) -> Result<Val, HeapError> {
        if !id
            .arguments()
            .iter()
            .any(crate::types::type_identity_is_symbolic)
        {
            return Err(HeapError(
                "symbolic type ref requires at least one symbolic argument",
            ));
        }
        self.reserve_type_metadata(id, name, placeholder)
    }

    pub(crate) fn seal_type_ref(&mut self, target: Val, body: Val) -> Result<Val, HeapError> {
        let handle = match target.value() {
            DecodedValue::DeclaredType(handle) | DecodedValue::SymbolicType(handle) => handle,
            _ => return Err(HeapError("type ref target is not declared type metadata")),
        };
        if handle.storage != Storage::Work {
            return Err(HeapError(
                "type refs can only be sealed in their Work world",
            ));
        }
        let (slot, sealed) = match self.object_mut(handle)? {
            Object::DeclaredType { body, sealed, .. }
            | Object::SymbolicType { body, sealed, .. } => (body, sealed),
            _ => return Err(HeapError("type ref target is not declared type metadata")),
        };
        if *sealed {
            return Err(HeapError("type ref is already sealed"));
        }
        *slot = body;
        *sealed = true;
        Ok(target)
    }
}

impl PersistentValue {
    pub(crate) const fn runtime(self) -> Val {
        self.0
    }

    pub(crate) fn without_location(self) -> Self {
        Self(self.0.with_loc(None))
    }
}
