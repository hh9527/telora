// A graph root is imported once per solver. Its children remain slot edges,
// including edges that cross a recursive nominal boundary.
#[cfg(test)]
mod graph_input_tests {
    use super::*;

    #[test]
    fn deep_annotation_aliases_are_compressed_without_descriptor_expansion() {
        let mut graph = TypeGraph::default();
        let item = graph.push(TypeNode::Int);
        let mut root = item;
        for _ in 0..16384 { root = graph.push(TypeNode::Ref(root)); }
        let mut variables = InferenceVariables::default();
        let mut slots = vec![None; graph.nodes.len()];
        let mut validated = vec![0; graph.nodes.len()];
        let imported = variables.import_graph_root(&graph, root, &mut slots, &mut validated).unwrap();
        assert_eq!(imported, slots[item.index()].unwrap());
        for slot in slots.iter().flatten() {
            assert_eq!(variables.root(*slot), imported);
            assert!(*slot == imported || matches!(variables.nodes[slot.0 as usize].get(),
                InferenceNode::ProxyTo(target) if target == imported));
        }
        assert!(variables.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn shared_annotation_roots_import_once_without_descriptor_views() {
        let mut graph = TypeGraph::default();
        let item = graph.push(TypeNode::Int);
        let root = graph.push(TypeNode::Tuple(vec![item, item]));
        let mut variables = InferenceVariables::default();
        let mut slots = vec![None; graph.nodes.len()];
        let mut validated = vec![0; graph.nodes.len()];
        let first = variables.import_graph_root(&graph, root, &mut slots, &mut validated).unwrap();
        let second = variables.import_graph_root(&graph, root, &mut slots, &mut validated).unwrap();
        assert_eq!(first, second);
        assert_eq!(variables.nodes.len(), 2);
        let arguments = variables.arguments(variables.known(first).unwrap());
        assert_eq!(arguments[0], arguments[1]);
        assert!(variables.descriptor_views.iter().all(|view| view.get().is_none()));
    }

    #[test]
    fn imported_nominal_recursion_can_be_canonicalized_but_structural_cycles_are_rejected() {
        let mut graph = TypeGraph::default();
        let root = graph.push(TypeNode::Pending);
        let body = graph.push(TypeNode::Array(root));
        graph.nodes[root.index()] = TypeNode::Declared {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 98),
            name: "Recursive".into(), body,
        };
        let mut variables = InferenceVariables::default();
        let mut slots = vec![None; graph.nodes.len()];
        let mut validated = vec![0; graph.nodes.len()];
        variables.import_graph_root(&graph, root, &mut slots, &mut validated).unwrap();
        variables.canonicalize_slots();
        assert!(variables.descriptor_views.iter().all(|view| view.get().is_none()));

        graph.nodes[root.index()] = TypeNode::Array(body);
        let error = InferenceVariables::default().import_graph_root(
            &graph, root, &mut vec![None; graph.nodes.len()], &mut vec![0; graph.nodes.len()],
        ).unwrap_err();
        assert!(error.contains("no nominal identity"));
    }
}

#[derive(Default)]
struct InferenceAnnotationInputs {
    variables: InferenceVariables,
    types: HashMap<crate::Location, TypeDescriptor>,
    declared_bodies: HashMap<crate::value::DeclaredTypeId, InferenceVariableId>,
}

impl InferenceAnnotationInputs {
    fn from_graph(
        graph: &TypeGraph,
        roots: &HashMap<crate::Location, AnalysisTypeId>,
        sources: &SourceDatabase,
    ) -> Result<Self, FrontendError> {
        let mut inputs = Self::default();
        if roots.is_empty() { return Ok(inputs); }
        let mut slots = vec![None; graph.nodes.len()];
        let mut validated = vec![0u8; graph.nodes.len()];
        let mut ordered = roots.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|(location, _)| **location);
        for (&location, &root) in ordered {
            let slot = inputs.variables.import_graph_root(graph, root, &mut slots, &mut validated)
                .map_err(|message| FrontendError::from_diagnostic(sources, Diagnostic::error(message, location)))?;
            inputs.types.insert(location, TypeDescriptor::Inference(slot));
        }
        for (index, slot) in slots.iter().enumerate() {
            if slot.is_some()
                && let TypeNode::Declared { id, body, .. } = &graph.nodes[index]
            {
                inputs.declared_bodies.insert(id.clone(), slots[body.index()].unwrap());
            }
        }
        Ok(inputs)
    }
}

fn visit_graph_input_children(node: &TypeNode, mut visit: impl FnMut(AnalysisTypeId)) {
    match node {
        TypeNode::Ref(id) | TypeNode::TypeOf(id) | TypeNode::Array(id)
        | TypeNode::Dict(id) | TypeNode::Newtype(id) => visit(*id),
        TypeNode::Declared { body, .. } => visit(*body),
        TypeNode::Tagged { payload, .. } => visit(*payload),
        TypeNode::Tuple(items) | TypeNode::PendingAlternatives(items) => items.iter().copied().for_each(visit),
        TypeNode::Struct(fields) => fields.values().copied().for_each(visit),
        TypeNode::Enum(variants) => variants.values().flatten().copied().for_each(visit),
        TypeNode::Function { parameters, result } => parameters.iter().copied().chain([*result]).for_each(visit),
        _ => {},
    }
}

impl InferenceVariables {
    fn import_graph_root(
        &mut self,
        graph: &TypeGraph,
        root: AnalysisTypeId,
        slots: &mut [Option<InferenceVariableId>],
        validated: &mut [u8],
    ) -> Result<InferenceVariableId, String> {
        if let Some(slot) = slots[root.index()] { return Ok(self.root(slot)); }
        let mut pending = vec![root];
        let mut order = Vec::new();
        while let Some(id) = pending.pop() {
            if slots[id.index()].is_some() { continue; }
            if matches!(graph.node(id), TypeNode::Pending) {
                return Err("type graph contains an open node".into());
            }
            slots[id.index()] = Some(self.fresh());
            order.push(id);
            visit_graph_input_children(graph.node(id), |child| pending.push(child));
        }
        // Every structural cycle must cross a nominal node. Check the graph
        // with nominal outgoing edges removed, including bodies as fresh roots.
        let mut stack = Vec::new();
        for &start in &order {
            if validated[start.index()] == 2 { continue; }
            stack.push((start, false));
            while let Some((id, finish)) = stack.pop() {
                if finish { validated[id.index()] = 2; continue; }
                match validated[id.index()] {
                    2 => continue,
                    1 => return Err("recursive structural type has no nominal identity".into()),
                    _ => {}
                }
                validated[id.index()] = 1;
                stack.push((id, true));
                if !matches!(graph.node(id), TypeNode::Declared { .. }) {
                    visit_graph_input_children(graph.node(id), |child| stack.push((child, false)));
                }
            }
        }
        // Resolve graph aliases before adding dependency edges to solver slots.
        for &id in &order {
            let TypeNode::Ref(target) = graph.node(id) else { continue; };
            let source = slots[id.index()].unwrap();
            let target = slots[target.index()].unwrap();
            self.nodes[source.0 as usize].set(InferenceNode::ProxyTo(target));
        }
        // Compress aliases only after every edge exists; a long alias chain
        // is then resolved once instead of walking every suffix separately.
        for &id in &order {
            if matches!(graph.node(id), TypeNode::Ref(_)) {
                self.root(slots[id.index()].unwrap());
            }
        }
        let mut arguments = Vec::new();
        for &id in order.iter().rev() {
            use InferenceConstructor as C;
            let node = graph.node(id);
            if matches!(node, TypeNode::Ref(_)) { continue; }
            arguments.clear();
            visit_graph_input_children(node, |child| arguments.push(slots[child.index()].unwrap()));
            let constructor = match node {
                TypeNode::Pending | TypeNode::Ref(_) => unreachable!("validated graph"),
                TypeNode::Bound(id) => C::Bound(*id),
                TypeNode::Named(name) => C::Named(name.clone()),
                TypeNode::Declared { id, name, .. } => {
                    let body = arguments.pop().unwrap();
                    arguments.extend(id.arguments().iter().map(|argument| self.structure_edge(argument.clone())));
                    arguments.push(body);
                    C::Declared { head: id.reapply(&[]), name: name.clone() }
                }
                TypeNode::Never => C::Never,
                TypeNode::Type => C::Type,
                TypeNode::Dyn => C::Dyn,
                TypeNode::Int => C::Int,
                TypeNode::Float => C::Float,
                TypeNode::String => C::String,
                TypeNode::Bytes => C::Bytes,
                TypeNode::AtomValue => C::AtomValue,
                TypeNode::Opaque(native) => C::Opaque(native.clone()),
                TypeNode::Atom(atom) => C::Atom(atom.clone()),
                TypeNode::TypeOf(_) => C::TypeOf,
                TypeNode::Array(_) => C::Array,
                TypeNode::Dict(_) => C::Dict,
                TypeNode::Newtype(_) => C::Newtype,
                TypeNode::Tagged { tag, .. } => C::Tagged(tag.clone()),
                TypeNode::Tuple(_) => C::Tuple,
                TypeNode::PendingAlternatives(_) => C::PendingAlternatives,
                TypeNode::Struct(fields) => C::Struct(fields.keys().cloned().collect()),
                TypeNode::Enum(variants) => C::Enum(variants.iter().map(|(name, value)| (name.clone(), value.is_some())).collect()),
                TypeNode::Function { .. } => C::Function,
            };
            let ty = self.push_type(constructor, &arguments);
            self.initialize_known(slots[id.index()].unwrap(), ty);
        }
        Ok(self.root(slots[root.index()].unwrap()))
    }
}
