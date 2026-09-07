#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InferenceTypeId(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InferenceNode {
    Unknown,
    ProxyTo(InferenceVariableId),
    Known(InferenceTypeId),
    Conflicted,
}

#[cfg(test)]
mod inference_variable_tests {
    use super::*;

    #[test]
    fn proxy_nodes_are_compact_and_need_no_drop() {
        assert_eq!(std::mem::size_of::<InferenceNode>(), 8);
        assert_eq!(std::mem::size_of::<std::cell::Cell<InferenceNode>>(), 8);
        assert!(!std::mem::needs_drop::<InferenceNode>());
    }

    #[test]
    fn long_proxy_chain_compresses_and_observes_late_binding() {
        let mut variables = InferenceVariables::default();
        let ids = (0..65536).map(|_| variables.fresh()).collect::<Vec<_>>();
        for pair in ids.windows(2) {
            variables.set(pair[0], TypeDescriptor::Inference(pair[1]));
        }
        let last = *ids.last().unwrap();
        assert_eq!(variables.root(ids[0]), last);
        for id in &ids[..ids.len() - 1] {
            assert_eq!(variables.nodes[id.0 as usize].get(), InferenceNode::ProxyTo(last));
        }
        variables.set(last, TypeDescriptor::Int);
        assert!(matches!(variables.nodes[last.0 as usize].get(), InferenceNode::Known(_)));
        for id in ids {
            assert_eq!(variables.binding(id), Some(&TypeDescriptor::Int));
        }
    }

    #[test]
    fn known_structures_keep_unresolved_edges_and_observe_late_solutions() {
        let mut variables = InferenceVariables::default();
        let item = variables.fresh();
        let alias = variables.fresh();
        let array = variables.fresh();
        let nested = variables.fresh();
        variables.set(array, TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(item))));
        variables.set(nested, TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(array))));
        let InferenceNode::Known(array_id) = variables.nodes[array.0 as usize].get() else {
            panic!("array structure must be known before its element");
        };
        let InferenceNode::Known(nested_id) = variables.nodes[nested.0 as usize].get() else {
            panic!("nested structure must be known before its element");
        };
        assert_eq!(variables.arguments(array_id), &[item]);
        assert_eq!(variables.arguments(nested_id), &[array]);
        variables.set(item, TypeDescriptor::Inference(alias));
        variables.set(alias, TypeDescriptor::Int);
        assert_eq!(variables.nodes[array.0 as usize].get(), InferenceNode::Known(array_id));
        assert_eq!(variables.nodes[nested.0 as usize].get(), InferenceNode::Known(nested_id));
        assert_eq!(variables.binding(item), Some(&TypeDescriptor::Int));
        let same = variables.fresh();
        variables.set(same, TypeDescriptor::Array(Box::new(TypeDescriptor::Int)));
        let independent = variables.nodes[same.0 as usize].get();
        variables.record_conflict(&TypeDescriptor::Inference(item), "incompatible element");
        assert_eq!(variables.nodes[array.0 as usize].get(), InferenceNode::Conflicted);
        assert_eq!(variables.nodes[nested.0 as usize].get(), InferenceNode::Conflicted);
        assert!(variables.binding(nested).is_none());
        assert_eq!(variables.nodes[same.0 as usize].get(), independent);
    }

    #[test]
    fn normalization_coalesces_outer_structures_after_argument_equality() {
        let mut variables = InferenceVariables::default();
        let t1 = variables.fresh();
        let t2 = variables.fresh();
        let a1 = variables.fresh();
        let a2 = variables.fresh();
        let outer1 = variables.fresh();
        let outer2 = variables.fresh();
        variables.set(a1, TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(t1))));
        variables.set(a2, TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(t2))));
        variables.set(outer1, TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(a1))));
        variables.set(outer2, TypeDescriptor::Array(Box::new(TypeDescriptor::Inference(a2))));
        assert_eq!(variables.canonicalize_slots(), 0);
        variables.set(t2, TypeDescriptor::Inference(t1));
        assert_eq!(variables.canonicalize_slots(), 2);
        assert_eq!(variables.root(a2), a1);
        assert_eq!(variables.root(outer2), outer1);
        assert_eq!(variables.canonicalize_slots(), 0);
        assert_eq!(variables.nodes[t1.0 as usize].get(), InferenceNode::Unknown);
    }
}

#[derive(Default)]
struct InferenceVariables {
    nodes: Vec<std::cell::Cell<InferenceNode>>,
    types: Vec<InferenceType>,
    arguments: Vec<InferenceVariableId>,
    constructors: Vec<Arc<InferenceConstructor>>,
    constructor_ids: HashMap<Arc<InferenceConstructor>, InferenceConstructorId>,
    descriptor_views: Vec<std::cell::OnceCell<Arc<TypeDescriptor>>>,
    descriptor_view_ids: std::cell::RefCell<HashMap<*const TypeDescriptor, InferenceTypeId>>,
    normalized_bodies: Vec<std::cell::RefCell<Option<(u64, Arc<TypeDescriptor>)>>>,
    revision: u64,
    imported_bodies: HashMap<*const TypeDescriptor, (Arc<TypeDescriptor>, InferenceVariableId)>,
    conflicts: Vec<Option<Arc<str>>>,
    dependents: Vec<Vec<InferenceVariableId>>,
}

// Borrow authored structures; only resolved variable bindings need shared ownership.
enum InferenceHead<'a> {
    Borrowed(&'a TypeDescriptor),
    Shared(Arc<TypeDescriptor>),
    Unknown(TypeDescriptor),
}

impl std::ops::Deref for InferenceHead<'_> {
    type Target = TypeDescriptor;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(ty) => ty,
            Self::Shared(ty) => ty,
            Self::Unknown(ty) => ty,
        }
    }
}

impl InferenceVariables {
    fn next_id(&self) -> u32 {
        u32::try_from(self.nodes.len()).expect("inference variable capacity exceeded")
    }

    fn fresh(&mut self) -> InferenceVariableId {
        let id = InferenceVariableId(u32::try_from(self.nodes.len()).expect("inference variable capacity exceeded"));
        self.nodes.push(std::cell::Cell::new(InferenceNode::Unknown));
        self.conflicts.push(None);
        self.dependents.push(Vec::new());
        id
    }

    fn root(&self, variable: InferenceVariableId) -> InferenceVariableId {
        let mut root = variable;
        while let InferenceNode::ProxyTo(target) = self.nodes[root.0 as usize].get() {
            root = target;
        }
        let mut current = variable;
        while let InferenceNode::ProxyTo(target) = self.nodes[current.0 as usize].get() {
            self.nodes[current.0 as usize].set(InferenceNode::ProxyTo(root));
            current = target;
        }
        root
    }

    fn binding(&self, variable: InferenceVariableId) -> Option<&TypeDescriptor> {
        let root = self.root(variable);
        match self.nodes[root.0 as usize].get() {
            InferenceNode::Known(id) => Some(self.descriptor_view(id)),
            _ => None,
        }
    }

    fn ensure_consistent(&self, ty: &TypeDescriptor) -> Result<(), String> {
        if let TypeDescriptor::Inference(variable) = ty {
            let root = self.root(*variable);
            if let Some(message) = &self.conflicts[root.0 as usize] {
                return Err(message.to_string());
            }
        }
        Ok(())
    }

    fn record_conflict(&mut self, ty: &TypeDescriptor, message: &str) {
        if let TypeDescriptor::Inference(variable) = ty {
            self.advance_revision();
            let root = self.root(*variable);
            self.nodes[root.0 as usize].set(InferenceNode::Conflicted);
            self.conflicts[root.0 as usize].get_or_insert_with(|| Arc::from(message));
            self.refresh(self.dependents[root.0 as usize].clone());
        }
    }

    fn bound(&self, variable: InferenceVariableId) -> Option<Arc<TypeDescriptor>> {
        match self.nodes[self.root(variable).0 as usize].get() {
            InferenceNode::Known(id) => Some(Arc::clone(self.descriptor_view(id))),
            _ => None,
        }
    }

    fn head<'a>(&self, ty: &'a TypeDescriptor) -> InferenceHead<'a> {
        let TypeDescriptor::Inference(variable) = ty else { return InferenceHead::Borrowed(ty); };
        let root = self.root(*variable);
        match self.bound(root) {
            None => InferenceHead::Unknown(TypeDescriptor::Inference(root)),
            Some(ty) => InferenceHead::Shared(ty),
        }
    }

    // Callers unify structures and transfer obligations before changing a root.
    fn set(&mut self, variable: InferenceVariableId, ty: TypeDescriptor) {
        let root = self.root(variable);
        assert_ne!(self.nodes[root.0 as usize].get(), InferenceNode::Conflicted,
            "cannot overwrite a conflicted inference variable");
        if let TypeDescriptor::Inference(target) = ty {
            let target = self.root(target);
            if root != target {
                self.advance_revision();
                self.nodes[root.0 as usize].set(InferenceNode::ProxyTo(target));
                let waiting = std::mem::take(&mut self.dependents[root.0 as usize]);
                self.dependents[target.0 as usize].extend(waiting.iter().copied());
                self.refresh(waiting);
            }
        } else {
            self.advance_revision();
            let id = self.lower_structure(ty);
            let mut dependencies = self.arguments(id).to_vec();
            dependencies.sort_unstable();
            dependencies.dedup();
            for dependency in dependencies {
                let dependency = self.root(dependency);
                self.dependents[dependency.0 as usize].push(root);
            }
            self.nodes[root.0 as usize].set(InferenceNode::Known(id));
            self.refresh(vec![root]);
        }
    }

    fn advance_revision(&mut self) {
        self.revision = self.revision.checked_add(1).expect("inference revision capacity exceeded");
    }
}
