trait TypeEnvironment {
    fn get(&self, name: &str) -> Option<&TypeDescriptor>;
    fn visit(&self, visitor: &mut dyn FnMut(&str, &TypeDescriptor));
}

trait MutableTypeEnvironment: TypeEnvironment {
    fn insert(&mut self, name: String, descriptor: TypeDescriptor);
    fn remove(&mut self, name: &str);
}

impl TypeEnvironment for HashMap<String, TypeDescriptor> {
    fn get(&self, name: &str) -> Option<&TypeDescriptor> { HashMap::get(self, name) }

    fn visit(&self, visitor: &mut dyn FnMut(&str, &TypeDescriptor)) {
        for (name, descriptor) in self { visitor(name, descriptor); }
    }
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

    fn visit(&self, visitor: &mut dyn FnMut(&str, &TypeDescriptor)) {
        self.parent.visit(&mut |name, descriptor| {
            if self.local(name).is_none() { visitor(name, descriptor); }
        });
        for (name, descriptor) in &self.bindings {
            if let Some(descriptor) = descriptor { visitor(name, descriptor); }
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
    fn visiting_exposes_each_visible_binding_once() {
        let base = HashMap::from([
            ("hidden".to_owned(), TypeDescriptor::Int),
            ("shadowed".to_owned(), TypeDescriptor::Int),
            ("inherited".to_owned(), TypeDescriptor::String),
        ]);
        let mut scope = ScopedTypeEnvironment::new(&base);
        scope.remove("hidden");
        scope.insert("shadowed".into(), TypeDescriptor::Float);
        let mut child = ScopedTypeEnvironment::new(&scope);
        child.insert("local".into(), TypeDescriptor::Int);
        let mut visible = HashMap::new();
        child.visit(&mut |name, descriptor| {
            assert!(visible.insert(name.to_owned(), descriptor.clone()).is_none());
        });
        assert_eq!(visible, HashMap::from([
            ("shadowed".to_owned(), TypeDescriptor::Float),
            ("inherited".to_owned(), TypeDescriptor::String),
            ("local".to_owned(), TypeDescriptor::Int),
        ]));
    }
}
