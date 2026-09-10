#[derive(Clone, Copy, Default)]
enum PublishedInferenceSlot {
    #[default]
    Unvisited,
    Visiting,
    Reserved(AnalysisTypeId),
    Complete(Result<AnalysisTypeId, PublicationFailure>),
}

#[derive(Clone, Copy, Default)]
struct PublicationFailure(u8);

fn publish_program_expressions(
    inference: &GenericInference<'_>,
    publication: &mut InferencePublication<'_>,
    types: &mut TypeGraph,
    sources: &SourceDatabase,
) -> Result<HashMap<crate::Location, AnalysisTypeId>, FrontendError> {
    let mut published = HashMap::with_capacity(inference.records.len());
    let mut expressions = inference.records.iter().collect::<Vec<_>>();
    expressions.sort_by_key(|(location, _)| location.range().start);
    for (&location, &slot) in expressions {
        let result = publication.publish(types, slot,
            |slot| inference.normalize(&TypeDescriptor::Inference(slot)));
        let failure = result.err().unwrap_or_default();
        if failure.contains(PublicationFailure::STANDALONE) {
            return Err(FrontendError::from_diagnostic(sources, Diagnostic::error(
                "standalone Atom/Tagged is not a public expression type; use an enum", location)));
        }
        if failure.contains(PublicationFailure::ALTERNATIVES) {
            let resolved = inference.normalize(&TypeDescriptor::Inference(slot));
            return Err(FrontendError::from_diagnostic(sources, Diagnostic::error(
                format!("no common type for {}; supply explicit context with .ty!(Ty) or @[Ty]", resolved.display_name()),
                location)));
        }
        if let Ok(id) = result { published.insert(location, id); }
    }
    Ok(published)
}

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
    validation: Vec<u8>,
    validation_parents: Vec<u32>,
    validation_edges: Vec<(usize, u32)>,
}

impl<'a> InferencePublication<'a> {
    fn publish_tool_root(
        &mut self,
        graph: &mut TypeGraph,
        slot: InferenceVariableId,
        normalize: impl Fn(InferenceVariableId) -> TypeDescriptor,
    ) -> ToolTypeRoot {
        if let Ok(id) = self.publish(graph, slot, &normalize) {
            return ToolTypeRoot::Graph(id);
        }
        let Some(ty) = self.variables.known(slot) else { return ToolTypeRoot::Unresolved; };
        match self.variables.constructor(ty) {
            InferenceConstructor::Function => {
                let arguments = self.variables.arguments(ty);
                let (&result, parameters) = arguments.split_last().expect("function result edge");
                let arity = parameters.len();
                let owner = self.publish(graph, result, &normalize).ok()
                    .filter(|id| matches!(graph.node(*id), TypeNode::Declared { .. }));
                ToolTypeRoot::OpenFunction { arity, owner }
            }
            InferenceConstructor::PendingAlternatives => ToolTypeRoot::open_shape(graph, &normalize(slot)),
            _ => ToolTypeRoot::Unresolved,
        }
    }

    fn new(variables: &'a InferenceVariables) -> Self {
        Self {
            variables,
            slots: vec![PublishedInferenceSlot::Unvisited; variables.nodes.len()],
            validation: vec![u8::MAX; variables.nodes.len()],
            validation_parents: vec![u32::MAX; variables.nodes.len()],
            validation_edges: Vec::new(),
        }
    }

    fn needs_normalization(constructor: &InferenceConstructor) -> bool {
        matches!(constructor, InferenceConstructor::PendingAlternatives)
            || matches!(constructor, InferenceConstructor::Declared { head, .. }
                if head.constructor() == unchecked_type_constructor())
    }

    fn validate(
        &mut self,
        root: InferenceVariableId,
        normalize: &impl Fn(InferenceVariableId) -> TypeDescriptor,
    ) -> PublicationFailure {
        let root = self.variables.root(root);
        if self.validation[root.0 as usize] != u8::MAX {
            return PublicationFailure(self.validation[root.0 as usize]);
        }
        let mut work = vec![root];
        let mut failures = Vec::new();
        while let Some(slot) = work.pop() {
            let slot = self.variables.root(slot);
            let index = slot.0 as usize;
            if self.validation[index] != u8::MAX { continue; }
            self.validation[index] = 0;
            let Some(ty) = self.variables.known(slot) else {
                self.validation[index] = PublicationFailure::UNRESOLVED;
                failures.push(index);
                continue;
            };
            let constructor = self.variables.constructor(ty);
            if Self::needs_normalization(constructor) {
                self.validation[index] = PublicationFailure::descriptor(&normalize(slot)).0;
                if self.validation[index] != 0 { failures.push(index); }
                continue;
            }
            let arguments = self.variables.arguments(ty);
            let mut failure = if matches!(constructor,
                InferenceConstructor::AtomValue | InferenceConstructor::Atom(_) | InferenceConstructor::Tagged(_))
            { PublicationFailure::STANDALONE } else { 0 };
            for argument in arguments {
                let child = self.variables.root(*argument).0 as usize;
                let edge = self.validation_edges.len() as u32;
                self.validation_edges.push((index, self.validation_parents[child]));
                self.validation_parents[child] = edge;
                if self.validation[child] == u8::MAX { work.push(*argument); }
                else { failure |= self.validation[child]; }
            }
            self.validation[index] = failure;
            if failure != 0 { failures.push(index); }
        }
        // A recursive edge is evidence sharing, not an unknown. Propagate
        // actual failures to a fixed point through the flat reverse edges.
        while let Some(child) = failures.pop() {
            let mut edge = self.validation_parents[child];
            while edge != u32::MAX {
                let (parent, next) = self.validation_edges[edge as usize];
                let previous = self.validation[parent];
                self.validation[parent] |= self.validation[child];
                if self.validation[parent] != previous { failures.push(parent); }
                edge = next;
            }
        }
        PublicationFailure(self.validation[root.0 as usize])
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
            if !finish && matches!(self.slots[index], PublishedInferenceSlot::Reserved(_)) { continue; }
            let Some(ty) = self.variables.known(slot) else {
                self.slots[index] = PublishedInferenceSlot::Complete(Err(PublicationFailure(PublicationFailure::UNRESOLVED)));
                continue;
            };
            let constructor = self.variables.constructor(ty);
            let arguments = self.variables.arguments(ty);
            if Self::needs_normalization(constructor) {
                let descriptor = normalize(slot);
                let failure = PublicationFailure::descriptor(&descriptor);
                self.slots[index] = PublishedInferenceSlot::Complete(if failure.0 == 0 {
                    Ok(graph.intern_descriptor(&descriptor))
                } else { Err(failure) });
                continue;
            }
            if let C::Declared { head, name } = constructor {
                if finish {
                    let PublishedInferenceSlot::Reserved(id) = self.slots[index] else { unreachable!() };
                    let body = self.variables.root(*arguments.last().expect("nominal body edge"));
                    let body = match self.slots[body.0 as usize] {
                        PublishedInferenceSlot::Complete(Ok(body)) | PublishedInferenceSlot::Reserved(body) => body,
                        _ => unreachable!("validated nominal body must publish"),
                    };
                    let TypeNode::Declared { body: target, .. } = &mut graph.nodes[id.index()] else { unreachable!() };
                    *target = body;
                    graph.record_interned_node(id);
                    self.slots[index] = PublishedInferenceSlot::Complete(Ok(id));
                    continue;
                }
                let failure = self.validate(slot, &normalize);
                if failure.0 != 0 {
                    self.slots[index] = PublishedInferenceSlot::Complete(Err(failure));
                    continue;
                }
                let (body, parameters) = arguments.split_last().expect("nominal body edge");
                // Only the public identity still needs argument descriptors;
                // the nominal body is published directly from its shared slot.
                let parameters = parameters.iter().map(|slot| normalize(*slot)).collect::<Vec<_>>();
                let identity = head.reapply(&parameters);
                if let Some(id) = graph.declared.get(&identity) {
                    self.slots[index] = PublishedInferenceSlot::Complete(Ok(*id));
                    continue;
                }
                let id = graph.push(TypeNode::Pending);
                graph.declared.insert(identity.clone(), id);
                graph.nodes[id.index()] = TypeNode::Declared { id: identity, name: name.clone(), body: id };
                self.slots[index] = PublishedInferenceSlot::Reserved(id);
                work.push((slot, true));
                work.push((*body, false));
                continue;
            }
            if !finish {
                if matches!(self.slots[index], PublishedInferenceSlot::Visiting) {
                    self.slots[index] = PublishedInferenceSlot::Reserved(graph.push(TypeNode::Pending));
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
                    PublishedInferenceSlot::Complete(Ok(id)) | PublishedInferenceSlot::Reserved(id) => children.push(id),
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
            let id = if let PublishedInferenceSlot::Reserved(id) = self.slots[index] {
                graph.nodes[id.index()] = node;
                graph.record_interned_node(id);
                id
            } else { graph.intern_node(node) };
            self.slots[index] = PublishedInferenceSlot::Complete(Ok(id));
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
    fn cyclic_slots_publish_from_structural_or_nominal_roots_and_propagate_errors() {
        for nominal_first in [false, true] {
            let mut variables = InferenceVariables::default();
            let nominal = variables.fresh();
            let body = variables.structure_node(InferenceConstructor::Array, &[nominal]);
            let constructor = InferenceConstructor::Declared {
                head: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 99),
                name: "Recursive".into(),
            };
            let ty = variables.push_type(constructor, &[body]);
            variables.initialize_known(nominal, ty);
            let mut graph = TypeGraph::default();
            let mut publication = InferencePublication::new(&variables);
            let root = if nominal_first { nominal } else { body };
            publication.publish(&mut graph, root, |_| panic!("must consume slots")).ok().unwrap();
            let nominal = publication.publish(&mut graph, nominal, |_| unreachable!()).ok().unwrap();
            let body = publication.publish(&mut graph, body, |_| unreachable!()).ok().unwrap();
            assert!(matches!(graph.node(nominal), TypeNode::Declared { body: id, .. } if *id == body));
            assert!(matches!(graph.node(body), TypeNode::Array(id) if *id == nominal));
            assert!(graph.nodes.iter().all(|node| !matches!(node, TypeNode::Pending)));
            assert!(variables.descriptor_views.iter().all(|view| view.get().is_none()));
        }

        let mut variables = InferenceVariables::default();
        let cycle = variables.fresh();
        let unknown = variables.fresh();
        let child = variables.structure_node(InferenceConstructor::Array, &[cycle]);
        let ty = variables.push_type(InferenceConstructor::Tuple, &[child, unknown]);
        variables.initialize_known(cycle, ty);
        let mut publication = InferencePublication::new(&variables);
        assert!(publication.validate(cycle, &|_| unreachable!()).contains(PublicationFailure::UNRESOLVED));
        // The failure must also reach the node visited before the unknown,
        // even when a subsequent query reads its cached result.
        assert!(publication.validate(child, &|_| unreachable!()).contains(PublicationFailure::UNRESOLVED));
    }

    #[test]
    fn deep_nominal_bodies_publish_without_normalization_or_descriptor_views() {
        let mut variables = InferenceVariables::default();
        let mut root = variables.structure_edge(TypeDescriptor::Int);
        for declaration in 100..16484 {
            root = variables.structure_node(InferenceConstructor::Declared {
                head: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, declaration),
                name: "Nested".into(),
            }, &[root]);
        }
        let mut graph = TypeGraph::default();
        let published = InferencePublication::new(&variables).publish(
            &mut graph, root, |_| panic!("concrete nominal publication must not normalize"),
        ).ok().unwrap();
        assert!(matches!(graph.node(published), TypeNode::Declared { .. }));
        assert_eq!(graph.nodes.len(), 16385);
        assert!(variables.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn nominal_reservation_precedes_recursive_stub_and_does_not_hide_invalid_bodies() {
        let mut variables = InferenceVariables::default();
        let head = crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 98);
        let constructor = InferenceConstructor::Declared { head: head.clone(), name: "Recursive".into() };
        let never = variables.structure_edge(TypeDescriptor::Never);
        let stub = variables.structure_node(constructor.clone(), &[never]);
        let body = variables.structure_node(InferenceConstructor::Struct(Box::new(["next".into()])), &[stub]);
        let root = variables.structure_node(constructor.clone(), &[body]);
        let unknown = variables.fresh();
        let invalid = variables.structure_node(constructor, &[unknown]);
        let mut graph = TypeGraph::default();
        let mut publication = InferencePublication::new(&variables);
        let published = publication.publish(&mut graph, root, |_| unreachable!()).ok().unwrap();
        let TypeNode::Declared { body, .. } = graph.node(published) else { panic!("nominal type") };
        let TypeNode::Struct(fields) = graph.node(*body) else { panic!("complete body must win over Never stub") };
        assert_eq!(fields["next"], published);
        assert_eq!(graph.declared[&head], published);
        assert!(publication.publish(&mut graph, invalid, |_| unreachable!()).is_err());
        assert!(variables.descriptor_views.iter().all(|view| view.get().is_none()));
    }

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
        let annotations = InferenceAnnotationInputs::default();
        let trait_ids = BTreeMap::new();
        let dyn_namespaces = HashSet::new();
        let mut inference = GenericInference::new(
            &schemes, &hir, &interfaces, &named_types, annotations,
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
