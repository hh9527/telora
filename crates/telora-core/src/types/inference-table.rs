#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct InferenceConstructorId(u32);

#[derive(Clone, Copy, Debug)]
struct InferenceType {
    constructor: InferenceConstructorId,
    arguments_start: u32,
    arguments_len: u32,
}

#[cfg(test)]
mod inference_table_tests {
    use super::*;

    #[test]
    fn type_rows_and_argument_edges_are_pod() {
        assert_eq!(std::mem::size_of::<InferenceType>(), 12);
        assert!(!std::mem::needs_drop::<InferenceType>());
        assert_eq!(std::mem::size_of::<InferenceVariableId>(), 4);
    }

    #[test]
    fn leaf_rows_are_shared_but_slots_remain_independent() {
        let mut arena = InferenceVariables::default();
        let left = arena.structure_edge(TypeDescriptor::Int);
        let right = arena.structure_edge(TypeDescriptor::Int);
        assert_ne!(left, right);
        assert_eq!(arena.known(left), arena.known(right));
        assert_eq!(arena.types.len(), 1);
        arena.set(left, TypeDescriptor::String);
        assert_eq!(arena.binding(left), Some(&TypeDescriptor::String));
        assert_eq!(arena.binding(right), Some(&TypeDescriptor::Int));
    }

    #[test]
    fn direct_constructor_nodes_share_edges_and_propagate_late_conflicts() {
        let mut arena = InferenceVariables::default();
        let item = arena.fresh();
        let left = arena.structure_node(InferenceConstructor::Array, &[item]);
        let right = arena.structure_node(InferenceConstructor::Array, &[item]);
        let tuple = arena.structure_node(InferenceConstructor::Tuple, &[left, right]);
        assert_eq!(arena.arguments(arena.known(tuple).unwrap()), &[left, right]);
        arena.set(item, TypeDescriptor::Int);
        assert!(arena.same_slots(left, right));
        assert_eq!(arena.canonicalize_slots(), 1);
        assert_eq!(arena.root(left), arena.root(right));
        arena.record_conflict(&TypeDescriptor::Inference(item), "late element conflict");
        assert!(arena.ensure_consistent(&TypeDescriptor::Inference(tuple)).is_err());
        assert!(arena.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn direct_instantiation_shares_parameters_but_isolates_calls() {
        let mut arena = InferenceVariables::default();
        let parameter = TypeParameterId(7);
        let item = TypeDescriptor::Array(Box::new(TypeDescriptor::Bound(parameter)));
        let template = TypeDescriptor::Function {
            parameters: vec![item.clone()], result: Box::new(item),
        };
        let first = arena.fresh();
        let second = arena.fresh();
        let left = arena.instantiate_descriptor(&template, &HashMap::from([(parameter, first)]));
        let right = arena.instantiate_descriptor(&template, &HashMap::from([(parameter, second)]));
        for (function, item) in [(left, first), (right, second)] {
            let children = arena.arguments(arena.known(function).unwrap());
            assert_eq!(children.len(), 2);
            for child in children {
                assert_eq!(arena.arguments(arena.known(*child).unwrap()), &[item]);
            }
        }
        assert!(!arena.same_slots(left, right));
        arena.set(first, TypeDescriptor::Int);
        assert!(arena.known(second).is_none());
        arena.set(second, TypeDescriptor::String);
        assert!(!arena.same_slots(left, right));
        assert!(arena.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn concrete_nominal_bodies_are_shared_by_identity_not_arc_address() {
        let mut arena = InferenceVariables::default();
        let make = |item| TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 93),
            name: "Items".into(), body: Arc::new(TypeDescriptor::Array(Box::new(item))),
        });
        let left = arena.structure_edge(make(TypeDescriptor::Int));
        let right = arena.structure_edge(make(TypeDescriptor::Int));
        assert_eq!(arena.arguments(arena.known(left).unwrap()), arena.arguments(arena.known(right).unwrap()));
        assert_eq!(arena.imported_bodies.len(), 1);
        assert_eq!(arena.resolved_declared_bodies.len(), 1);
        let unknown = arena.fresh();
        let pending = arena.structure_edge(make(TypeDescriptor::Inference(unknown)));
        assert_ne!(arena.arguments(arena.known(left).unwrap()), arena.arguments(arena.known(pending).unwrap()));
        assert_eq!(arena.imported_bodies.len(), 2);
        arena.structure_edge(make(TypeDescriptor::Bound(TypeParameterId(0))));
        assert_eq!(arena.imported_bodies.len(), 3);
        assert_eq!(arena.resolved_declared_bodies.len(), 1);
        assert!(arena.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn nominal_body_cache_preserves_recursive_view_completeness() {
        let mut arena = InferenceVariables::default();
        let make = |body| TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 95),
            name: "Outer".into(),
            body: Arc::new(TypeDescriptor::Array(Box::new(TypeDescriptor::Declared(DeclaredTypeDescriptor {
                id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 96),
                name: "Inner".into(), body: Arc::new(body),
            })))),
        });
        let partial = arena.structure_edge(make(TypeDescriptor::Never));
        let complete = arena.structure_edge(make(TypeDescriptor::Struct(BTreeMap::from([
            ("value".into(), TypeDescriptor::Int),
        ]))));
        let repeated = arena.structure_edge(make(TypeDescriptor::Struct(BTreeMap::from([
            ("value".into(), TypeDescriptor::Int),
        ]))));
        assert_eq!(arena.arguments(arena.known(complete).unwrap()), arena.arguments(arena.known(repeated).unwrap()));
        let inner_body = |slot| {
            let array = arena.arguments(arena.known(slot).unwrap())[0];
            let inner = arena.arguments(arena.known(array).unwrap())[0];
            arena.arguments(arena.known(inner).unwrap())[0]
        };
        assert!(matches!(arena.constructor(arena.known(inner_body(partial)).unwrap()), InferenceConstructor::Never));
        assert!(matches!(arena.constructor(arena.known(inner_body(complete)).unwrap()), InferenceConstructor::Struct(_)));
    }

    #[test]
    fn normalization_cache_is_allocated_only_for_used_bodies() {
        let mut arena = InferenceVariables::default();
        let item = arena.fresh();
        for _ in 0..1024 {
            arena.structure_node(InferenceConstructor::Array, &[item]);
        }
        assert_eq!(arena.normalized_body_indices.len(), 1024);
        assert!(arena.normalized_bodies.borrow().is_empty());
        let id = InferenceTypeId(0);
        arena.cache_normalized_body(id, Arc::new(TypeDescriptor::Int));
        assert_eq!(arena.normalized_bodies.borrow().len(), 1);
        assert_eq!(arena.normalized_body(id).as_deref(), Some(&TypeDescriptor::Int));
        let revision = arena.revision;
        arena.structure_edge(TypeDescriptor::Tuple(vec![TypeDescriptor::Int]));
        arena.structure_node(InferenceConstructor::Array, &[item]);
        assert_eq!(arena.revision, revision);
        assert!(arena.normalized_body(id).is_some());
        arena.set(item, TypeDescriptor::String);
        assert!(arena.normalized_body(id).is_none());
        arena.cache_normalized_body(id, Arc::new(TypeDescriptor::String));
        assert_eq!(arena.normalized_bodies.borrow().len(), 1);
        assert_eq!(arena.normalized_body(id).as_deref(), Some(&TypeDescriptor::String));
    }

    #[test]
    fn graph_operations_do_not_materialize_descriptor_views() {
        let mut arena = InferenceVariables::default();
        let item = arena.fresh();
        let left = arena.structure_edge(TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(item))));
        let right = arena.structure_edge(TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(item))));
        assert!(arena.same_slots(left, right));
        assert_eq!(arena.canonicalize_slots(), 1);
        assert_eq!(arena.root(left), arena.root(right));
        arena.record_conflict(&TypeDescriptor::Inference(item), "element conflict");
        assert!(arena.ensure_consistent(&TypeDescriptor::Inference(left)).is_err());
        assert!(arena.ensure_consistent(&TypeDescriptor::Inference(right)).is_err());
        assert!(arena.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn canonicalization_handles_deep_forward_edges_without_repeated_scans() {
        let mut arena = InferenceVariables::default();
        let left = (0..16384).map(|_| arena.fresh()).collect::<Vec<_>>();
        let right = (0..16384).map(|_| arena.fresh()).collect::<Vec<_>>();
        for slots in [&left, &right] {
            for pair in slots.windows(2) {
                arena.set(pair[0], TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(pair[1]))));
            }
            arena.set(*slots.last().unwrap(), TypeDescriptor::Int);
        }
        assert_eq!(arena.canonicalize_slots(), left.len());
        for (left, right) in left.iter().zip(&right) {
            assert_eq!(arena.root(*left), arena.root(*right));
        }
        assert_eq!(arena.canonicalize_slots(), 0);
        assert!(arena.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn declared_arguments_and_shared_bodies_are_slot_edges() {
        let mut arena = InferenceVariables::default();
        let item = arena.fresh();
        let body = Arc::new(TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(item))));
        let declared = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::applied(crate::ModuleId::ANONYMOUS, 42,
                &[TypeDescriptor::Inference(item)]),
            name: "Container".into(),
            body: Arc::clone(&body),
        });
        let left = arena.structure_edge(declared.clone());
        let right = arena.structure_edge(declared);
        let left_id = arena.known(left).unwrap();
        let right_id = arena.known(right).unwrap();
        assert_eq!(arena.arguments(left_id), arena.arguments(right_id));
        assert_eq!(arena.arguments(left_id)[0], item);
        assert_eq!(arena.imported_bodies.len(), 1);
        assert!(arena.same_slots(left, right));
        arena.set(item, TypeDescriptor::Int);
        assert!(arena.same_slots(left, right));
        assert!(arena.descriptor_views.iter().all(|view| view.get().is_none()));
        let InferenceConstructor::Declared { head, .. } = arena.constructor(left_id) else {
            panic!("expected nominal constructor metadata");
        };
        assert!(head.arguments().is_empty());
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum InferenceConstructor {
    Bound(TypeParameterId),
    Named(String),
    Declared { head: crate::value::DeclaredTypeId, name: String },
    Never,
    Type,
    Dyn,
    TypeOf,
    Int,
    Float,
    String,
    Bytes,
    AtomValue,
    Opaque(crate::NativeType),
    Atom(Atom),
    Array,
    Dict,
    Tagged(Atom),
    Tuple,
    Newtype,
    Struct(Box<[String]>),
    Enum(Box<[(String, bool)]>),
    PendingAlternatives,
    Function,
}

impl InferenceVariables {
    fn known(&self, slot: InferenceVariableId) -> Option<InferenceTypeId> {
        match self.nodes[self.root(slot).0 as usize].get() {
            InferenceNode::Known(id) => Some(id),
            _ => None,
        }
    }

    fn arguments(&self, id: InferenceTypeId) -> &[InferenceVariableId] {
        let ty = self.types[id.0 as usize];
        let start = ty.arguments_start as usize;
        &self.arguments[start..start + ty.arguments_len as usize]
    }

    fn constructor(&self, id: InferenceTypeId) -> &InferenceConstructor {
        &self.constructors[self.types[id.0 as usize].constructor.0 as usize]
    }

    fn push_type(
        &mut self,
        constructor: InferenceConstructor,
        arguments: &[InferenceVariableId],
    ) -> InferenceTypeId {
        let constructor = match self.constructor_ids.get(&constructor) {
            Some(id) => *id,
            None => {
                let id = InferenceConstructorId(u32::try_from(self.constructors.len())
                    .expect("inference constructor capacity exceeded"));
                let constructor = Arc::new(constructor);
                self.constructors.push(Arc::clone(&constructor));
                self.leaf_types.push(None);
                self.constructor_ids.insert(constructor, id);
                id
            }
        };
        if arguments.is_empty() && let Some(id) = self.leaf_types[constructor.0 as usize] {
            return id;
        }
        let id = InferenceTypeId(u32::try_from(self.types.len())
            .expect("inference type capacity exceeded"));
        let arguments_start = u32::try_from(self.arguments.len())
            .expect("inference argument capacity exceeded");
        let arguments_len = u32::try_from(arguments.len())
            .expect("inference argument capacity exceeded");
        arguments_start.checked_add(arguments_len).expect("inference argument capacity exceeded");
        self.arguments.extend_from_slice(arguments);
        self.types.push(InferenceType { constructor, arguments_start, arguments_len });
        self.descriptor_views.push(std::cell::OnceCell::new());
        self.normalized_body_indices.push(std::cell::Cell::new(u32::MAX));
        if arguments.is_empty() { self.leaf_types[constructor.0 as usize] = Some(id); }
        id
    }

    fn structure_edge(&mut self, ty: TypeDescriptor) -> InferenceVariableId {
        if let TypeDescriptor::Inference(slot) = ty { return slot; }
        let id = self.lower_structure(ty);
        let slot = self.fresh();
        self.initialize_known(slot, id);
        slot
    }

    fn structure_node(
        &mut self,
        constructor: InferenceConstructor,
        arguments: &[InferenceVariableId],
    ) -> InferenceVariableId {
        let id = self.push_type(constructor, arguments);
        let slot = self.fresh();
        self.initialize_known(slot, id);
        slot
    }

    fn lower_structure(&mut self, ty: TypeDescriptor) -> InferenceTypeId {
        use InferenceConstructor as C;
        let mut arguments = Vec::new();
        let constructor = match ty {
            TypeDescriptor::Inference(_) => unreachable!("proxy edges have no constructor"),
            TypeDescriptor::Bound(id) => C::Bound(id),
            TypeDescriptor::Named(name) => C::Named(name),
            TypeDescriptor::Declared(declared) => {
                arguments.extend(declared.id.arguments().iter()
                    .map(|argument| self.structure_edge(argument.clone())));
                arguments.push(self.import_declared_body(&declared));
                C::Declared { head: declared.id.reapply(&[]), name: declared.name }
            }
            TypeDescriptor::Never => C::Never,
            TypeDescriptor::Type => C::Type,
            TypeDescriptor::Dyn => C::Dyn,
            TypeDescriptor::Int => C::Int,
            TypeDescriptor::Float => C::Float,
            TypeDescriptor::String => C::String,
            TypeDescriptor::Bytes => C::Bytes,
            TypeDescriptor::AtomValue => C::AtomValue,
            TypeDescriptor::Opaque(native) => C::Opaque(native),
            TypeDescriptor::Atom(atom) => C::Atom(atom),
            TypeDescriptor::Array(item) => { arguments.push(self.structure_edge(*item)); C::Array }
            TypeDescriptor::Dict(item) => { arguments.push(self.structure_edge(*item)); C::Dict }
            TypeDescriptor::Newtype(item) => { arguments.push(self.structure_edge(*item)); C::Newtype }
            TypeDescriptor::TypeOf(item) => { arguments.push(self.structure_edge(*item)); C::TypeOf }
            TypeDescriptor::Tagged { tag, payload } => {
                arguments.push(self.structure_edge(*payload));
                C::Tagged(tag)
            }
            TypeDescriptor::Tuple(items) => {
                arguments.extend(items.into_iter().map(|item| self.structure_edge(item)));
                C::Tuple
            }
            TypeDescriptor::Struct(fields) => {
                let mut names = Vec::with_capacity(fields.len());
                for (name, item) in fields {
                    names.push(name);
                    arguments.push(self.structure_edge(item));
                }
                C::Struct(names.into())
            }
            TypeDescriptor::Enum(variants) => {
                let mut names = Vec::with_capacity(variants.len());
                for (name, payload) in variants {
                    names.push((name, payload.is_some()));
                    if let Some(item) = payload { arguments.push(self.structure_edge(*item)); }
                }
                C::Enum(names.into())
            }
            TypeDescriptor::PendingAlternatives(items) => {
                arguments.extend(items.into_iter().map(|item| self.structure_edge(item)));
                C::PendingAlternatives
            }
            TypeDescriptor::Function { parameters, result } => {
                arguments.extend(parameters.into_iter().map(|item| self.structure_edge(item)));
                arguments.push(self.structure_edge(*result));
                C::Function
            }
        };
        self.push_type(constructor, &arguments)
    }

    fn instantiate_descriptor(
        &mut self,
        ty: &TypeDescriptor,
        parameters: &HashMap<TypeParameterId, InferenceVariableId>,
    ) -> InferenceVariableId {
        self.instantiate_descriptor_with(ty, parameters, &mut HashMap::new())
    }

    fn instantiate_descriptor_with(
        &mut self,
        ty: &TypeDescriptor,
        parameters: &HashMap<TypeParameterId, InferenceVariableId>,
        bodies: &mut HashMap<*const TypeDescriptor, InferenceVariableId>,
    ) -> InferenceVariableId {
        use InferenceConstructor as C;
        let mut arguments = Vec::new();
        let constructor = match ty {
            TypeDescriptor::Inference(slot) => return *slot,
            TypeDescriptor::Bound(parameter) => {
                if let Some(slot) = parameters.get(parameter) { return *slot; }
                C::Bound(*parameter)
            }
            TypeDescriptor::Declared(declared) => {
                for argument in declared.id.arguments() {
                    arguments.push(self.instantiate_descriptor_with(argument, parameters, bodies));
                }
                let body = if arguments.is_empty() {
                    self.import_declared_body(declared)
                } else if let Some(slot) = bodies.get(&Arc::as_ptr(&declared.body)) {
                    *slot
                } else {
                    let slot = self.instantiate_descriptor_with(&declared.body, parameters, bodies);
                    bodies.insert(Arc::as_ptr(&declared.body), slot);
                    slot
                };
                arguments.push(body);
                C::Declared { head: declared.id.reapply(&[]), name: declared.name.clone() }
            }
            TypeDescriptor::Array(item) | TypeDescriptor::Dict(item)
            | TypeDescriptor::Newtype(item) | TypeDescriptor::TypeOf(item)
            | TypeDescriptor::Tagged { payload: item, .. } => {
                arguments.push(self.instantiate_descriptor_with(item, parameters, bodies));
                match ty {
                    TypeDescriptor::Array(_) => C::Array,
                    TypeDescriptor::Dict(_) => C::Dict,
                    TypeDescriptor::Newtype(_) => C::Newtype,
                    TypeDescriptor::TypeOf(_) => C::TypeOf,
                    TypeDescriptor::Tagged { tag, .. } => C::Tagged(tag.clone()),
                    _ => unreachable!(),
                }
            }
            TypeDescriptor::Tuple(items) | TypeDescriptor::PendingAlternatives(items) => {
                for item in items {
                    arguments.push(self.instantiate_descriptor_with(item, parameters, bodies));
                }
                if matches!(ty, TypeDescriptor::Tuple(_)) { C::Tuple } else { C::PendingAlternatives }
            }
            TypeDescriptor::Struct(fields) => {
                for item in fields.values() {
                    arguments.push(self.instantiate_descriptor_with(item, parameters, bodies));
                }
                C::Struct(fields.keys().cloned().collect())
            }
            TypeDescriptor::Enum(variants) => {
                for item in variants.values().flatten() {
                    arguments.push(self.instantiate_descriptor_with(item, parameters, bodies));
                }
                C::Enum(variants.iter().map(|(name, payload)| (name.clone(), payload.is_some())).collect())
            }
            TypeDescriptor::Function { parameters: inputs, result } => {
                for item in inputs {
                    arguments.push(self.instantiate_descriptor_with(item, parameters, bodies));
                }
                arguments.push(self.instantiate_descriptor_with(result, parameters, bodies));
                C::Function
            }
            ty => return self.structure_edge(ty.clone()),
        };
        self.structure_node(constructor, &arguments)
    }

    fn import_declared_body(&mut self, declared: &DeclaredTypeDescriptor) -> InferenceVariableId {
        // A complete nominal identity fixes its skeleton. Normalization may
        // produce new Arc addresses for that same immutable body.
        if !matches!(declared.body.as_ref(), TypeDescriptor::Never)
            && !declared.id.arguments().iter().any(type_identity_is_symbolic)
            && !contains_type_variable(&declared.body)
            && !type_identity_contains_bound_parameter(&declared.body)
        {
            // Recursive descriptor adapters can expose different amounts of a
            // skeleton; an incomplete view must not replace a complete one.
            if let Some(views) = self.resolved_declared_bodies.get(&declared.id)
                && let Some((_, slot)) = views.iter().find(|(body, _)| body == &declared.body)
            { return *slot; }
            let slot = self.import_body(Arc::clone(&declared.body));
            self.resolved_declared_bodies.entry(declared.id.clone())
                .or_default().push((Arc::clone(&declared.body), slot));
            return slot;
        }
        self.import_body(Arc::clone(&declared.body))
    }

    fn import_body(&mut self, body: Arc<TypeDescriptor>) -> InferenceVariableId {
        let address = Arc::as_ptr(&body);
        if let Some((_, slot)) = self.imported_bodies.get(&address) { return *slot; }
        let slot = self.fresh();
        self.imported_bodies.insert(address, (Arc::clone(&body), slot));
        self.set(slot, body.as_ref().clone());
        slot
    }

    fn normalized_body(&self, id: InferenceTypeId) -> Option<Arc<TypeDescriptor>> {
        let index = self.normalized_body_indices[id.0 as usize].get();
        if index == u32::MAX { return None; }
        let bodies = self.normalized_bodies.borrow();
        let (revision, body) = &bodies[index as usize];
        (*revision == self.revision).then(|| Arc::clone(body))
    }

    fn cache_normalized_body(&self, id: InferenceTypeId, body: Arc<TypeDescriptor>) {
        let index = &self.normalized_body_indices[id.0 as usize];
        let mut bodies = self.normalized_bodies.borrow_mut();
        if index.get() == u32::MAX {
            let next = u32::try_from(bodies.len()).expect("normalized body capacity exceeded");
            assert_ne!(next, u32::MAX, "normalized body capacity exceeded");
            bodies.push((self.revision, body));
            index.set(next);
        } else {
            bodies[index.get() as usize] = (self.revision, body);
        }
    }

    // Compatibility adapter for descriptor-facing consumers. Solver storage and
    // graph passes never depend on these lazily constructed, immutable views.
    fn descriptor_view(&self, id: InferenceTypeId) -> &Arc<TypeDescriptor> {
        self.descriptor_views[id.0 as usize].get_or_init(|| {
            use InferenceConstructor as C;
            let arguments = self.arguments(id);
            let edge = |index: usize| TypeDescriptor::Inference(arguments[index]);
            let unary = || Box::new(edge(0));
            let view = Arc::new(match self.constructor(id) {
                C::Bound(id) => TypeDescriptor::Bound(*id),
                C::Named(name) => TypeDescriptor::Named(name.clone()),
                C::Declared { head, name } => {
                    let (body, parameters) = arguments.split_last().expect("declared body edge");
                    let parameters = parameters.iter().copied().map(TypeDescriptor::Inference).collect::<Vec<_>>();
                    TypeDescriptor::Declared(DeclaredTypeDescriptor {
                        id: head.reapply(&parameters),
                        name: name.clone(),
                        body: self.bound(*body).unwrap_or_else(|| Arc::new(TypeDescriptor::Inference(*body))),
                    })
                }
                C::Never => TypeDescriptor::Never,
                C::Type => TypeDescriptor::Type,
                C::Dyn => TypeDescriptor::Dyn,
                C::Int => TypeDescriptor::Int,
                C::Float => TypeDescriptor::Float,
                C::String => TypeDescriptor::String,
                C::Bytes => TypeDescriptor::Bytes,
                C::AtomValue => TypeDescriptor::AtomValue,
                C::Opaque(native) => TypeDescriptor::Opaque(native.clone()),
                C::Atom(atom) => TypeDescriptor::Atom(atom.clone()),
                C::Array => TypeDescriptor::Array(unary()),
                C::Dict => TypeDescriptor::Dict(unary()),
                C::Newtype => TypeDescriptor::Newtype(unary()),
                C::TypeOf => TypeDescriptor::TypeOf(unary()),
                C::Tagged(tag) => TypeDescriptor::Tagged { tag: tag.clone(), payload: unary() },
                C::Tuple => TypeDescriptor::Tuple(arguments.iter().copied().map(TypeDescriptor::Inference).collect()),
                C::Struct(names) => TypeDescriptor::Struct(names.iter().cloned().zip(
                    arguments.iter().copied().map(TypeDescriptor::Inference)).collect()),
                C::Enum(names) => {
                    let mut payloads = arguments.iter();
                    TypeDescriptor::Enum(names.iter().map(|(name, payload)| (name.clone(), payload.then(|| {
                        Box::new(TypeDescriptor::Inference(*payloads.next().expect("enum payload edge")))
                    }))).collect())
                }
                C::PendingAlternatives => TypeDescriptor::PendingAlternatives(
                    arguments.iter().copied().map(TypeDescriptor::Inference).collect()),
                C::Function => {
                    let (result, parameters) = arguments.split_last().expect("function result edge");
                    TypeDescriptor::Function {
                        parameters: parameters.iter().copied().map(TypeDescriptor::Inference).collect(),
                        result: Box::new(TypeDescriptor::Inference(*result)),
                    }
                }
            });
            self.descriptor_view_ids.borrow_mut().insert(Arc::as_ptr(&view), id);
            view
        })
    }
}
