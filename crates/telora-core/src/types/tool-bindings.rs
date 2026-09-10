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

}

impl ToolBindings for ScopedToolBindings<'_> {
    fn get(&self, name: &str) -> Option<&Val> {
        self.local.iter().rev().find_map(|(key, value)| (key == name).then_some(value))
            .or_else(|| self.parent.get(name))
    }
}
// This consumes execution links only after static plans have been solved.
fn link_external_tool_values(
    source_name: &str,
    sources: &SourceDatabase,
    program: &Program,
    external_roots: &BTreeMap<String, PersistentValue>,
    dynamic_bindings: &HashSet<String>,
    authored_names: &HashSet<&str>,
) -> Result<BTreeMap<String, Val>, FrontendError> {
    for name in dynamic_bindings {
        if !external_roots.contains_key(name) {
            return Err(frontend_error(source_name,
                format!("dynamic binding {name:?} has no value")));
        }
    }
    let mut values = external_roots.iter()
        .filter(|(name, _)| !authored_names.contains(name.as_str()))
        .map(|(name, root)| (name.clone(), root.runtime()))
        .collect::<BTreeMap<_, _>>();
    for binding in &program.value.body.value.bindings {
        let name = &binding.value.name.value;
        if !matches!(binding.value.kind,
            BindingKind::Import | BindingKind::Native | BindingKind::NativeType)
        {
            continue;
        }
        let root = external_roots.get(name).ok_or_else(|| {
            let message = if binding.value.kind == BindingKind::Import {
                format!("import {name} has not been resolved")
            } else {
                format!("native symbol {name:?} has not been linked")
            };
            FrontendError::from_diagnostic(sources,
                Diagnostic::error(message, binding.location))
        })?;
        values.insert(name.clone(), root.runtime());
    }
    Ok(values)
}
