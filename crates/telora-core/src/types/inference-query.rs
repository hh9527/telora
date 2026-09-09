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
    fn graph_predicates_match_normalization_before_and_after_solving() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let names = BTreeMap::new();
        let annotations = HashMap::new();
        let traits = BTreeMap::new();
        let namespaces = HashSet::new();
        let mut inference = GenericInference::new(
            &schemes,
            &hir,
            &interfaces,
            &names,
            &annotations,
            &[],
            &[],
            &traits,
            None,
            &namespaces,
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
