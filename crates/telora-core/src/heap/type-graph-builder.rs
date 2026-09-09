// Runtime metadata is a consumer of the solved graph. This builder intentionally
// has no source-origin callback: origin-bearing projections use their own path.
pub(crate) enum TypeMetadataRoot<'a> {
    Graph(crate::types::AnalysisTypeId),
    Descriptor(&'a crate::types::TypeDescriptor),
}

impl Heap {
    #[cfg(test)]
    pub(crate) fn type_graph_value(
        &mut self,
        background: Option<&Heap>,
        graph: &crate::types::TypeGraph,
        root: crate::types::AnalysisTypeId,
    ) -> Result<Val, HeapError> {
        self.type_graph_values(background, graph, [TypeMetadataRoot::Graph(root)])
            .map(|mut values| values.pop().expect("one root"))
    }

    pub(crate) fn type_graph_values<'a>(
        &mut self,
        background: Option<&Heap>,
        graph: &crate::types::TypeGraph,
        roots: impl IntoIterator<Item = TypeMetadataRoot<'a>>,
    ) -> Result<Vec<Val>, HeapError> {
        struct Builder<'a> {
            graph: &'a crate::types::TypeGraph,
            values: Vec<Option<Val>>,
            active: Vec<Option<usize>>,
            depth: usize,
            nominal_boundary: usize,
            declared: HashMap<crate::value::DeclaredTypeId, Val>,
        }
        impl Builder<'_> {
            fn build(
                &mut self,
                heap: &mut Heap,
                background: Option<&Heap>,
                root: crate::types::AnalysisTypeId,
            ) -> Result<Val, HeapError> {
                use crate::types::TypeNode as T;
                if let Some(value) = self.values[root.index()] {
                    return Ok(value);
                }
                let previous = self.active[root.index()];
                if previous.is_some_and(|depth| depth >= self.nominal_boundary) {
                    return Err(HeapError(
                        "recursive structural type has no nominal identity",
                    ));
                }
                self.active[root.index()] = Some(self.depth);
                self.depth += 1;
                let atom = |heap: &mut Heap, text: &str| Val::unknown(heap.atom(background, text));
                let kind = |heap: &mut Heap, text: &str| {
                    let value = atom(heap, text);
                    heap.record_value([("kind".into(), value)])
                };
                let node = self.graph.node(root);
                let value = match node {
                    T::Pending => {
                        return Err(HeapError(
                            "non-concrete type metadata cannot enter the runtime",
                        ));
                    }
                    T::PendingAlternatives(_) => return Err(HeapError("unresolved common type")),
                    T::Ref(target) => self.build(heap, background, *target)?,
                    T::Bound(parameter) => {
                        let tag = atom(heap, "Bound");
                        heap.record_value([
                            ("kind".into(), tag),
                            (
                                "parameter".into(),
                                Val::unknown(DecodedValue::Int(i64::from(parameter.index()))),
                            ),
                        ])?
                    }
                    T::Named(name) => {
                        let tag = atom(heap, "Named");
                        let name = Val::unknown(heap.string(background, name));
                        heap.record_value([("kind".into(), tag), ("name".into(), name)])?
                    }
                    T::Declared { id, name, body } => {
                        if let Some(existing) = self.declared.get(id) {
                            *existing
                        } else {
                            let existing =
                                heap.canonical_declared_type_id(id)
                                    .ok()
                                    .and_then(|type_id| {
                                        heap.declared_types
                                            .get(&type_id)
                                            .or_else(|| background?.declared_types.get(&type_id))
                                            .copied()
                                    });
                            if let Some(existing) = existing
                                && let DecodedValue::DeclaredType(handle) = existing.value()
                                && matches!(
                                    (HeapView {
                                        current: heap,
                                        background
                                    })
                                    .object(handle)?,
                                    Object::DeclaredType { sealed: true, .. }
                                )
                            {
                                existing
                            } else {
                                let tag = atom(heap, "Named");
                                let text = Val::unknown(heap.string(background, name));
                                let placeholder = heap
                                    .record_value([("kind".into(), tag), ("name".into(), text)])?;
                                let owner = heap.reserve_type_metadata(
                                    id.clone(),
                                    name.as_str(),
                                    placeholder,
                                )?;
                                // Reserve before following the body: every recursive edge
                                // observes the same owner, including duplicate identity rows.
                                self.declared.insert(id.clone(), owner);
                                self.values[root.index()] = Some(owner);
                                let boundary =
                                    std::mem::replace(&mut self.nominal_boundary, self.depth);
                                let body = self.build(heap, background, *body)?;
                                self.nominal_boundary = boundary;
                                heap.seal_type_ref(owner, body)?
                            }
                        }
                    }
                    T::Never => kind(heap, "Never")?,
                    T::Type => kind(heap, "Type")?,
                    T::Dyn => kind(heap, "Dyn")?,
                    T::Int => kind(heap, "Int")?,
                    T::Float => kind(heap, "Float")?,
                    T::String => kind(heap, "String")?,
                    T::Bytes => kind(heap, "Bytes")?,
                    T::AtomValue => kind(heap, "Atom")?,
                    T::Opaque(native) => heap.native_type_value(native.clone()),
                    T::Atom(tag) => {
                        let kind = atom(heap, "Atom");
                        let tag = atom(heap, tag.name());
                        heap.record_value([("kind".into(), kind), ("tag".into(), tag)])?
                    }
                    T::TypeOf(child) | T::Array(child) | T::Dict(child) | T::Newtype(child) => {
                        let child = self.build(heap, background, *child)?;
                        let (name, field) = match node {
                            T::TypeOf(_) => ("TypeOf", "instance"),
                            T::Array(_) => ("Array", "item"),
                            T::Dict(_) => ("Dict", "item"),
                            _ => ("Newtype", "payload"),
                        };
                        let tag = atom(heap, name);
                        heap.record_value([("kind".into(), tag), (field.into(), child)])?
                    }
                    T::Tagged { tag, payload } => {
                        let payload = self.build(heap, background, *payload)?;
                        let kind = atom(heap, "Tagged");
                        let tag = atom(heap, tag.name());
                        heap.record_value([
                            ("kind".into(), kind),
                            ("tag".into(), tag),
                            ("payload".into(), payload),
                        ])?
                    }
                    T::Tuple(children) => {
                        let children = children
                            .iter()
                            .map(|id| self.build(heap, background, *id))
                            .collect::<Result<Vec<_>, _>>()?;
                        let items = Val::unknown(DecodedValue::Array(
                            heap.allocate(Object::Array(children.into_boxed_slice())),
                        ));
                        let tag = atom(heap, "Tuple");
                        heap.record_value([("kind".into(), tag), ("items".into(), items)])?
                    }
                    T::Struct(fields) => {
                        let fields = fields
                            .iter()
                            .map(|(name, id)| {
                                self.build(heap, background, *id)
                                    .map(|value| (name.clone(), value))
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        let fields = heap.record_value(fields)?;
                        let tag = atom(heap, "Struct");
                        heap.record_value([("kind".into(), tag), ("fields".into(), fields)])?
                    }
                    T::Enum(variants) => {
                        let variants = variants
                            .iter()
                            .map(|(name, id)| {
                                let value = match id {
                                    Some(id) => self.build(heap, background, *id)?,
                                    None => {
                                        Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None))
                                    }
                                };
                                Ok((name.clone(), value))
                            })
                            .collect::<Result<Vec<_>, HeapError>>()?;
                        let variants = heap.record_value(variants)?;
                        let tag = atom(heap, "Enum");
                        heap.record_value([("kind".into(), tag), ("variants".into(), variants)])?
                    }
                    T::Function { parameters, result } => {
                        let parameters = parameters
                            .iter()
                            .map(|id| self.build(heap, background, *id))
                            .collect::<Result<Vec<_>, _>>()?;
                        let parameters = Val::unknown(DecodedValue::Array(
                            heap.allocate(Object::Array(parameters.into_boxed_slice())),
                        ));
                        let result = self.build(heap, background, *result)?;
                        let tag = atom(heap, "Func");
                        heap.record_value([
                            ("kind".into(), tag),
                            ("parameters".into(), parameters),
                            ("result".into(), result),
                        ])?
                    }
                };
                self.depth -= 1;
                self.active[root.index()] = previous;
                self.values[root.index()] = Some(value);
                Ok(value)
            }
        }
        // The conversion table cannot escape this heap/graph operation. Reuse it
        // across roots, but allocate nothing for an empty or descriptor-only batch.
        let mut builder = None;
        roots
            .into_iter()
            .map(|root| match root {
                TypeMetadataRoot::Graph(root) => builder
                    .get_or_insert_with(|| Builder {
                        graph,
                        values: vec![None; graph.nodes().len()],
                        active: vec![None; graph.nodes().len()],
                        depth: 0,
                        nominal_boundary: 0,
                        declared: HashMap::new(),
                    })
                    .build(self, background, root),
                TypeMetadataRoot::Descriptor(descriptor) => {
                    self.type_descriptor_value(background, descriptor)
                }
            })
            .collect()
    }
}
