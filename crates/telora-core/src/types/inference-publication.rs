#[derive(Clone, Copy, Default)]
enum PublishedInferenceSlot {
    #[default]
    Unvisited,
    Visiting,
    Complete(Result<AnalysisTypeId, PublicationFailure>),
}

#[derive(Clone, Copy, Default)]
struct PublicationFailure(u8);

impl PublicationFailure {
    const UNRESOLVED: u8 = 1;
    const STANDALONE: u8 = 2;
    const ALTERNATIVES: u8 = 4;

    fn contains(self, flag: u8) -> bool { self.0 & flag != 0 }

    fn descriptor(ty: &TypeDescriptor) -> Self {
        Self(u8::from(contains_type_variable(ty)) * Self::UNRESOLVED
            | u8::from(contains_standalone_sum(ty)) * Self::STANDALONE
            | u8::from(contains_pending_alternatives(ty)) * Self::ALTERNATIVES)
    }
}

// One publication session owns the cache: neither the solver nor graph may be
// replaced or mutated by inference while these final IDs are being reused.
struct InferencePublication<'a> {
    variables: &'a InferenceVariables,
    slots: Vec<PublishedInferenceSlot>,
}

impl<'a> InferencePublication<'a> {
    fn new(variables: &'a InferenceVariables) -> Self {
        Self { variables, slots: vec![PublishedInferenceSlot::Unvisited; variables.nodes.len()] }
    }

    fn publish(
        &mut self,
        graph: &mut TypeGraph,
        root: InferenceVariableId,
        normalize: impl Fn(InferenceVariableId) -> TypeDescriptor,
    ) -> Result<AnalysisTypeId, PublicationFailure> {
        use InferenceConstructor as C;
        let root = self.variables.root(root);
        if let PublishedInferenceSlot::Complete(result) = self.slots[root.0 as usize] {
            return result;
        }
        let mut work = vec![(root, false)];
        while let Some((slot, finish)) = work.pop() {
            let slot = self.variables.root(slot);
            let index = slot.0 as usize;
            if matches!(self.slots[index], PublishedInferenceSlot::Complete(_)) { continue; }
            let Some(ty) = self.variables.known(slot) else {
                self.slots[index] = PublishedInferenceSlot::Complete(Err(PublicationFailure(PublicationFailure::UNRESOLVED)));
                continue;
            };
            let constructor = self.variables.constructor(ty);
            let arguments = self.variables.arguments(ty);
            // Nominal publication reserves the identity before its recursive
            // body. Keep that ordering, and Unchecked/alternative normalization,
            // at the compatibility boundary until those rules are slot-native.
            if matches!(constructor, C::Declared { .. } | C::PendingAlternatives) {
                let descriptor = normalize(slot);
                let failure = PublicationFailure::descriptor(&descriptor);
                self.slots[index] = PublishedInferenceSlot::Complete(if failure.0 == 0 {
                    Ok(graph.intern_descriptor(&descriptor))
                } else { Err(failure) });
                continue;
            }
            if !finish {
                if matches!(self.slots[index], PublishedInferenceSlot::Visiting) {
                    self.slots[index] = PublishedInferenceSlot::Complete(Err(PublicationFailure(PublicationFailure::UNRESOLVED)));
                    continue;
                }
                self.slots[index] = PublishedInferenceSlot::Visiting;
                work.push((slot, true));
                work.extend(arguments.iter().rev().map(|argument| (*argument, false)));
                continue;
            }
            let mut failure = PublicationFailure::default();
            let mut children = Vec::with_capacity(arguments.len());
            for argument in arguments {
                match self.slots[self.variables.root(*argument).0 as usize] {
                    PublishedInferenceSlot::Complete(Ok(id)) => children.push(id),
                    PublishedInferenceSlot::Complete(Err(error)) => failure.0 |= error.0,
                    _ => failure.0 |= PublicationFailure::UNRESOLVED,
                }
            }
            if matches!(constructor, C::AtomValue | C::Atom(_) | C::Tagged(_)) {
                failure.0 |= PublicationFailure::STANDALONE;
            }
            if failure.0 != 0 {
                self.slots[index] = PublishedInferenceSlot::Complete(Err(failure));
                continue;
            }
            let node = match constructor {
                C::Bound(id) => TypeNode::Bound(*id),
                C::Named(name) => graph.names.get(name).copied()
                    .map_or_else(|| TypeNode::Named(name.clone()), TypeNode::Ref),
                C::Never => TypeNode::Never,
                C::Type => TypeNode::Type,
                C::Dyn => TypeNode::Dyn,
                C::Int => TypeNode::Int,
                C::Float => TypeNode::Float,
                C::String => TypeNode::String,
                C::Bytes => TypeNode::Bytes,
                C::Opaque(native) => TypeNode::Opaque(native.clone()),
                C::TypeOf => TypeNode::TypeOf(children[0]),
                C::Array => TypeNode::Array(children[0]),
                C::Dict => TypeNode::Dict(children[0]),
                C::Newtype => TypeNode::Newtype(children[0]),
                C::Tuple => TypeNode::Tuple(children),
                C::Struct(names) => TypeNode::Struct(names.iter().cloned().zip(children).collect()),
                C::Enum(variants) => {
                    let mut children = children.into_iter();
                    TypeNode::Enum(variants.iter().map(|(name, payload)|
                        (name.clone(), payload.then(|| children.next().unwrap()))).collect())
                }
                C::Function => {
                    let result = children.pop().expect("function row has a result edge");
                    TypeNode::Function { parameters: children, result }
                }
                C::Declared { .. } | C::PendingAlternatives | C::AtomValue | C::Atom(_) | C::Tagged(_) => unreachable!(),
            };
            self.slots[index] = PublishedInferenceSlot::Complete(Ok(graph.intern_node(node)));
        }
        match self.slots[root.0 as usize] {
            PublishedInferenceSlot::Complete(result) => result,
            _ => unreachable!("publication completes the requested root"),
        }
    }
}

#[cfg(test)]
mod inference_publication_tests {
    use super::*;

    #[test]
    fn deep_shared_structures_publish_without_descriptor_views() {
        let mut variables = InferenceVariables::default();
        let item = variables.fresh();
        let mut root = item;
        for _ in 0..16384 {
            root = variables.structure_node(InferenceConstructor::Array, &[root]);
        }
        variables.set(item, TypeDescriptor::Int);
        let mut graph = TypeGraph::default();
        let mut publication = InferencePublication::new(&variables);
        let published = publication.publish(&mut graph, root, |_| panic!("structural publication must not normalize")).ok().unwrap();
        assert!(matches!(graph.node(published), TypeNode::Array(_)));
        assert_eq!(graph.nodes.len(), 16385);
        assert_eq!(publication.publish(&mut graph, root, |_| unreachable!()).ok(), Some(published));
        assert_eq!(graph.nodes.len(), 16385);
        assert!(variables.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn publication_preserves_all_failure_categories_and_bound_parameters() {
        let mut variables = InferenceVariables::default();
        let unknown = variables.fresh();
        let atom = variables.structure_edge(TypeDescriptor::AtomValue);
        let tuple = variables.structure_node(InferenceConstructor::Tuple, &[unknown, atom]);
        let bound = variables.structure_edge(TypeDescriptor::Bound(TypeParameterId(4)));
        let conflict = variables.fresh();
        variables.record_conflict(&TypeDescriptor::Inference(conflict), "conflict");
        let mut graph = TypeGraph::default();
        let mut publication = InferencePublication::new(&variables);
        let error = publication.publish(&mut graph, tuple, |_| unreachable!()).err().unwrap();
        assert!(error.contains(PublicationFailure::UNRESOLVED));
        assert!(error.contains(PublicationFailure::STANDALONE));
        assert!(publication.publish(&mut graph, conflict, |_| unreachable!()).is_err());
        let id = publication.publish(&mut graph, bound, |_| unreachable!()).ok().unwrap();
        assert!(matches!(graph.node(id), TypeNode::Bound(TypeParameterId(4))));
    }

    #[test]
    fn publication_matches_normalization_at_nominal_and_alternative_boundaries() {
        let schemes = HashMap::new();
        let hir = HirProgram::default();
        let interfaces = BTreeMap::new();
        let named_types = BTreeMap::new();
        let annotations = HashMap::new();
        let trait_ids = BTreeMap::new();
        let dyn_namespaces = HashSet::new();
        let mut inference = GenericInference::new(
            &schemes, &hir, &interfaces, &named_types, &annotations,
            &[], &[], &trait_ids, None, &dyn_namespaces, true, None, None,
        );
        let id = crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 97);
        let recursive = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: id.clone(), name: "Recursive".into(),
            body: Arc::new(TypeDescriptor::Struct(BTreeMap::from([(
                "next".into(), TypeDescriptor::Declared(DeclaredTypeDescriptor {
                    id, name: "Recursive".into(), body: Arc::new(TypeDescriptor::Never),
                }),
            )]))),
        });
        let item = inference.variables.fresh();
        let alternatives = inference.variables.structure_node(InferenceConstructor::PendingAlternatives, &[item, item]);
        inference.variables.set(item, TypeDescriptor::Int);
        let descriptors = [
            recursive.clone(),
            unchecked_descriptor(unchecked_descriptor(recursive)),
            TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Dict(Box::new(TypeDescriptor::String))],
                result: Box::new(TypeDescriptor::Enum(BTreeMap::from([
                    ("Empty".into(), None),
                    ("Value".into(), Some(Box::new(TypeDescriptor::Newtype(Box::new(TypeDescriptor::Int))))),
                ]))),
            },
        ];
        let mut roots = descriptors.into_iter().map(|ty| inference.variables.structure_edge(ty)).collect::<Vec<_>>();
        roots.push(alternatives);
        for root in roots {
            let resolved = inference.normalize(&TypeDescriptor::Inference(root));
            let mut expected = TypeGraph::default();
            let expected_id = expected.intern_resolved_descriptor(&resolved);
            let mut actual = TypeGraph::default();
            let actual_id = InferencePublication::new(&inference.variables).publish(
                &mut actual, root, |slot| inference.normalize(&TypeDescriptor::Inference(slot)),
            ).ok();
            assert_eq!(actual_id.is_some(), expected_id.is_some());
            if let (Some(actual_id), Some(expected_id)) = (actual_id, expected_id) {
                assert_eq!(actual.descriptor(actual_id), expected.descriptor(expected_id));
                if let TypeNode::Declared { body, .. } = actual.node(actual_id) {
                    assert!(!matches!(actual.node(*body), TypeNode::Never));
                }
            }
        }
    }
}
