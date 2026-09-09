trait ToolBindings {
    fn get(&self, name: &str) -> Option<&Val>;
}

impl ToolBindings for BTreeMap<String, Val> {
    fn get(&self, name: &str) -> Option<&Val> { BTreeMap::get(self, name) }
}

struct ScopedToolBindings<'a> {
    parent: &'a dyn ToolBindings,
    local: Vec<(String, Val)>,
}

impl<'a> ScopedToolBindings<'a> {
    fn new(parent: &'a dyn ToolBindings) -> Self {
        Self { parent, local: Vec::new() }
    }

    fn insert(&mut self, name: String, value: Val) {
        self.local.push((name, value));
    }

    fn insert_type_parameters(
        &mut self,
        parameters: &[TypeParameter],
        evaluator: &mut ToolEvaluator<'_>,
    ) -> Result<(), FrontendError> {
        for parameter in parameters {
            let value = evaluator.descriptor(&TypeDescriptor::Bound(parameter.id))?;
            self.insert(parameter.name.clone(), value);
        }
        Ok(())
    }
}

impl ToolBindings for ScopedToolBindings<'_> {
    fn get(&self, name: &str) -> Option<&Val> {
        self.local.iter().rev().find_map(|(key, value)| (key == name).then_some(value))
            .or_else(|| self.parent.get(name))
    }
}
