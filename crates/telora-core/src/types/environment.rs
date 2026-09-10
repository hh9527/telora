trait TypeEnvironment {
    fn get(&self, name: &str) -> Option<&TypeDescriptor>;
}

trait MutableTypeEnvironment: TypeEnvironment {
    fn insert(&mut self, name: String, descriptor: TypeDescriptor);
    fn remove(&mut self, name: &str);
}

impl TypeEnvironment for HashMap<String, TypeDescriptor> {
    fn get(&self, name: &str) -> Option<&TypeDescriptor> { HashMap::get(self, name) }
}

impl MutableTypeEnvironment for HashMap<String, TypeDescriptor> {
    fn insert(&mut self, name: String, descriptor: TypeDescriptor) {
        HashMap::insert(self, name, descriptor);
    }

    fn remove(&mut self, name: &str) { HashMap::remove(self, name); }
}

struct ScopedTypeEnvironment<'a> {
    parent: &'a dyn TypeEnvironment,
    // None hides an outer binding whose local type is not yet known.
    bindings: Vec<(String, Option<TypeDescriptor>)>,
}

impl<'a> ScopedTypeEnvironment<'a> {
    fn new(parent: &'a dyn TypeEnvironment) -> Self {
        Self { parent, bindings: Vec::new() }
    }

    fn local(&self, name: &str) -> Option<&Option<TypeDescriptor>> {
        self.bindings.iter().rev()
            .find_map(|(key, descriptor)| (key == name).then_some(descriptor))
    }

    fn set(&mut self, name: String, descriptor: Option<TypeDescriptor>) {
        if let Some((_, previous)) = self.bindings.iter_mut().rev().find(|(key, _)| *key == name) {
            *previous = descriptor;
        } else {
            self.bindings.push((name, descriptor));
        }
    }
}

impl TypeEnvironment for ScopedTypeEnvironment<'_> {
    fn get(&self, name: &str) -> Option<&TypeDescriptor> {
        match self.local(name) {
            Some(descriptor) => descriptor.as_ref(),
            None => self.parent.get(name),
        }
    }
}

impl MutableTypeEnvironment for ScopedTypeEnvironment<'_> {
    fn insert(&mut self, name: String, descriptor: TypeDescriptor) {
        self.set(name, Some(descriptor));
    }

    fn remove(&mut self, name: &str) { self.set(name.to_owned(), None); }
}

#[cfg(test)]
mod environment_tests {
    use super::*;

    #[test]
    fn tool_input_overrides_are_sparse_nested_and_restore_missing_bindings() {
        let hir = HirProgram::default();
        let mut context = ToolInferenceContext::new(
            TypeGraph::default(), &hir, BTreeMap::new(), HashMap::new(), HashMap::new(),
            BTreeMap::new(), true, HashSet::new(),
        );
        for index in 0..1024 {
            context.environment.insert(format!("unused{index}"), TypeDescriptor::Int);
        }
        context.environment.insert("changed".into(), TypeDescriptor::Int);
        context.environment.insert("missing".into(), TypeDescriptor::String);
        context.environment.insert("Annotation".into(), TypeDescriptor::Type);
        let original = context.environment.clone();
        let program = crate::parser::parse("tool-inputs.telora",
            "fn(local: Annotation) { (local, changed, missing, added) }").unwrap();
        let replacement = HashMap::from([
            ("changed".into(), TypeDescriptor::Float),
            ("added".into(), TypeDescriptor::Bytes),
            ("Annotation".into(), TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Int))),
        ]);
        let previous = context.scope_environment_inputs(&program.value.body.value.result, &replacement);
        assert_eq!(previous.len(), 4);
        assert_eq!(context.environment["changed"], TypeDescriptor::Float);
        assert_eq!(context.environment["added"], TypeDescriptor::Bytes);
        assert!(!context.environment.contains_key("missing"));
        assert!(!context.environment.contains_key("local"));
        let inner = crate::parser::parse("tool-inner.telora", "changed").unwrap();
        let previous_inner = context.scope_environment_inputs(&inner.value.body.value.result,
            &HashMap::from([("changed".into(), TypeDescriptor::String)]));
        assert_eq!(previous_inner.len(), 1);
        assert_eq!(context.environment["changed"], TypeDescriptor::String);
        context.restore_environment_inputs(previous_inner);
        assert_eq!(context.environment["changed"], TypeDescriptor::Float);
        context.restore_environment_inputs(previous);
        assert_eq!(context.environment, original);
    }

    #[test]
    fn nested_scopes_borrow_and_shadow_without_changing_parents() {
        let base = HashMap::from([
            ("value".to_owned(), TypeDescriptor::Int),
            ("outer".to_owned(), TypeDescriptor::String),
        ]);
        let mut scope = ScopedTypeEnvironment::new(&base);
        assert!(std::ptr::eq(scope.get("outer").unwrap(), base.get("outer").unwrap()));
        scope.insert("value".into(), TypeDescriptor::String);
        {
            let mut child = ScopedTypeEnvironment::new(&scope);
            child.remove("value");
            child.remove("outer");
            assert!(child.get("value").is_none());
            assert!(child.get("outer").is_none());
            child.insert("value".into(), TypeDescriptor::Float);
            assert_eq!(child.get("value"), Some(&TypeDescriptor::Float));
        }
        assert_eq!(scope.get("value"), Some(&TypeDescriptor::String));
        assert_eq!(base.get("value"), Some(&TypeDescriptor::Int));
    }

    #[test]
    fn sibling_scopes_share_slots_and_observe_late_solutions() {
        let mut variables = InferenceVariables::default();
        let slot = variables.fresh();
        let base = HashMap::from([("value".to_owned(), TypeDescriptor::Inference(slot))]);
        let left = ScopedTypeEnvironment::new(&base);
        let right = ScopedTypeEnvironment::new(&base);
        assert!(left.bindings.is_empty());
        assert!(right.bindings.is_empty());
        assert!(std::ptr::eq(left.get("value").unwrap(), right.get("value").unwrap()));
        variables.set(slot, TypeDescriptor::Int);
        for scope in [&left, &right] {
            let Some(TypeDescriptor::Inference(id)) = scope.get("value") else {
                panic!("inherited binding must retain its slot");
            };
            assert_eq!(*id, slot);
            assert_eq!(variables.binding(*id), Some(&TypeDescriptor::Int));
        }
    }
}
