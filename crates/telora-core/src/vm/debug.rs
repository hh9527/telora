struct DebugValueFormatter<'a> {
    view: HeapView<'a>,
    output: String,
    active: HashSet<Handle>,
    truncated: bool,
}

impl<'a> DebugValueFormatter<'a> {
    fn new(view: HeapView<'a>) -> Self {
        Self {
            view,
            output: String::new(),
            active: HashSet::new(),
            truncated: false,
        }
    }

    fn format(mut self, value: Val) -> Result<String, crate::heap::HeapError> {
        self.value(value, 0)?;
        if self.truncated {
            self.output.push_str("...");
        }
        Ok(self.output)
    }

    fn value(&mut self, value: Val, depth: usize) -> Result<(), crate::heap::HeapError> {
        if self.truncated {
            return Ok(());
        }
        match value.value() {
            DecodedValue::Failed(_) => self.push("<failed>"),
            DecodedValue::Int(value) => self.push(&value.to_string()),
            DecodedValue::Float(value) => self.push(&format!("{value:?}")),
            DecodedValue::BuiltinAtom(atom) => {
                self.push("'");
                self.push(atom.name());
            }
            DecodedValue::InlineAtom(text) => {
                self.push("'");
                self.push(text.as_str());
            }
            DecodedValue::Atom(id) => {
                self.push("'");
                self.push(self.view.text(id)?);
            }
            DecodedValue::InlineString(text) => self.quoted(text.as_str()),
            DecodedValue::ShortString(id) => self.quoted(self.view.text(id)?),
            DecodedValue::Bytes(handle) => match self.view.object(handle)? {
                Object::Bytes(value) => {
                    self.push("b\"");
                    for byte in value.iter().take(DEBUG_MAX_ITEMS) {
                        self.push(&format!("\\x{byte:02x}"));
                    }
                    if value.len() > DEBUG_MAX_ITEMS {
                        self.push("...");
                    }
                    self.push("\"");
                }
                _ => return Err(crate::heap::HeapError::new("invalid Bytes handle")),
            },
            DecodedValue::Opaque(handle) => match self.view.object(handle)? {
                Object::Opaque(value) => self.push(&format!("{value:?}")),
                _ => return Err(crate::heap::HeapError::new("invalid Opaque handle")),
            },
            DecodedValue::NativeType(id) => {
                self.push("<type ");
                self.push(self.view.native_type(id)?.qualified_name());
                self.push(">");
            }
            DecodedValue::DeclaredType(handle) => match self.view.object(handle)? {
                Object::DeclaredType { type_id, name, .. } => {
                    let canonical_name = self.view.canonical_type_name(*type_id)?;
                    self.push("<type ");
                    self.push(canonical_name.as_deref().unwrap_or(name));
                    self.push(">");
                }
                _ => return Err(crate::heap::HeapError::new("invalid DeclaredType handle")),
            },
            DecodedValue::SymbolicType(handle) => match self.view.object(handle)? {
                Object::SymbolicType { name, .. } => {
                    self.push("<symbolic-type ");
                    self.push(name);
                    self.push(">");
                }
                _ => return Err(crate::heap::HeapError::new("invalid SymbolicType handle")),
            },
            DecodedValue::Array(handle) => self.sequence(handle, false, depth, "[", "]")?,
            DecodedValue::Tuple(handle) => self.sequence(handle, true, depth, "(", ")")?,
            DecodedValue::Tagged(handle) => {
                if !self.enter(handle, depth) {
                    return Ok(());
                }
                let (tag, payload) = self.view.tagged(handle)?;
                self.push("'");
                let tag = self
                    .view
                    .atom_text(tag)?
                    .ok_or_else(|| crate::heap::HeapError::new("Tagged tag is not an Atom"))?;
                self.push(tag.as_str());
                self.push("(");
                self.value(payload, depth + 1)?;
                self.push(")");
                self.active.remove(&handle);
            }
            DecodedValue::Dict(handle) => self.dict(handle, depth)?,
            DecodedValue::Func(handle) => {
                let (prototype, _) = self.view.closure(handle)?;
                let name = match prototype {
                    crate::heap::RuntimePrototype::Native(function) => function.name(),
                    crate::heap::RuntimePrototype::Bytecode(prototype) => {
                        self.view.bytecode(prototype)?.0.name()
                    }
                };
                self.push("<fn ");
                self.push(name);
                self.push(">");
            }
            DecodedValue::FuncRef(id) => {
                self.push("<fn-ref ");
                self.push(&format!("{}:{}", id.module.raw(), id.local));
                self.push(">");
            }
            DecodedValue::Dyn(_) => self.push("<dyn>"),
            DecodedValue::Module(_) => self.push("<module>"),
            DecodedValue::TypeSlot(handle) => {
                if !self.enter(handle, depth) {
                    return Ok(());
                }
                match self.view.type_slot(handle)? {
                    Some(value) => self.value(value, depth + 1)?,
                    None => self.push("<uninitialized up-link>"),
                }
                self.active.remove(&handle);
            }
        }
        Ok(())
    }

    fn sequence(
        &mut self,
        handle: Handle,
        tuple: bool,
        depth: usize,
        open: &str,
        close: &str,
    ) -> Result<(), crate::heap::HeapError> {
        if !self.enter(handle, depth) {
            return Ok(());
        }
        self.push(open);
        let (value_count, values) = {
            let sequence = self.view.sequence(handle, tuple)?;
            (
                sequence.len(),
                sequence
                    .iter()
                    .take(DEBUG_MAX_ITEMS)
                    .copied()
                    .collect::<Vec<_>>(),
            )
        };
        for (index, value) in values.iter().take(DEBUG_MAX_ITEMS).enumerate() {
            if index > 0 {
                self.push(", ");
            }
            self.value(*value, depth + 1)?;
        }
        if value_count > DEBUG_MAX_ITEMS {
            if DEBUG_MAX_ITEMS > 0 {
                self.push(", ");
            }
            self.push("...");
        }
        self.push(close);
        self.active.remove(&handle);
        Ok(())
    }

    fn dict(&mut self, handle: Handle, depth: usize) -> Result<(), crate::heap::HeapError> {
        if !self.enter(handle, depth) {
            return Ok(());
        }
        self.push("{");
        let (fields, values) = self.view.dict_parts(handle)?;
        let entries = fields
            .iter()
            .zip(values)
            .take(DEBUG_MAX_ITEMS)
            .map(|(field, value)| Ok((self.view.text(*field)?.to_owned(), *value)))
            .collect::<Result<Vec<_>, crate::heap::HeapError>>()?;
        for (index, (field, value)) in entries.into_iter().enumerate() {
            if index > 0 {
                self.push(", ");
            }
            self.push(&field);
            self.push(": ");
            self.value(value, depth + 1)?;
        }
        if values.len() > DEBUG_MAX_ITEMS {
            if DEBUG_MAX_ITEMS > 0 {
                self.push(", ");
            }
            self.push("...");
        }
        self.push("}");
        self.active.remove(&handle);
        Ok(())
    }

    fn enter(&mut self, handle: Handle, depth: usize) -> bool {
        if depth >= DEBUG_MAX_DEPTH {
            self.push("...");
            return false;
        }
        if !self.active.insert(handle) {
            self.push("<cycle>");
            return false;
        }
        true
    }

    fn quoted(&mut self, text: &str) {
        self.push(&format!("{text:?}"));
    }

    fn push(&mut self, text: &str) {
        if self.truncated {
            return;
        }
        let content_limit = DEBUG_MAX_BYTES.saturating_sub(3);
        for character in text.chars() {
            if self.output.len() + character.len_utf8() > content_limit {
                self.truncated = true;
                return;
            }
            self.output.push(character);
        }
    }
}
