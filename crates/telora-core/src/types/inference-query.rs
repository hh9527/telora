// A borrowed descriptor is an ingress adapter; a stored row never creates a
// descriptor view merely to expose its constructor. IDs belong to this solver.
#[derive(Clone, Copy)]
enum InferenceView<'a> {
    Descriptor(&'a TypeDescriptor),
    Row(InferenceTypeId),
    Unknown(InferenceVariableId),
    Conflicted(u32),
}

impl InferenceVariables {
    fn view<'a>(&self, ty: &'a TypeDescriptor) -> InferenceView<'a> {
        #[cfg(feature = "inference-profile")]
        profile_increment(&self.profile.head_queries);
        let TypeDescriptor::Inference(slot) = ty else {
            return InferenceView::Descriptor(ty);
        };
        self.slot_view(*slot)
    }

    fn slot_view(&self, slot: InferenceVariableId) -> InferenceView<'static> {
        let root = self.root(slot);
        match self.nodes[root.0 as usize].get() {
            InferenceNode::Unknown => InferenceView::Unknown(root),
            InferenceNode::Known(row) => InferenceView::Row(row),
            InferenceNode::Conflicted(error) => InferenceView::Conflicted(error),
            InferenceNode::ProxyTo(_) => unreachable!("root follows proxies"),
        }
    }

    // Alias completion and semantic alternative collapse remain on the existing
    // conversion path. An ordinary known head cannot normalize into Unchecked.
    fn may_be_unchecked(&self, ty: &TypeDescriptor) -> bool {
        match self.view(ty) {
            InferenceView::Row(row) => match self.constructor(row) {
                InferenceConstructor::Named(_) | InferenceConstructor::PendingAlternatives => true,
                InferenceConstructor::Declared { head, .. } => {
                    head.constructor() == unchecked_type_constructor()
                }
                _ => false,
            },
            InferenceView::Descriptor(ty) => match ty {
                TypeDescriptor::Named(_) | TypeDescriptor::PendingAlternatives(_) => true,
                TypeDescriptor::Declared(declared) => {
                    declared.id.constructor() == unchecked_type_constructor()
                }
                _ => false,
            },
            InferenceView::Unknown(_) | InferenceView::Conflicted(_) => true,
        }
    }

    fn query_unresolved(&self, ty: &TypeDescriptor, exposed: bool) -> Option<bool> {
        let mut pending = vec![self.view(ty)];
        let mut visited = HashSet::new();
        while let Some(view) = pending.pop() {
            match view {
                InferenceView::Unknown(_) | InferenceView::Conflicted(_) => return Some(true),
                InferenceView::Row(row) => {
                    if !visited.insert(row.0) {
                        continue;
                    }
                    let constructor = self.constructor(row);
                    if matches!(constructor, InferenceConstructor::PendingAlternatives)
                        || matches!(constructor, InferenceConstructor::Declared { head, .. }
                            if head.constructor() == unchecked_type_constructor())
                    {
                        return None;
                    }
                    let arguments = self.arguments(row);
                    let arguments = if exposed
                        && matches!(constructor, InferenceConstructor::Declared { .. })
                    {
                        &arguments[..arguments.len() - 1]
                    } else {
                        arguments
                    };
                    pending.extend(arguments.iter().map(|slot| self.slot_view(*slot)));
                }
                InferenceView::Descriptor(ty) => {
                    let mut push = |ty| pending.push(self.view(ty));
                    match ty {
                        TypeDescriptor::PendingAlternatives(_) => return None,
                        TypeDescriptor::Declared(declared) => {
                            if declared.id.constructor() == unchecked_type_constructor() {
                                return None;
                            }
                            for argument in declared.id.arguments() {
                                push(argument);
                            }
                            if !exposed {
                                push(&declared.body);
                            }
                        }
                        TypeDescriptor::Array(item)
                        | TypeDescriptor::Newtype(item)
                        | TypeDescriptor::Dict(item)
                        | TypeDescriptor::TypeOf(item)
                        | TypeDescriptor::Tagged { payload: item, .. } => push(item),
                        TypeDescriptor::Tuple(items) => {
                            for item in items {
                                push(item);
                            }
                        }
                        TypeDescriptor::Struct(fields) => {
                            for item in fields.values() {
                                push(item);
                            }
                        }
                        TypeDescriptor::Enum(variants) => {
                            for item in variants.values().flatten() {
                                push(item);
                            }
                        }
                        TypeDescriptor::Function { parameters, result } => {
                            for parameter in parameters {
                                push(parameter);
                            }
                            push(result);
                        }
                        _ => {}
                    }
                }
            }
        }
        Some(false)
    }

    fn query_type_value(&self, ty: &TypeDescriptor) -> Option<bool> {
        let mut current = self.view(ty);
        let mut visited = HashSet::new();
        loop {
            match current {
                InferenceView::Unknown(_) | InferenceView::Conflicted(_) => return Some(false),
                InferenceView::Row(row) => {
                    if !visited.insert(row.0) {
                        return None;
                    }
                    match self.constructor(row) {
                        InferenceConstructor::Type | InferenceConstructor::TypeOf => {
                            return Some(true);
                        }
                        InferenceConstructor::Function => {
                            current = self
                                .slot_view(*self.arguments(row).last().expect("function result"));
                        }
                        InferenceConstructor::PendingAlternatives => return None,
                        _ => return Some(false),
                    }
                }
                InferenceView::Descriptor(ty) => match ty {
                    TypeDescriptor::Type | TypeDescriptor::TypeOf(_) => return Some(true),
                    TypeDescriptor::Function { result, .. } => current = self.view(result),
                    TypeDescriptor::PendingAlternatives(_) => return None,
                    _ => return Some(false),
                },
            }
        }
    }
}

impl GenericInference<'_> {
    fn is_type_of(&self, ty: &TypeDescriptor) -> bool {
        match self.variables.view(ty) {
            InferenceView::Row(row) => match self.variables.constructor(row) {
                InferenceConstructor::TypeOf => true,
                InferenceConstructor::PendingAlternatives => matches!(self.normalize(ty), TypeDescriptor::TypeOf(_)),
                _ => false,
            },
            InferenceView::Descriptor(TypeDescriptor::TypeOf(_)) => true,
            InferenceView::Descriptor(TypeDescriptor::PendingAlternatives(_)) => matches!(self.normalize(ty), TypeDescriptor::TypeOf(_)),
            _ => false,
        }
    }

    fn contains_owned_unknown(&self, ty: &TypeDescriptor, first: u32) -> bool {
        enum Input<'a> { Descriptor(&'a TypeDescriptor), Slot(InferenceVariableId) }
        let mut pending = vec![Input::Descriptor(ty)];
        let mut visited = HashSet::new();
        while let Some(input) = pending.pop() {
            match input {
                Input::Slot(slot) => {
                    let slot = self.variables.root(slot);
                    let Some(row) = self.variables.known(slot) else {
                        if slot.0 >= first { return true; }
                        continue;
                    };
                    if !visited.insert(row.0) { continue; }
                    match self.variables.constructor(row) {
                        // Preserve the monomorphic-binding boundary: nominal
                        // definitions are checked at their own declaration.
                        InferenceConstructor::Declared { .. } => {},
                        InferenceConstructor::PendingAlternatives => {
                            if contains_inference_variable_at_or_after(&self.normalize(&TypeDescriptor::Inference(slot)), first) { return true; }
                        }
                        _ => pending.extend(self.variables.arguments(row).iter().copied().map(Input::Slot)),
                    }
                }
                Input::Descriptor(ty) => match ty {
                    TypeDescriptor::Inference(slot) => pending.push(Input::Slot(*slot)),
                    TypeDescriptor::Array(item) | TypeDescriptor::Dict(item) | TypeDescriptor::Newtype(item)
                    | TypeDescriptor::TypeOf(item) | TypeDescriptor::Tagged { payload: item, .. } => pending.push(Input::Descriptor(item)),
                    TypeDescriptor::Tuple(items) => pending.extend(items.iter().map(Input::Descriptor)),
                    TypeDescriptor::Struct(fields) => pending.extend(fields.values().map(Input::Descriptor)),
                    TypeDescriptor::Enum(variants) => pending.extend(variants.values().flatten().map(|ty| Input::Descriptor(ty))),
                    TypeDescriptor::Function { parameters, result } => {
                        pending.extend(parameters.iter().map(Input::Descriptor));
                        pending.push(Input::Descriptor(result));
                    }
                    TypeDescriptor::PendingAlternatives(_) => {
                        if contains_inference_variable_at_or_after(&self.normalize(ty), first) { return true; }
                    }
                    _ => {},
                },
            }
        }
        false
    }

    fn nominal_view<'a>(&'a self, ty: &'a TypeDescriptor) -> Option<InferenceView<'a>> {
        let mut current = self.variables.view(ty);
        let mut names = HashSet::new();
        loop {
            let name = match current {
                InferenceView::Row(row) => match self.variables.constructor(row) {
                    InferenceConstructor::Declared { .. } => return Some(current),
                    InferenceConstructor::Named(name) => name,
                    _ => return None,
                },
                InferenceView::Descriptor(TypeDescriptor::Declared(_)) => return Some(current),
                InferenceView::Descriptor(TypeDescriptor::Named(name)) => name,
                _ => return None,
            };
            if !names.insert(name.as_str()) { return None; }
            current = self.variables.view(self.named_type(name)?);
        }
    }

    fn declared_constructor(&self, ty: &TypeDescriptor) -> Option<crate::TypeConstructorId> {
        match self.nominal_view(ty)? {
            InferenceView::Row(row) => {
                let InferenceConstructor::Declared { head, .. } = self.variables.constructor(row) else { unreachable!() };
                Some(head.constructor())
            }
            InferenceView::Descriptor(TypeDescriptor::Declared(declared)) => Some(declared.id.constructor()),
            _ => unreachable!("nominal view"),
        }
    }

    fn matching_nominal_arguments(&self, left: &TypeDescriptor, right: &TypeDescriptor)
        -> Option<(Vec<TypeDescriptor>, Vec<TypeDescriptor>)>
    {
        let constructor = self.declared_constructor(left)?;
        if constructor != self.declared_constructor(right)? { return None; }
        let arguments = |ty: &TypeDescriptor| {
            if constructor == unchecked_type_constructor() {
                return self.declared_identity(ty).expect("nominal identity").arguments().to_vec();
            }
            match self.nominal_view(ty).expect("nominal head") {
                InferenceView::Row(row) => {
                    let arguments = self.variables.arguments(row);
                    arguments[..arguments.len() - 1].iter().copied().map(TypeDescriptor::Inference).collect()
                }
                InferenceView::Descriptor(TypeDescriptor::Declared(declared)) => declared.id.arguments().to_vec(),
                _ => unreachable!("nominal view"),
            }
        };
        let (left, right) = (arguments(left), arguments(right));
        (left.len() == right.len()).then_some((left, right))
    }

    fn declared_context(&self, ty: &TypeDescriptor) -> Option<DeclaredTypeDescriptor> {
        let mut current = self.variables.view(ty);
        let mut names = HashSet::new();
        loop {
            match current {
                InferenceView::Row(row) => match self.variables.constructor(row) {
                    InferenceConstructor::Declared { .. } | InferenceConstructor::Named(_)
                    | InferenceConstructor::PendingAlternatives => {
                        current = InferenceView::Descriptor(self.variables.descriptor_view(row));
                    }
                    _ => return None,
                },
                InferenceView::Descriptor(descriptor) => match descriptor {
                    TypeDescriptor::Named(name) => {
                        if !names.insert(name.clone()) { return None; }
                        current = self.variables.view(self.named_type(name)?);
                    }
                    TypeDescriptor::Declared(declared) => {
                        if declared.id.constructor() == unchecked_type_constructor() {
                            let TypeDescriptor::Declared(declared) = self.normalize(descriptor) else { unreachable!() };
                            return Some(declared);
                        }
                        let arguments = declared.id.arguments().iter().map(|argument| self.normalize(argument)).collect::<Vec<_>>();
                        let id = declared.id.reapply(&arguments);
                        let body = if matches!(declared.body.as_ref(), TypeDescriptor::Never) {
                            self.declared_bodies.get(&id).unwrap_or(&declared.body)
                        } else { &declared.body };
                        return Some(DeclaredTypeDescriptor { id, name: declared.name.clone(), body: Arc::clone(body) });
                    }
                    TypeDescriptor::PendingAlternatives(_) => {
                        let resolved = self.normalize(descriptor);
                        return if matches!(resolved, TypeDescriptor::PendingAlternatives(_)) { None }
                            else { self.declared_context(&resolved) };
                    }
                    _ => return None,
                },
                InferenceView::Unknown(_) | InferenceView::Conflicted(_) => return None,
            }
        }
    }

    // Collection literals need their outer constructor and child slots, not
    // recursively normalized nominal bodies. Alternatives still require their
    // semantic join before a single outer constructor can be selected.
    fn collection_shape(&self, ty: &TypeDescriptor) -> Option<TypeDescriptor> {
        match self.variables.view(ty) {
            InferenceView::Row(row) => {
                let arguments = self.variables.arguments(row);
                let child = |slot| {
                    let ty = TypeDescriptor::Inference(slot);
                    if matches!(self.variables.view(&ty), InferenceView::Row(row)
                        if matches!(self.variables.constructor(row), InferenceConstructor::PendingAlternatives))
                    { self.normalize(&ty) } else { ty }
                };
                let edge = |index: usize| child(arguments[index]);
                Some(match self.variables.constructor(row) {
                    InferenceConstructor::Array => TypeDescriptor::Array(Box::new(edge(0))),
                    InferenceConstructor::Dict => TypeDescriptor::Dict(Box::new(edge(0))),
                    InferenceConstructor::Tuple => TypeDescriptor::Tuple(arguments.iter().copied().map(child).collect()),
                    InferenceConstructor::Struct(names) => TypeDescriptor::Struct(names.iter().cloned().zip(arguments.iter().copied().map(child)).collect()),
                    InferenceConstructor::Type => TypeDescriptor::Type,
                    InferenceConstructor::TypeOf => TypeDescriptor::TypeOf(Box::new(edge(0))),
                    InferenceConstructor::PendingAlternatives => return self.collection_shape(&self.normalize(ty)),
                    _ => return None,
                })
            }
            InferenceView::Descriptor(ty) => match ty {
                TypeDescriptor::Array(_) | TypeDescriptor::Dict(_) | TypeDescriptor::Tuple(_)
                | TypeDescriptor::Struct(_) | TypeDescriptor::Type | TypeDescriptor::TypeOf(_) => Some(ty.clone()),
                TypeDescriptor::PendingAlternatives(_) => {
                    let resolved = self.normalize(ty);
                    if matches!(resolved, TypeDescriptor::PendingAlternatives(_)) { None }
                    else { self.collection_shape(&resolved) }
                }
                _ => None,
            },
            InferenceView::Unknown(_) | InferenceView::Conflicted(_) => None,
        }
    }

    // Read just the result edge of a function/type witness. A family constructor
    // has one Function layer followed by TypeOf, not an arbitrary callable chain.
    fn constructor_result(&self, ty: &TypeDescriptor, function: bool) -> Option<TypeDescriptor> {
        match self.variables.view(ty) {
            InferenceView::Row(row) => match self.variables.constructor(row) {
                InferenceConstructor::Function if function => self.variables.arguments(row).last().copied().map(TypeDescriptor::Inference),
                InferenceConstructor::TypeOf if !function => Some(TypeDescriptor::Inference(self.variables.arguments(row)[0])),
                InferenceConstructor::PendingAlternatives => {
                    let resolved = self.normalize(ty);
                    if matches!(resolved, TypeDescriptor::PendingAlternatives(_)) { None }
                    else { self.constructor_result(&resolved, function) }
                }
                _ => None,
            },
            InferenceView::Descriptor(ty) => match ty {
                TypeDescriptor::Function { result, .. } if function => Some(result.as_ref().clone()),
                TypeDescriptor::TypeOf(result) if !function => Some(result.as_ref().clone()),
                TypeDescriptor::PendingAlternatives(_) => {
                    let resolved = self.normalize(ty);
                    if matches!(resolved, TypeDescriptor::PendingAlternatives(_)) { None }
                    else { self.constructor_result(&resolved, function) }
                }
                _ => None,
            },
            InferenceView::Unknown(_) | InferenceView::Conflicted(_) => None,
        }
    }

    fn enum_member(&self, ty: &TypeDescriptor, tag: &str)
        -> Result<Option<(TypeDescriptor, ValueConstructor)>, String>
    {
        let result = self.constructor_result(ty, true);
        let Some(owner) = self.constructor_result(result.as_ref().unwrap_or(ty), false) else { return Ok(None); };
        match self.variables.view(&owner) {
            InferenceView::Row(row) => match self.variables.constructor(row) {
                InferenceConstructor::Declared { head, .. } => {
                    // Unchecked derives its body from its solved argument.
                    if head.constructor() == unchecked_type_constructor() {
                        let resolved = self.normalize(&owner);
                        let TypeDescriptor::Declared(declared) = &resolved else { unreachable!("Unchecked identity"); };
                        return self.enum_member_from_body(&resolved, &declared.body, tag);
                    }
                    let body = TypeDescriptor::Inference(*self.variables.arguments(row).last().expect("nominal body"));
                    self.enum_member_from_body(&owner, &body, tag)
                }
                InferenceConstructor::PendingAlternatives => {
                    let resolved = self.normalize(&owner);
                    if matches!(resolved, TypeDescriptor::PendingAlternatives(_)) { return Ok(None); }
                    self.enum_member(&TypeDescriptor::TypeOf(Box::new(resolved)), tag)
                }
                _ => self.enum_member_from_body(&owner, &owner, tag),
            },
            InferenceView::Descriptor(TypeDescriptor::Declared(declared)) => {
                if declared.id.constructor() == unchecked_type_constructor() {
                    let resolved = self.normalize(&owner);
                    let TypeDescriptor::Declared(declared) = &resolved else { unreachable!("Unchecked identity"); };
                    return self.enum_member_from_body(&resolved, &declared.body, tag);
                }
                self.enum_member_from_body(&owner, &declared.body, tag)
            }
            InferenceView::Descriptor(TypeDescriptor::PendingAlternatives(_)) => {
                let resolved = self.normalize(&owner);
                if matches!(resolved, TypeDescriptor::PendingAlternatives(_)) { return Ok(None); }
                self.enum_member(&TypeDescriptor::TypeOf(Box::new(resolved)), tag)
            }
            _ => self.enum_member_from_body(&owner, &owner, tag),
        }
    }

    fn enum_member_from_body(&self, owner: &TypeDescriptor, body: &TypeDescriptor, tag: &str)
        -> Result<Option<(TypeDescriptor, ValueConstructor)>, String>
    {
        let payload = match self.variables.view(body) {
            InferenceView::Row(row) => match self.variables.constructor(row) {
                InferenceConstructor::Enum(variants) => {
                    variants.binary_search_by(|(name, _)| name.as_str().cmp(tag)).ok().map(|index| {
                        variants[index].1.then(|| {
                            let offset = variants[..index].iter().filter(|(_, payload)| *payload).count();
                            TypeDescriptor::Inference(self.variables.arguments(row)[offset])
                        })
                    })
                }
                InferenceConstructor::PendingAlternatives => {
                    let resolved = self.normalize(body);
                    if matches!(resolved, TypeDescriptor::PendingAlternatives(_)) { return Ok(None); }
                    return self.enum_member_from_body(owner, &resolved, tag);
                }
                _ => return Ok(None),
            },
            InferenceView::Descriptor(TypeDescriptor::Enum(variants)) => variants.get(tag).map(|payload| payload.as_deref().cloned()),
            InferenceView::Descriptor(TypeDescriptor::PendingAlternatives(_)) => {
                let resolved = self.normalize(body);
                if matches!(resolved, TypeDescriptor::PendingAlternatives(_)) { return Ok(None); }
                return self.enum_member_from_body(owner, &resolved, tag);
            }
            _ => return Ok(None),
        }.ok_or_else(|| format!("enum {} has no member {tag:?}", self.normalize(owner).display_name()))?;
        let has_payload = payload.is_some();
        let ty = match payload {
            Some(payload) => TypeDescriptor::Function { parameters: vec![payload], result: Box::new(owner.clone()) },
            None => owner.clone(),
        };
        Ok(Some((ty, ValueConstructor::EnumMember { tag: tag.into(), has_payload })))
    }

    // Nominal refinement visits matching children itself. Expanding every child
    // here would repeatedly rebuild the same subtree at each recursion level.
    fn nominal_refinement_shape(&self, ty: &TypeDescriptor) -> TypeDescriptor {
        let mut target = std::borrow::Cow::Borrowed(ty);
        let mut names = HashSet::new();
        loop {
            let current = self.variables.view(&target);
            let name = match current {
                InferenceView::Row(row) => match self.variables.constructor(row) {
                    InferenceConstructor::Named(name) => Some(name),
                    _ => None,
                },
                InferenceView::Descriptor(TypeDescriptor::Named(name)) => Some(name),
                _ => None,
            };
            if let Some(name) = name {
                if !names.insert(name.clone()) { return TypeDescriptor::Named(name.clone()); }
                let Some(resolved) = self.named_type(name) else { return TypeDescriptor::Named(name.clone()); };
                target = std::borrow::Cow::Borrowed(resolved);
                continue;
            }
            if matches!(current, InferenceView::Row(row) if matches!(self.variables.constructor(row), InferenceConstructor::PendingAlternatives))
                || matches!(current, InferenceView::Descriptor(TypeDescriptor::PendingAlternatives(_)))
            {
                let resolved = self.normalize(&target);
                if matches!(resolved, TypeDescriptor::PendingAlternatives(_)) { return resolved; }
                target = std::borrow::Cow::Owned(resolved);
                continue;
            }
            return match current {
                InferenceView::Row(row) => {
                    use InferenceConstructor as C;
                    let arguments = self.variables.arguments(row);
                    let edge = |index: usize| TypeDescriptor::Inference(arguments[index]);
                    let unary = || Box::new(edge(0));
                    let all = || arguments.iter().copied().map(TypeDescriptor::Inference).collect();
                    match self.variables.constructor(row) {
                        C::Named(_) | C::PendingAlternatives => unreachable!("resolved refinement head"),
                        C::Declared { .. } => TypeDescriptor::Declared(self.declared_context(&target).expect("nominal refinement target")),
                        C::Array => TypeDescriptor::Array(unary()),
                        C::Dict => TypeDescriptor::Dict(unary()),
                        C::Newtype => TypeDescriptor::Newtype(unary()),
                        C::TypeOf => TypeDescriptor::TypeOf(unary()),
                        C::Tagged(tag) => TypeDescriptor::Tagged { tag: tag.clone(), payload: unary() },
                        C::Tuple => TypeDescriptor::Tuple(all()),
                        C::Struct(fields) => TypeDescriptor::Struct(fields.iter().cloned().zip(arguments.iter().copied().map(TypeDescriptor::Inference)).collect()),
                        C::Enum(variants) => {
                            let mut payloads = arguments.iter();
                            TypeDescriptor::Enum(variants.iter().map(|(name, has_payload)| (name.clone(), has_payload.then(|| {
                                Box::new(TypeDescriptor::Inference(*payloads.next().expect("enum payload")))
                            }))).collect())
                        }
                        C::Function => {
                            let (result, parameters) = arguments.split_last().expect("function result");
                            TypeDescriptor::Function {
                                parameters: parameters.iter().copied().map(TypeDescriptor::Inference).collect(),
                                result: Box::new(TypeDescriptor::Inference(*result)),
                            }
                        }
                        C::Bound(id) => TypeDescriptor::Bound(*id),
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
                    }
                }
                InferenceView::Descriptor(TypeDescriptor::Declared(_)) => {
                    TypeDescriptor::Declared(self.declared_context(&target).expect("nominal refinement target"))
                }
                InferenceView::Descriptor(ty) => ty.clone(),
                InferenceView::Unknown(slot) => TypeDescriptor::Inference(slot),
                InferenceView::Conflicted(_) => self.normalize(&target),
            };
        }
    }

    fn has_unresolved(&self, ty: &TypeDescriptor) -> bool {
        self.variables
            .query_unresolved(ty, false)
            .unwrap_or_else(|| contains_type_variable(&self.normalize(ty)))
    }

    fn has_exposed_unresolved(&self, ty: &TypeDescriptor) -> bool {
        self.variables
            .query_unresolved(ty, true)
            .unwrap_or_else(|| contains_exposed_type_variable(&self.normalize(ty)))
    }

    fn expects_type_value(&self, ty: &TypeDescriptor) -> bool {
        self.variables
            .query_type_value(ty)
            .unwrap_or_else(|| expects_type_value(&self.normalize(ty)))
    }
}

#[cfg(test)]
mod inference_query_tests {
    use super::*;

    #[test]
    fn nominal_refinement_updates_child_slots_without_rebuilding_parents_or_other_calls() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let names = BTreeMap::new();
        let traits = BTreeMap::new();
        let mut inference = GenericInference::new(&schemes, &hir, &interfaces, &names,
            InferenceAnnotationInputs::default(), &[], &[], &traits, None, true, None, None);
        let int = inference.variables.structure_edge(TypeDescriptor::Int);
        let record = inference.variables.structure_node(InferenceConstructor::Struct(Box::new(["value".into()])), &[int]);
        let other_record = inference.variables.structure_node(InferenceConstructor::Struct(Box::new(["value".into()])), &[int]);
        let parameter = inference.variables.structure_node(InferenceConstructor::Array, &[record]);
        let other_parameter = inference.variables.structure_node(InferenceConstructor::Array, &[other_record]);
        let parent_row = inference.variables.known(parameter).unwrap();
        let other_row = inference.variables.known(other_record).unwrap();
        let body = TypeDescriptor::Struct(BTreeMap::from([("value".into(), TypeDescriptor::Int)]));
        let nominal = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 129),
            name: "Record".into(), body: Arc::new(body),
        });
        let actual = inference.variables.structure_edge(TypeDescriptor::Array(Box::new(nominal.clone())));
        let root = TypeDescriptor::Inference(parameter);
        assert_eq!(inference.refine_argument_nominal_context(&root, &TypeDescriptor::Inference(actual)).unwrap(), root);
        assert_eq!(inference.variables.known(parameter), Some(parent_row));
        assert_eq!(inference.variables.arguments(parent_row), &[record]);
        assert_eq!(inference.normalize(&TypeDescriptor::Inference(record)), nominal);
        assert_eq!(inference.variables.known(other_record), Some(other_row));
        assert!(matches!(inference.variables.constructor(other_row), InferenceConstructor::Struct(_)));
        assert_eq!(inference.variables.arguments(inference.variables.known(other_parameter).unwrap()), &[other_record]);
        let rows = inference.variables.types.len();
        inference.refine_argument_nominal_context(&root, &TypeDescriptor::Inference(actual)).unwrap();
        assert_eq!(inference.variables.types.len(), rows);
    }

    #[test]
    fn enum_members_retain_live_payload_and_owner_slots_without_expanding_siblings() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let names = BTreeMap::new();
        let traits = BTreeMap::new();
        let mut inference = GenericInference::new(&schemes, &hir, &interfaces, &names,
            InferenceAnnotationInputs::default(), &[], &[], &traits, None, true, None, None);
        let payload = inference.variables.fresh();
        let mut deep = inference.variables.fresh();
        for _ in 0..16384 {
            deep = inference.variables.structure_node(InferenceConstructor::Array, &[deep]);
        }
        let body = inference.variables.structure_node(InferenceConstructor::Enum(Box::new([
            ("Deep".into(), true), ("Empty".into(), false), ("Value".into(), true),
        ])), &[deep, payload]);
        let owner = inference.variables.structure_node(InferenceConstructor::Declared {
            head: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 127), name: "Choice".into(),
        }, &[body]);
        let witness = inference.variables.structure_node(InferenceConstructor::TypeOf, &[owner]);
        let family = inference.variables.structure_node(InferenceConstructor::Function, &[payload, witness]);
        for root in [witness, family] {
            let (member, _) = inference.enum_member(&TypeDescriptor::Inference(root), "Value").unwrap().unwrap();
            assert_eq!(member, TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Inference(payload)], result: Box::new(TypeDescriptor::Inference(owner)),
            });
            let (empty, _) = inference.enum_member(&TypeDescriptor::Inference(root), "Empty").unwrap().unwrap();
            assert_eq!(empty, TypeDescriptor::Inference(owner));
        }
        inference.variables.set(payload, TypeDescriptor::Int);
        let (member, _) = inference.enum_member(&TypeDescriptor::Inference(witness), "Value").unwrap().unwrap();
        let TypeDescriptor::Function { parameters, .. } = member else { panic!("payload constructor"); };
        let InferenceView::Row(row) = inference.variables.view(&parameters[0]) else { panic!("solved payload"); };
        assert!(matches!(inference.variables.constructor(row), InferenceConstructor::Int));
        assert!(inference.variables.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn enum_member_queries_match_normalized_descriptor_semantics() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let names = BTreeMap::new();
        let traits = BTreeMap::new();
        let mut inference = GenericInference::new(&schemes, &hir, &interfaces, &names,
            InferenceAnnotationInputs::default(), &[], &[], &traits, None, true, None, None);
        let enumeration = TypeDescriptor::Enum(BTreeMap::from([
            ("Empty".into(), None), ("Value".into(), Some(Box::new(TypeDescriptor::Int))),
        ]));
        let declared = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 128),
            name: "Choice".into(), body: Arc::new(enumeration.clone()),
        });
        for owner in [enumeration, declared.clone(), unchecked_descriptor(declared.clone()),
            TypeDescriptor::PendingAlternatives(vec![declared, TypeDescriptor::Never]),
            TypeDescriptor::PendingAlternatives(vec![TypeDescriptor::Int, TypeDescriptor::String]),
            TypeDescriptor::Int]
        {
            let witness = TypeDescriptor::TypeOf(Box::new(owner));
            for descriptor in [witness.clone(), TypeDescriptor::PendingAlternatives(vec![witness.clone(), TypeDescriptor::Never]),
                TypeDescriptor::Function { parameters: vec![TypeDescriptor::Int], result: Box::new(witness) }] {
                let slot = inference.variables.structure_edge(descriptor.clone());
                for ty in [descriptor, TypeDescriptor::Inference(slot)] {
                    for tag in ["Empty", "Value", "Missing"] {
                        let expected = enum_member_type(&inference.normalize(&ty), tag);
                        let actual = inference.enum_member(&ty, tag).map(|member| member.map(|(ty, constructor)| (inference.normalize(&ty), constructor)));
                        assert_eq!(actual, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn declared_context_resolves_identity_arguments_without_expanding_its_body() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let names = BTreeMap::new();
        let traits = BTreeMap::new();
        let mut inference = GenericInference::new(&schemes, &hir, &interfaces, &names,
            InferenceAnnotationInputs::default(), &[], &[], &traits, None, true, None, None);
        let parameter = inference.variables.fresh();
        let mut child = inference.variables.fresh();
        for _ in 0..16384 {
            child = inference.variables.structure_node(InferenceConstructor::Array, &[child]);
        }
        let deep_row = inference.variables.known(child).unwrap();
        let body = inference.variables.structure_node(InferenceConstructor::Struct(Box::new(["deep".into()])), &[child]);
        let root = inference.variables.structure_node(InferenceConstructor::Declared {
            head: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 95),
            name: "Container".into(),
        }, &[parameter, body]);
        assert!(inference.declared_context(&TypeDescriptor::Inference(child)).is_none());
        assert!(inference.declared_constructor(&TypeDescriptor::Inference(child)).is_none());
        let identity = inference.declared_identity(&TypeDescriptor::Inference(root)).unwrap();
        assert_eq!(identity.arguments(), &[TypeDescriptor::Inference(parameter)]);
        assert_eq!(inference.declared_constructor(&TypeDescriptor::Inference(root)), Some(identity.constructor()));
        let concrete = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: identity.reapply(&[TypeDescriptor::Int]), name: "Container".into(), body: Arc::new(TypeDescriptor::Never),
        });
        assert_eq!(inference.matching_nominal_arguments(&TypeDescriptor::Inference(root), &concrete),
            Some((vec![TypeDescriptor::Inference(parameter)], vec![TypeDescriptor::Int])));
        assert!(inference.variables.descriptor_views.iter().all(|view| view.get().is_none()));
        let first = inference.declared_context(&TypeDescriptor::Inference(root)).unwrap();
        assert_eq!(first.id.arguments(), &[TypeDescriptor::Inference(parameter)]);
        inference.variables.set(parameter, TypeDescriptor::Int);
        let second = inference.declared_context(&TypeDescriptor::Inference(root)).unwrap();
        assert_eq!(second.id.arguments(), &[TypeDescriptor::Int]);
        assert!(Arc::ptr_eq(&first.body, &second.body));
        let TypeDescriptor::Struct(fields) = second.body.as_ref() else { panic!("shallow record body"); };
        assert_eq!(fields["deep"], TypeDescriptor::Inference(child));
        assert!(inference.variables.descriptor_views[deep_row.0 as usize].get().is_none());
    }

    #[test]
    fn field_projection_shares_slots_and_does_not_expand_recursive_siblings() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let names = BTreeMap::new();
        let traits = BTreeMap::new();
        let mut inference = GenericInference::new(&schemes, &hir, &interfaces, &names,
            InferenceAnnotationInputs::default(), &[], &[], &traits, None, true, None, None);
        let value = inference.variables.fresh();
        let nominal = inference.variables.fresh();
        let body = inference.variables.structure_node(InferenceConstructor::Struct(Box::new([
            "next".into(), "value".into(),
        ])), &[nominal, value]);
        let identity = crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 96);
        let row = inference.variables.push_type(InferenceConstructor::Declared {
            head: identity.clone(), name: "Recursive".into(),
        }, &[body]);
        inference.variables.initialize_known(nominal, row);
        let receiver = TypeDescriptor::Inference(nominal);
        let projected = inference.project_field(&receiver, "value").unwrap();
        assert_eq!(projected, TypeDescriptor::Inference(value));
        inference.variables.set(value, TypeDescriptor::Int);
        assert_eq!(inference.project_field(&receiver, "value").unwrap(), projected);
        assert_eq!(inference.project_field(&receiver, "next").unwrap(), receiver);
        assert_eq!(inference.project_field(&receiver, "missing").unwrap_err(), "Struct has no field \"missing\"");
        inference.declared_bodies.to_mut().insert(identity.clone(), Arc::new(TypeDescriptor::Inference(body)));
        let stub = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: identity, name: "Recursive".into(), body: Arc::new(TypeDescriptor::Never),
        });
        assert_eq!(inference.project_field(&stub, "value").unwrap(), projected);
        assert!(inference.variables.descriptor_views.iter().all(|view| view.get().is_none()));
        let unknown = TypeDescriptor::Inference(inference.variables.fresh());
        let first = inference.project_field(&unknown, "field").unwrap();
        assert_eq!(inference.project_field(&unknown, "field").unwrap(), first);
    }

    #[test]
    fn collection_shapes_preserve_deep_child_slots_without_descriptor_views() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let names = BTreeMap::new();
        let traits = BTreeMap::new();
        let mut inference = GenericInference::new(&schemes, &hir, &interfaces, &names,
            InferenceAnnotationInputs::default(), &[], &[], &traits, None, true, None, None);
        let leaf = inference.variables.fresh();
        let mut child = leaf;
        for _ in 0..16384 {
            child = inference.variables.structure_node(InferenceConstructor::Array, &[child]);
        }
        let root = inference.variables.structure_node(InferenceConstructor::Struct(Box::new(["deep".into()])), &[child]);
        let Some(TypeDescriptor::Struct(fields)) = inference.collection_shape(&TypeDescriptor::Inference(root)) else {
            panic!("record shape");
        };
        assert_eq!(fields["deep"], TypeDescriptor::Inference(child));
        assert_eq!(inference.nominal_refinement_shape(&TypeDescriptor::Inference(root)), TypeDescriptor::Struct(fields.clone()));
        assert!(inference.contains_owned_unknown(&TypeDescriptor::Inference(root), leaf.0));
        assert!(!inference.contains_owned_unknown(&TypeDescriptor::Inference(root), leaf.0 + 1));
        inference.variables.set(leaf, TypeDescriptor::Int);
        assert!(!inference.contains_owned_unknown(&TypeDescriptor::Inference(root), 0));
        assert_eq!(inference.collection_shape(&TypeDescriptor::Inference(root)), Some(TypeDescriptor::Struct(fields)));
        assert!(inference.variables.descriptor_views.iter().all(|view| view.get().is_none()));
        let unresolved = TypeDescriptor::PendingAlternatives(vec![TypeDescriptor::Int, TypeDescriptor::String]);
        assert!(inference.collection_shape(&unresolved).is_none());
        let alternatives = inference.variables.structure_edge(unresolved.clone());
        let array = inference.variables.structure_node(InferenceConstructor::Array, &[alternatives]);
        assert_eq!(inference.collection_shape(&TypeDescriptor::Inference(array)),
            Some(TypeDescriptor::Array(Box::new(unresolved))));
    }

    #[test]
    fn graph_predicates_match_normalization_before_and_after_solving() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let names = BTreeMap::from([
            ("Alias".into(), TypeDescriptor::Int),
            ("Alternatives".into(), TypeDescriptor::PendingAlternatives(vec![TypeDescriptor::Int, TypeDescriptor::Never])),
            ("Cycle".into(), TypeDescriptor::Named("Cycle".into())),
            ("PendingCycle".into(), TypeDescriptor::PendingAlternatives(vec![TypeDescriptor::Named("PendingCycle".into()), TypeDescriptor::Never])),
        ]);
        let annotations = InferenceAnnotationInputs::default();
        let traits = BTreeMap::new();
        let mut inference = GenericInference::new(
            &schemes,
            &hir,
            &interfaces,
            &names,
            annotations,
            &[],
            &[],
            &traits,
            None,
            true,
            None,
            None,
        );
        let unknown = inference.variables.fresh();
        let parameter = TypeDescriptor::Inference(unknown);
        let nominal = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 97),
            name: "BodyOnly".into(),
            body: Arc::new(TypeDescriptor::Array(Box::new(parameter.clone()))),
        });
        let descriptors = vec![
            TypeDescriptor::Never,
            TypeDescriptor::Bound(TypeParameterId(0)),
            TypeDescriptor::Named("UnresolvedName".into()),
            TypeDescriptor::Named("Alias".into()),
            TypeDescriptor::Named("Alternatives".into()),
            TypeDescriptor::Named("Cycle".into()),
            TypeDescriptor::Named("PendingCycle".into()),
            TypeDescriptor::PendingAlternatives(vec![nominal.clone(), TypeDescriptor::Never]),
            nominal,
            TypeDescriptor::TypeOf(Box::new(parameter.clone())),
            TypeDescriptor::Function {
                parameters: vec![parameter.clone()],
                result: Box::new(TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Int))),
            },
            TypeDescriptor::Tuple(vec![TypeDescriptor::Int, parameter.clone()]),
            TypeDescriptor::Struct(BTreeMap::from([("value".into(), parameter.clone())])),
            TypeDescriptor::Enum(BTreeMap::from([(
                "Some".into(),
                Some(Box::new(parameter.clone())),
            )])),
            unchecked_descriptor(parameter.clone()),
            TypeDescriptor::PendingAlternatives(vec![TypeDescriptor::Int, TypeDescriptor::Int]),
        ];
        let slots = descriptors
            .iter()
            .cloned()
            .map(|ty| inference.variables.structure_edge(ty))
            .collect::<Vec<_>>();
        for solved in [false, true] {
            if solved {
                inference.variables.set(unknown, TypeDescriptor::String);
            }
            for ty in descriptors
                .iter()
                .cloned()
                .chain(slots.iter().copied().map(TypeDescriptor::Inference))
            {
                let normalized = inference.normalize(&ty);
                assert_eq!(inference.normalize(&inference.nominal_refinement_shape(&ty)),
                    inference.normalize(&inference.expose_named(&ty)), "refinement shape {ty:?}");
                assert_eq!(inference.is_type_of(&ty), matches!(normalized, TypeDescriptor::TypeOf(_)));
                for first in [0, unknown.0, unknown.0 + 1] {
                    assert_eq!(inference.contains_owned_unknown(&ty, first),
                        contains_inference_variable_at_or_after(&normalized, first), "{ty:?}");
                }
                assert_eq!(
                    inference.has_unresolved(&ty),
                    contains_type_variable(&normalized),
                    "{ty:?}"
                );
                assert_eq!(
                    inference.has_exposed_unresolved(&ty),
                    contains_exposed_type_variable(&normalized),
                    "{ty:?}"
                );
                assert_eq!(
                    inference.expects_type_value(&ty),
                    expects_type_value(&normalized),
                    "{ty:?}"
                );
            }
        }
    }

    #[test]
    fn deep_graph_predicates_do_not_create_descriptor_trees() {
        let mut arena = InferenceVariables::default();
        let unknown = arena.fresh();
        let mut root = unknown;
        for _ in 0..16_384 {
            root = arena.structure_node(InferenceConstructor::Function, &[unknown, root]);
        }
        let ty = TypeDescriptor::Inference(root);
        assert_eq!(arena.query_unresolved(&ty, false), Some(true));
        assert_eq!(arena.query_type_value(&ty), Some(false));
        arena.set(unknown, TypeDescriptor::Type);
        assert_eq!(arena.query_unresolved(&ty, false), Some(false));
        assert_eq!(arena.query_type_value(&ty), Some(true));
        assert!(
            arena
                .descriptor_views
                .iter()
                .all(|view| view.get().is_none())
        );
    }

    #[test]
    fn head_queries_follow_mutations_without_materializing_children() {
        let mut arena = InferenceVariables::default();
        let leaf = arena.fresh();
        let alias = arena.fresh();
        arena.set(alias, TypeDescriptor::Inference(leaf));
        assert!(matches!(arena.view(&TypeDescriptor::Inference(alias)),
            InferenceView::Unknown(root) if root == leaf));
        let mut child = leaf;
        for _ in 0..16_384 {
            child = arena.structure_node(InferenceConstructor::Array, &[child]);
        }
        assert!(!arena.may_be_unchecked(&TypeDescriptor::Inference(child)));
        arena.set(leaf, TypeDescriptor::Int);
        assert!(matches!(arena.view(&TypeDescriptor::Inference(alias)),
            InferenceView::Row(row) if matches!(arena.constructor(row), InferenceConstructor::Int)));
        arena.set(leaf, TypeDescriptor::String);
        assert!(matches!(arena.view(&TypeDescriptor::Inference(alias)),
            InferenceView::Row(row) if matches!(arena.constructor(row), InferenceConstructor::String)));
        arena.record_conflict(&TypeDescriptor::Inference(leaf), "late conflict");
        assert!(matches!(arena.view(&TypeDescriptor::Inference(child)),
            InferenceView::Conflicted(id) if arena.conflicts[id as usize].as_ref() == "late conflict"));
        assert!(
            arena
                .descriptor_views
                .iter()
                .all(|view| view.get().is_none())
        );
    }

    #[test]
    fn unchecked_guard_keeps_semantic_fallbacks() {
        let mut arena = InferenceVariables::default();
        let target = arena.structure_edge(TypeDescriptor::Int);
        let special = arena.structure_node(
            InferenceConstructor::Declared {
                head: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 2),
                name: "Unchecked".into(),
            },
            &[target, target],
        );
        let alternative =
            arena.structure_node(InferenceConstructor::PendingAlternatives, &[special]);
        assert!(arena.may_be_unchecked(&TypeDescriptor::Inference(special)));
        assert!(arena.may_be_unchecked(&TypeDescriptor::Inference(alternative)));
        assert!(arena.may_be_unchecked(&TypeDescriptor::Named("Alias".into())));
        assert!(!arena.may_be_unchecked(&TypeDescriptor::Inference(target)));
        assert!(
            arena
                .descriptor_views
                .iter()
                .all(|view| view.get().is_none())
        );
    }
}
