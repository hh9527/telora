#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AnalysisTypeId(u32);

fn display_named_type(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

impl AnalysisTypeId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TypeNode {
    Pending,
    Ref(AnalysisTypeId),
    Bound(TypeParameterId),
    Named(String),
    Declared {
        id: crate::value::DeclaredTypeId,
        name: String,
        body: AnalysisTypeId,
    },
    Any,
    Never,
    Type,
    Dyn,
    TypeOf(AnalysisTypeId),
    Int,
    Float,
    String,
    Bytes,
    AtomValue,
    Opaque(crate::NativeType),
    Atom(Atom),
    Array(AnalysisTypeId),
    Dict(AnalysisTypeId),
    Tagged {
        tag: Atom,
        payload: AnalysisTypeId,
    },
    Tuple(Vec<AnalysisTypeId>),
    Struct(BTreeMap<String, AnalysisTypeId>),
    Enum(BTreeMap<String, Option<AnalysisTypeId>>),
    Union(Vec<AnalysisTypeId>),
    Function {
        parameters: Vec<AnalysisTypeId>,
        result: AnalysisTypeId,
    },
}

#[derive(Clone, Default)]
pub struct TypeGraph {
    nodes: Vec<TypeNode>,
    names: BTreeMap<String, AnalysisTypeId>,
    declared: HashMap<crate::value::DeclaredTypeId, AnalysisTypeId>,
    interned: RawTable<AnalysisTypeId>,
    interner_hasher: std::collections::hash_map::RandomState,
}

impl std::fmt::Debug for TypeGraph {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TypeGraph")
            .field("nodes", &self.nodes)
            .field("names", &self.names)
            .finish_non_exhaustive()
    }
}

fn type_nodes_canonically_equal(left: &TypeNode, right: &TypeNode) -> bool {
    match (left, right) {
        (TypeNode::Declared { id: left, .. }, TypeNode::Declared { id: right, .. }) => {
            left == right
        }
        _ => left == right,
    }
}

fn type_node_hash(hash_builder: &std::collections::hash_map::RandomState, node: &TypeNode) -> u64 {
    let mut state = hash_builder.build_hasher();
    std::mem::discriminant(node).hash(&mut state);
    match node {
        TypeNode::Declared { id, .. } => id.hash(&mut state),
        _ => node.hash(&mut state),
    }
    state.finish()
}

impl TypeGraph {
    pub fn node(&self, id: AnalysisTypeId) -> &TypeNode {
        &self.nodes[id.index()]
    }

    pub fn named(&self, name: &str) -> Option<AnalysisTypeId> {
        self.names.get(name).copied()
    }

    pub fn names(&self) -> impl Iterator<Item = (&str, AnalysisTypeId)> {
        self.names.iter().map(|(name, id)| (name.as_str(), *id))
    }

    pub fn nodes(&self) -> impl ExactSizeIterator<Item = (AnalysisTypeId, &TypeNode)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (AnalysisTypeId(index as u32), node))
    }

    pub(crate) fn from_module_interface(interface: &ModuleInterface) -> (Self, AnalysisTypeId) {
        let mut graph = Self::default();
        for (name, descriptor) in &interface.concrete_types {
            let ty = graph.intern_descriptor(descriptor);
            graph.names.insert(name.clone(), ty);
        }
        let fields = interface
            .exports
            .iter()
            .map(|(name, scheme)| (name.clone(), graph.intern_descriptor(&scheme.body)))
            .collect();
        let result = graph.intern_node(TypeNode::Struct(fields));
        (graph, result)
    }

    pub fn display(&self, id: AnalysisTypeId) -> String {
        self.display_with(id, &mut HashSet::new())
    }

    pub fn is_assignable(&self, actual: AnalysisTypeId, expected: AnalysisTypeId) -> bool {
        self.assignable_with(actual, expected, &mut HashSet::new())
    }

    fn root_model_kind(&self, root: AnalysisTypeId) -> Option<crate::ast::DeclaredInitializerKind> {
        let mut current = root;
        let mut visited = HashSet::new();
        while visited.insert(current) {
            match self.node(current) {
                TypeNode::Ref(target) => current = *target,
                TypeNode::Declared { body, .. } => current = *body,
                TypeNode::Struct(_) => {
                    return Some(crate::ast::DeclaredInitializerKind::Struct);
                }
                TypeNode::Enum(_) => return Some(crate::ast::DeclaredInitializerKind::Enum),
                _ => return None,
            }
        }
        None
    }

    fn canonicalize(&self, root: AnalysisTypeId, store: &mut TypeStore) -> Result<TypeId, String> {
        fn visit(
            graph: &TypeGraph,
            id: AnalysisTypeId,
            store: &mut TypeStore,
            canonical: &mut HashMap<AnalysisTypeId, TypeId>,
            visiting: &mut HashSet<AnalysisTypeId>,
        ) -> Result<TypeId, String> {
            if let Some(id) = canonical.get(&id) {
                return Ok(*id);
            }
            if !visiting.insert(id) {
                return Err(format!(
                    "recursive structural type at graph node {id:?} has no canonical nominal ID"
                ));
            }
            let result = match graph.node(id) {
                TypeNode::Pending => Err("type graph contains an open node".into()),
                TypeNode::Ref(target) => visit(graph, *target, store, canonical, visiting),
                TypeNode::Bound(_) => Err("cannot canonicalize an unbound type parameter".into()),
                TypeNode::Named(name) => Err(format!(
                    "cannot canonicalize unresolved named type {name:?}"
                )),
                TypeNode::Declared {
                    id: declared,
                    name,
                    body,
                } => {
                    let arguments = declared
                        .arguments()
                        .iter()
                        .map(|argument| store.intern_descriptor(argument))
                        .collect::<Result<Vec<_>, _>>()?;
                    let type_id = match store.begin(declared.constructor(), arguments) {
                        InternType::Existing(id) | InternType::Reserved(id) => id,
                    };
                    canonical.insert(id, type_id);
                    if store.is_pending(type_id) {
                        let shape = nominal_shape(graph, *body, store, canonical, visiting)?;
                        store.seal_shape(type_id, name.clone(), shape)?;
                    }
                    Ok(type_id)
                }
                TypeNode::Any => Ok(TypeId::ANY),
                TypeNode::Never => Ok(TypeId::NEVER),
                TypeNode::Type => Ok(TypeId::TYPE),
                TypeNode::Dyn => Ok(TypeId::DYN),
                TypeNode::Int => Ok(TypeId::INT),
                TypeNode::Float => Ok(TypeId::FLOAT),
                TypeNode::String => Ok(TypeId::STRING),
                TypeNode::Bytes => Ok(TypeId::BYTES),
                TypeNode::AtomValue => Ok(TypeId::ATOM),
                TypeNode::TypeOf(inner) => visit(graph, *inner, store, canonical, visiting)
                    .map(|inner| store.intern_structural(TypeShape::TypeOf(inner))),
                TypeNode::Opaque(native) => {
                    Ok(store
                        .intern_structural(TypeShape::Opaque(native.qualified_name().to_owned())))
                }
                TypeNode::Atom(atom) => {
                    Ok(store.intern_structural(TypeShape::Atom(atom.name().to_owned())))
                }
                TypeNode::Array(item) => visit(graph, *item, store, canonical, visiting)
                    .map(|item| store.intern_structural(TypeShape::Array(item))),
                TypeNode::Dict(item) => visit(graph, *item, store, canonical, visiting)
                    .map(|item| store.intern_structural(TypeShape::Dict(item))),
                TypeNode::Tagged { tag, payload } => {
                    let payload = visit(graph, *payload, store, canonical, visiting)?;
                    Ok(store.intern_structural(TypeShape::Tagged {
                        tag: tag.name().to_owned(),
                        payload,
                    }))
                }
                TypeNode::Tuple(items) => {
                    let items = items
                        .iter()
                        .map(|item| visit(graph, *item, store, canonical, visiting))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(store.intern_structural(TypeShape::Tuple(items.into())))
                }
                TypeNode::Struct(fields) => {
                    let fields = fields
                        .iter()
                        .map(|(name, field)| {
                            visit(graph, *field, store, canonical, visiting)
                                .map(|field| (name.clone(), field))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(store.intern_structural(TypeShape::Struct(fields.into())))
                }
                TypeNode::Enum(variants) => {
                    let variants = variants
                        .iter()
                        .map(|(name, payload)| {
                            payload
                                .map(|payload| visit(graph, payload, store, canonical, visiting))
                                .transpose()
                                .map(|payload| (name.clone(), payload))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(store.intern_structural(TypeShape::Enum(variants.into())))
                }
                TypeNode::Union(variants) => {
                    let variants = variants
                        .iter()
                        .map(|variant| visit(graph, *variant, store, canonical, visiting))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(store.intern_structural(TypeShape::Union(variants.into())))
                }
                TypeNode::Function { parameters, result } => {
                    let parameters = parameters
                        .iter()
                        .map(|parameter| visit(graph, *parameter, store, canonical, visiting))
                        .collect::<Result<Vec<_>, _>>()?;
                    let result = visit(graph, *result, store, canonical, visiting)?;
                    Ok(store.intern_structural(TypeShape::Function {
                        parameters: parameters.into(),
                        result,
                    }))
                }
            };
            visiting.remove(&id);
            let result = result?;
            canonical.insert(id, result);
            Ok(result)
        }

        fn nominal_shape(
            graph: &TypeGraph,
            root: AnalysisTypeId,
            store: &mut TypeStore,
            canonical: &mut HashMap<AnalysisTypeId, TypeId>,
            visiting: &mut HashSet<AnalysisTypeId>,
        ) -> Result<TypeShape, String> {
            match graph.node(root) {
                TypeNode::Ref(target) => nominal_shape(graph, *target, store, canonical, visiting),
                TypeNode::Struct(fields) => fields
                    .iter()
                    .map(|(name, field)| {
                        visit(graph, *field, store, canonical, visiting)
                            .map(|field| (name.clone(), field))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|fields| TypeShape::Struct(fields.into())),
                TypeNode::Enum(variants) => variants
                    .iter()
                    .map(|(name, payload)| {
                        payload
                            .map(|payload| visit(graph, payload, store, canonical, visiting))
                            .transpose()
                            .map(|payload| (name.clone(), payload))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|variants| TypeShape::Enum(variants.into())),
                _ => Err("nominal type body must be a struct or enum".into()),
            }
        }

        visit(self, root, store, &mut HashMap::new(), &mut HashSet::new())
    }

    fn push(&mut self, node: TypeNode) -> AnalysisTypeId {
        let id = AnalysisTypeId(u32::try_from(self.nodes.len()).expect("type graph exceeds u32"));
        self.nodes.push(node);
        id
    }

    fn intern_node(&mut self, node: TypeNode) -> AnalysisTypeId {
        let hash = type_node_hash(&self.interner_hasher, &node);
        if let Some(id) = self.interned.get(hash, |id| {
            type_nodes_canonically_equal(&self.nodes[id.index()], &node)
        }) {
            return *id;
        }
        let id = self.push(node);
        let nodes = &self.nodes;
        let hasher = &self.interner_hasher;
        self.interned
            .insert(hash, id, |id| type_node_hash(hasher, &nodes[id.index()]));
        id
    }

    fn interned_node(&self, node: &TypeNode) -> Option<AnalysisTypeId> {
        let hash = type_node_hash(&self.interner_hasher, node);
        self.interned
            .get(hash, |id| {
                type_nodes_canonically_equal(&self.nodes[id.index()], node)
            })
            .copied()
    }

    fn record_interned_node(&mut self, id: AnalysisTypeId) {
        let hash = type_node_hash(&self.interner_hasher, &self.nodes[id.index()]);
        let nodes = &self.nodes;
        let hasher = &self.interner_hasher;
        self.interned
            .insert(hash, id, |id| type_node_hash(hasher, &nodes[id.index()]));
    }

    fn finish_reserved_node(&mut self, reserved: AnalysisTypeId, node: TypeNode) -> AnalysisTypeId {
        self.nodes[reserved.index()] = node;
        reserved
    }

    fn intern_descriptor(&mut self, descriptor: &TypeDescriptor) -> AnalysisTypeId {
        if let TypeDescriptor::Declared(declared) = descriptor {
            if let Some(id) = self.declared.get(&declared.id) {
                return *id;
            }
            let id = self.push(TypeNode::Pending);
            self.declared.insert(declared.id.clone(), id);
            let body = self.intern_descriptor(&declared.body);
            self.nodes[id.index()] = TypeNode::Declared {
                id: declared.id.clone(),
                name: declared.name.clone(),
                body,
            };
            self.record_interned_node(id);
            return id;
        }
        let node = match descriptor {
            TypeDescriptor::Bound(parameter) => TypeNode::Bound(*parameter),
            TypeDescriptor::Named(name) => self
                .names
                .get(name)
                .copied()
                .map_or_else(|| TypeNode::Named(name.clone()), TypeNode::Ref),
            TypeDescriptor::Declared(_) => unreachable!("declared descriptors return above"),
            TypeDescriptor::Inference(_) => {
                unreachable!("solver descriptors must be explicitly erased before interning")
            }
            TypeDescriptor::Any => TypeNode::Any,
            TypeDescriptor::Never => TypeNode::Never,
            TypeDescriptor::Type => TypeNode::Type,
            TypeDescriptor::Dyn => TypeNode::Dyn,
            TypeDescriptor::TypeOf(instance) => TypeNode::TypeOf(self.intern_descriptor(instance)),
            TypeDescriptor::Int => TypeNode::Int,
            TypeDescriptor::Float => TypeNode::Float,
            TypeDescriptor::String => TypeNode::String,
            TypeDescriptor::Bytes => TypeNode::Bytes,
            TypeDescriptor::AtomValue => TypeNode::AtomValue,
            TypeDescriptor::Opaque(native_type) => TypeNode::Opaque(native_type.clone()),
            TypeDescriptor::Atom(atom) => TypeNode::Atom(atom.clone()),
            TypeDescriptor::Array(item) => TypeNode::Array(self.intern_descriptor(item)),
            TypeDescriptor::Dict(item) => TypeNode::Dict(self.intern_descriptor(item)),
            TypeDescriptor::Tagged { tag, payload } => TypeNode::Tagged {
                tag: tag.clone(),
                payload: self.intern_descriptor(payload),
            },
            TypeDescriptor::Tuple(items) => TypeNode::Tuple(
                items
                    .iter()
                    .map(|item| self.intern_descriptor(item))
                    .collect(),
            ),
            TypeDescriptor::Struct(fields) => TypeNode::Struct(
                fields
                    .iter()
                    .map(|(name, item)| (name.clone(), self.intern_descriptor(item)))
                    .collect(),
            ),
            TypeDescriptor::Enum(variants) => TypeNode::Enum(
                variants
                    .iter()
                    .map(|(name, payload)| {
                        (
                            name.clone(),
                            payload.as_deref().map(|item| self.intern_descriptor(item)),
                        )
                    })
                    .collect(),
            ),
            TypeDescriptor::Union(variants) => TypeNode::Union(
                variants
                    .iter()
                    .map(|item| self.intern_descriptor(item))
                    .collect(),
            ),
            TypeDescriptor::Function { parameters, result } => TypeNode::Function {
                parameters: parameters
                    .iter()
                    .map(|item| self.intern_descriptor(item))
                    .collect(),
                result: self.intern_descriptor(result),
            },
        };
        self.intern_node(node)
    }

    fn intern_erased_descriptor(&mut self, descriptor: &TypeDescriptor) -> AnalysisTypeId {
        self.intern_descriptor(&erase_type_variables(descriptor))
    }

    fn descriptor(&self, root: AnalysisTypeId) -> Result<TypeDescriptor, String> {
        fn build(
            graph: &TypeGraph,
            id: AnalysisTypeId,
            visiting: &mut HashSet<AnalysisTypeId>,
        ) -> Result<TypeDescriptor, String> {
            if !visiting.insert(id) {
                return match graph.node(id) {
                    TypeNode::Declared { id, name, .. } => {
                        Ok(TypeDescriptor::Declared(DeclaredTypeDescriptor {
                            id: id.clone(),
                            name: name.clone(),
                            body: Arc::new(TypeDescriptor::Never),
                        }))
                    }
                    _ => Err("recursive structural type has no nominal identity".into()),
                };
            }
            let descriptor = match graph.node(id) {
                TypeNode::Pending => return Err("type graph contains an open node".into()),
                TypeNode::Ref(target) => build(graph, *target, visiting)?,
                TypeNode::Bound(parameter) => TypeDescriptor::Bound(*parameter),
                TypeNode::Named(name) => TypeDescriptor::Named(name.clone()),
                TypeNode::Declared { id, name, body } => {
                    let body = build(graph, *body, visiting)?;
                    TypeDescriptor::Declared(DeclaredTypeDescriptor {
                        id: id.clone(),
                        name: name.clone(),
                        body: Arc::new(body),
                    })
                }
                TypeNode::Any => TypeDescriptor::Any,
                TypeNode::Never => TypeDescriptor::Never,
                TypeNode::Type => TypeDescriptor::Type,
                TypeNode::Dyn => TypeDescriptor::Dyn,
                TypeNode::TypeOf(inner) => {
                    TypeDescriptor::TypeOf(Box::new(build(graph, *inner, visiting)?))
                }
                TypeNode::Int => TypeDescriptor::Int,
                TypeNode::Float => TypeDescriptor::Float,
                TypeNode::String => TypeDescriptor::String,
                TypeNode::Bytes => TypeDescriptor::Bytes,
                TypeNode::AtomValue => TypeDescriptor::AtomValue,
                TypeNode::Opaque(native) => TypeDescriptor::Opaque(native.clone()),
                TypeNode::Atom(atom) => TypeDescriptor::Atom(atom.clone()),
                TypeNode::Array(item) => {
                    TypeDescriptor::Array(Box::new(build(graph, *item, visiting)?))
                }
                TypeNode::Dict(item) => {
                    TypeDescriptor::Dict(Box::new(build(graph, *item, visiting)?))
                }
                TypeNode::Tagged { tag, payload } => TypeDescriptor::Tagged {
                    tag: tag.clone(),
                    payload: Box::new(build(graph, *payload, visiting)?),
                },
                TypeNode::Tuple(items) => TypeDescriptor::Tuple(
                    items
                        .iter()
                        .map(|item| build(graph, *item, visiting))
                        .collect::<Result<_, _>>()?,
                ),
                TypeNode::Struct(fields) => TypeDescriptor::Struct(
                    fields
                        .iter()
                        .map(|(name, item)| Ok((name.clone(), build(graph, *item, visiting)?)))
                        .collect::<Result<_, String>>()?,
                ),
                TypeNode::Enum(variants) => TypeDescriptor::Enum(
                    variants
                        .iter()
                        .map(|(name, payload)| {
                            Ok((
                                name.clone(),
                                payload
                                    .map(|payload| build(graph, payload, visiting))
                                    .transpose()?
                                    .map(Box::new),
                            ))
                        })
                        .collect::<Result<_, String>>()?,
                ),
                TypeNode::Union(items) => TypeDescriptor::Union(
                    items
                        .iter()
                        .map(|item| build(graph, *item, visiting))
                        .collect::<Result<_, _>>()?,
                ),
                TypeNode::Function { parameters, result } => TypeDescriptor::Function {
                    parameters: parameters
                        .iter()
                        .map(|parameter| build(graph, *parameter, visiting))
                        .collect::<Result<_, _>>()?,
                    result: Box::new(build(graph, *result, visiting)?),
                },
            };
            visiting.remove(&id);
            Ok(descriptor)
        }

        build(self, root, &mut HashSet::new())
    }

    fn install_named_descriptors(
        &mut self,
        descriptors: &BTreeMap<String, TypeDescriptor>,
    ) -> BTreeMap<String, AnalysisTypeId> {
        let roots = descriptors
            .keys()
            .map(|name| {
                let id = self.push(TypeNode::Pending);
                self.names.insert(name.clone(), id);
                (name.clone(), id)
            })
            .collect::<BTreeMap<_, _>>();
        for (name, descriptor) in descriptors {
            let body = self.intern_descriptor(descriptor);
            self.nodes[roots[name].index()] = self.nodes[body.index()].clone();
        }
        roots
    }

    fn decode_persistent(
        &mut self,
        value: ValueRef<'_>,
        path: &str,
        links: &mut HashMap<Handle, AnalysisTypeId>,
    ) -> Result<AnalysisTypeId, String> {
        if let Some(handle) = value.hidden_type_slot_handle() {
            if let Some(id) = links.get(&handle) {
                return Ok(*id);
            }
            let id = self.push(TypeNode::Pending);
            links.insert(handle, id);
            let resolved = value.resolve_hidden_type_slot().map_err(|message| {
                format!("{path} contains an uninitialized recursive type link: {message}")
            })?;
            let target = self.decode_persistent(resolved, path, links)?;
            self.nodes[id.index()] = TypeNode::Ref(target);
            links.insert(handle, id);
            return Ok(id);
        }
        if let Some((declared_id, name, _)) = value.declared_type_parts() {
            let identity = TypeNode::Declared {
                id: declared_id.clone(),
                name: name.to_owned(),
                body: AnalysisTypeId(0),
            };
            if let Some(id) = self.interned_node(&identity) {
                return Ok(id);
            }
        }
        if let Some(handle) = value.object_handle() {
            if let Some(id) = links.get(&handle) {
                return Ok(*id);
            }
            let id = self.push(TypeNode::Pending);
            links.insert(handle, id);
            let node = self.decode_persistent_node(value, path, links)?;
            let canonical = self.finish_reserved_node(id, node);
            if matches!(self.nodes[canonical.index()], TypeNode::Declared { .. }) {
                self.record_interned_node(canonical);
            }
            links.insert(handle, canonical);
            return Ok(canonical);
        }
        let node = self.decode_persistent_node(value, path, links)?;
        Ok(self.push(node))
    }

    fn decode_persistent_node(
        &mut self,
        value: ValueRef<'_>,
        path: &str,
        links: &mut HashMap<Handle, AnalysisTypeId>,
    ) -> Result<TypeNode, String> {
        if let Some(native_type) = value.as_native_type() {
            return Ok(TypeNode::Opaque(native_type.clone()));
        }
        if let Some((id, name, body)) = value.declared_type_parts() {
            return Ok(TypeNode::Declared {
                id: id.clone(),
                name: name.to_owned(),
                body: self.decode_persistent(body, path, links)?,
            });
        }
        let fields = value
            .dict_fields()
            .ok_or_else(|| format!("{path} must be a Dict"))?;
        let kind = value
            .dict_get("kind")
            .and_then(ValueRef::as_atom)
            .ok_or_else(|| format!("{path}.kind must be an Atom"))?;
        let require = |expected: &[&str]| {
            fields
                .iter()
                .copied()
                .eq(expected.iter().copied())
                .then_some(())
                .ok_or_else(|| format!("{path} has invalid fields for {kind}"))
        };
        Ok(match kind.as_str() {
            "Bound" => {
                require(&["kind", "parameter"])?;
                let parameter = value
                    .dict_get("parameter")
                    .and_then(ValueRef::as_int)
                    .and_then(|parameter| u32::try_from(parameter).ok())
                    .ok_or_else(|| format!("{path}.parameter must be a non-negative Int"))?;
                TypeNode::Bound(TypeParameterId(parameter))
            }
            "Named" => {
                require(&["kind", "name"])?;
                let name = value
                    .dict_get("name")
                    .and_then(ValueRef::as_str)
                    .ok_or_else(|| format!("{path}.name must be a String"))?;
                TypeNode::Named(name.as_str().to_owned())
            }
            "Any" => {
                require(&["kind"])?;
                TypeNode::Any
            }
            "Never" => {
                require(&["kind"])?;
                TypeNode::Never
            }
            "Type" => {
                require(&["kind"])?;
                TypeNode::Type
            }
            "Dyn" => {
                require(&["kind"])?;
                TypeNode::Dyn
            }
            "TypeOf" => {
                require(&["instance", "kind"])?;
                let instance = self.decode_persistent(
                    value.dict_get("instance").expect("field exists"),
                    &format!("{path}.instance"),
                    links,
                )?;
                TypeNode::TypeOf(instance)
            }
            "Int" => {
                require(&["kind"])?;
                TypeNode::Int
            }
            "Float" => {
                require(&["kind"])?;
                TypeNode::Float
            }
            "String" => {
                require(&["kind"])?;
                TypeNode::String
            }
            "Bytes" => {
                require(&["kind"])?;
                TypeNode::Bytes
            }
            "Atom" => {
                if fields.iter().copied().eq(["kind"]) {
                    return Ok(TypeNode::AtomValue);
                }
                require(&["kind", "tag"])?;
                let tag = value
                    .dict_get("tag")
                    .and_then(ValueRef::as_atom)
                    .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
                TypeNode::Atom(atom_from_name(tag.as_str()))
            }
            "Array" => {
                require(&["item", "kind"])?;
                let item = self.decode_persistent(
                    value.dict_get("item").expect("field exists"),
                    &format!("{path}.item"),
                    links,
                )?;
                TypeNode::Array(item)
            }
            "Dict" => {
                require(&["item", "kind"])?;
                let item = self.decode_persistent(
                    value.dict_get("item").expect("field exists"),
                    &format!("{path}.item"),
                    links,
                )?;
                TypeNode::Dict(item)
            }
            "Tagged" => {
                require(&["kind", "payload", "tag"])?;
                let tag = value
                    .dict_get("tag")
                    .and_then(ValueRef::as_atom)
                    .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
                let payload = self.decode_persistent(
                    value.dict_get("payload").expect("field exists"),
                    &format!("{path}.payload"),
                    links,
                )?;
                TypeNode::Tagged {
                    tag: atom_from_name(tag.as_str()),
                    payload,
                }
            }
            "Tuple" | "Union" => {
                let field = if kind == "Tuple" { "items" } else { "variants" };
                require(if kind == "Tuple" {
                    &["items", "kind"]
                } else {
                    &["kind", "variants"]
                })?;
                let sequence = value.dict_get(field).expect("field exists");
                if sequence.kind() != ValueKind::Array {
                    return Err(format!("{path}.{field} must be an Array"));
                }
                let mut values = Vec::new();
                for index in 0..sequence.sequence_len().expect("Array length") {
                    values.push(self.decode_persistent(
                        sequence.sequence_get(index).expect("Array item"),
                        &format!("{path}.{field}[{index}]"),
                        links,
                    )?);
                }
                if kind == "Union" && values.is_empty() {
                    return Err(format!("{path}.variants must not be empty"));
                }
                if kind == "Tuple" {
                    TypeNode::Tuple(values)
                } else {
                    TypeNode::Union(values)
                }
            }
            "Struct" => {
                require(&["fields", "kind"])?;
                let values = value.dict_get("fields").expect("field exists");
                let names = values
                    .dict_fields()
                    .ok_or_else(|| format!("{path}.fields must be a Dict"))?;
                let mut decoded = BTreeMap::new();
                for name in names {
                    let id = self.decode_persistent(
                        values.dict_get(name).expect("Dict field"),
                        &format!("{path}.fields.{name}"),
                        links,
                    )?;
                    decoded.insert(name.to_owned(), id);
                }
                TypeNode::Struct(decoded)
            }
            "Enum" => {
                require(&["kind", "variants"])?;
                let values = value.dict_get("variants").expect("field exists");
                let names = values
                    .dict_fields()
                    .ok_or_else(|| format!("{path}.variants must be a Dict"))?;
                if names.is_empty() {
                    return Err(format!("{path}.variants must not be empty"));
                }
                let mut decoded = BTreeMap::new();
                for name in names {
                    let variant_path = format!("{path}.variants.{name}");
                    let variant = values.dict_get(name).expect("Dict field");
                    let payload = if variant.as_atom().is_some_and(|atom| atom == "None") {
                        None
                    } else {
                        Some(self.decode_persistent(variant, &variant_path, links)?)
                    };
                    decoded.insert(name.to_owned(), payload);
                }
                TypeNode::Enum(decoded)
            }
            "Func" => {
                require(&["kind", "parameters", "result"])?;
                let values = value.dict_get("parameters").expect("field exists");
                if values.kind() != ValueKind::Array {
                    return Err(format!("{path}.parameters must be an Array"));
                }
                let mut parameters = Vec::new();
                for index in 0..values.sequence_len().expect("Array length") {
                    parameters.push(self.decode_persistent(
                        values.sequence_get(index).expect("Array item"),
                        &format!("{path}.parameters[{index}]"),
                        links,
                    )?);
                }
                let result = self.decode_persistent(
                    value.dict_get("result").expect("field exists"),
                    &format!("{path}.result"),
                    links,
                )?;
                TypeNode::Function { parameters, result }
            }
            _ => return Err(format!("{path}.kind has unknown value '{kind}'")),
        })
    }

    fn display_with(&self, id: AnalysisTypeId, active: &mut HashSet<AnalysisTypeId>) -> String {
        if !active.insert(id) {
            return self
                .names
                .iter()
                .find_map(|(name, candidate)| {
                    (*candidate == id).then(|| display_named_type(name).to_owned())
                })
                .unwrap_or_else(|| "recursive".into());
        }
        let shown = match self.node(id) {
            TypeNode::Pending => "<pending>".into(),
            TypeNode::Ref(target) => self.display_with(*target, active),
            TypeNode::Bound(parameter) => format!("T{}", parameter.0),
            TypeNode::Named(name) => display_named_type(name).to_owned(),
            TypeNode::Declared { name, .. } => name.clone(),
            TypeNode::Any => "Any".into(),
            TypeNode::Never => "Never".into(),
            TypeNode::Type => "Type".into(),
            TypeNode::Dyn => "Dyn".into(),
            TypeNode::TypeOf(instance) => {
                format!("TypeOf({})", self.display_with(*instance, active))
            }
            TypeNode::Int => "Int".into(),
            TypeNode::Float => "Float".into(),
            TypeNode::String => "String".into(),
            TypeNode::Bytes => "Bytes".into(),
            TypeNode::AtomValue => "Atom".into(),
            TypeNode::Opaque(native_type) => {
                format!("opaque({})", native_type.qualified_name())
            }
            TypeNode::Atom(atom) => format!("'{}", atom.name()),
            TypeNode::Array(item) => format!("Array<{}>", self.display_with(*item, active)),
            TypeNode::Dict(item) => format!("Dict<{}>", self.display_with(*item, active)),
            TypeNode::Tagged { tag, payload } => {
                format!("'{}({})", tag.name(), self.display_with(*payload, active))
            }
            TypeNode::Tuple(items) => format!(
                "({})",
                items
                    .iter()
                    .map(|item| self.display_with(*item, active))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeNode::Struct(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(name, item)| format!("{name}: {}", self.display_with(*item, active)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeNode::Enum(variants) => format!(
                "enum {{{}}}",
                variants
                    .iter()
                    .map(|(name, payload)| payload.map_or_else(
                        || name.clone(),
                        |payload| format!("{name}({})", self.display_with(payload, active))
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeNode::Union(variants) => variants
                .iter()
                .map(|item| self.display_with(*item, active))
                .collect::<Vec<_>>()
                .join(" | "),
            TypeNode::Function { parameters, result } => format!(
                "Fn({}) -> {}",
                parameters
                    .iter()
                    .map(|item| self.display_with(*item, active))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.display_with(*result, active)
            ),
        };
        active.remove(&id);
        shown
    }

    fn assignable_with(
        &self,
        actual: AnalysisTypeId,
        expected: AnalysisTypeId,
        visited: &mut HashSet<(AnalysisTypeId, AnalysisTypeId)>,
    ) -> bool {
        if !visited.insert((actual, expected)) {
            return true;
        }
        match (self.node(actual), self.node(expected)) {
            (TypeNode::Ref(actual), _) => self.assignable_with(*actual, expected, visited),
            (_, TypeNode::Ref(expected)) => self.assignable_with(actual, *expected, visited),
            (TypeNode::Bound(actual), TypeNode::Bound(expected)) => actual == expected,
            (TypeNode::Named(actual), TypeNode::Named(expected)) => actual == expected,
            (TypeNode::Declared { id: actual, .. }, TypeNode::Declared { id: expected, .. }) => {
                actual == expected
            }
            (TypeNode::Never, _) => true,
            (TypeNode::Any, _) | (_, TypeNode::Any) => true,
            (TypeNode::TypeOf(_), TypeNode::Type) => true,
            (TypeNode::TypeOf(a), TypeNode::TypeOf(e)) => self.assignable_with(*a, *e, visited),
            (TypeNode::Atom(_), TypeNode::AtomValue) => true,
            (TypeNode::Array(a), TypeNode::Array(e)) => self.assignable_with(*a, *e, visited),
            (TypeNode::Dict(a), TypeNode::Dict(e)) => self.assignable_with(*a, *e, visited),
            (TypeNode::Struct(fields), TypeNode::Dict(expected)) => fields
                .values()
                .all(|actual| self.assignable_with(*actual, *expected, visited)),
            (
                TypeNode::Tagged {
                    tag: a_tag,
                    payload: a,
                },
                TypeNode::Tagged {
                    tag: e_tag,
                    payload: e,
                },
            ) => a_tag == e_tag && self.assignable_with(*a, *e, visited),
            (TypeNode::Tuple(a), TypeNode::Tuple(e)) => {
                a.len() == e.len()
                    && a.iter()
                        .zip(e)
                        .all(|(a, e)| self.assignable_with(*a, *e, visited))
            }
            (TypeNode::Struct(a), TypeNode::Struct(e)) => {
                a.len() == e.len()
                    && e.iter().all(|(name, e)| {
                        a.get(name)
                            .is_some_and(|a| self.assignable_with(*a, *e, visited))
                    })
            }
            (TypeNode::Enum(a), TypeNode::Enum(e)) => {
                a.len() == e.len()
                    && e.iter().all(|(name, e)| {
                        a.get(name).is_some_and(|a| match (a, e) {
                            (None, None) => true,
                            (Some(a), Some(e)) => self.assignable_with(*a, *e, visited),
                            _ => false,
                        })
                    })
            }
            (TypeNode::Union(a), _) => a
                .iter()
                .all(|a| self.assignable_with(*a, expected, visited)),
            (_, TypeNode::Union(e)) => e.iter().any(|e| self.assignable_with(actual, *e, visited)),
            (
                TypeNode::Function {
                    parameters: ap,
                    result: ar,
                },
                TypeNode::Function {
                    parameters: ep,
                    result: er,
                },
            ) => {
                ap.len() == ep.len()
                    && ap
                        .iter()
                        .zip(ep)
                        .all(|(a, e)| self.assignable_with(*a, *e, visited))
                    && self.assignable_with(*ar, *er, visited)
            }
            (a, e) => a == e,
        }
    }
}
