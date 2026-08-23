struct JsonWriter<'a> {
    view: HeapView<'a>,
    indent: Option<usize>,
    output: String,
    active: HashSet<Handle>,
}

impl<'a> JsonWriter<'a> {
    fn new(view: HeapView<'a>, indent: Option<usize>) -> Self {
        Self {
            view,
            indent,
            output: String::new(),
            active: HashSet::new(),
        }
    }

    fn value(&mut self, value: Val, depth: usize) -> Result<(), String> {
        match value.value() {
            DecodedValue::Failed(_) => {
                return Err("JSON cannot encode a failed evaluation node".into());
            }
            DecodedValue::Int(value) => self.output.push_str(&value.to_string()),
            DecodedValue::Float(value) if value.is_finite() => {
                self.output.push_str(&value.to_string())
            }
            DecodedValue::Float(_) => return Err("JSON cannot encode a non-finite Float".into()),
            DecodedValue::BuiltinAtom(BuiltinAtom::None) => self.output.push_str("null"),
            DecodedValue::BuiltinAtom(BuiltinAtom::True) => self.output.push_str("true"),
            DecodedValue::BuiltinAtom(BuiltinAtom::False) => self.output.push_str("false"),
            DecodedValue::InlineString(text) => self.string(text.as_str()),
            DecodedValue::ShortString(id) => {
                self.string(self.view.text(id).map_err(|e| e.to_string())?)
            }
            DecodedValue::Array(handle) => self.array(handle, depth)?,
            DecodedValue::Dict(handle) => self.dict(handle, depth)?,
            DecodedValue::BuiltinAtom(atom) => {
                return Err(format!("JSON cannot encode '{}", atom.name()));
            }
            DecodedValue::InlineAtom(text) => {
                return Err(format!("JSON cannot encode '{}", text.as_str()));
            }
            DecodedValue::Atom(id) => {
                return Err(format!(
                    "JSON cannot encode '{}",
                    self.view.text(id).map_err(|e| e.to_string())?
                ));
            }
            DecodedValue::Bytes(_) => return Err("JSON cannot encode Bytes".into()),
            DecodedValue::Opaque(_) => return Err("JSON cannot encode Opaque values".into()),
            DecodedValue::NativeType(_) => return Err("JSON cannot encode Type values".into()),
            DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_) => {
                return Err("JSON cannot encode Type values".into());
            }
            DecodedValue::Tuple(_) => {
                return Err("JSON cannot encode Tuple; use a codec first".into());
            }
            DecodedValue::Tagged(_) => {
                return Err("JSON cannot encode Tagged; use a codec first".into());
            }
            DecodedValue::Func(_) => return Err("JSON cannot encode Func".into()),
            DecodedValue::FuncRef(_) => return Err("JSON cannot encode Func".into()),
            DecodedValue::Dyn(_) => return Err("JSON cannot encode Dyn".into()),
            DecodedValue::Module(_) => return Err("JSON cannot encode Module".into()),
            DecodedValue::TypeSlot(_) => {
                return Err("JSON cannot encode an internal up-link".into());
            }
        }
        Ok(())
    }

    fn array(&mut self, handle: Handle, depth: usize) -> Result<(), String> {
        if !self.active.insert(handle) {
            return Err("JSON cannot encode cyclic values".into());
        }
        let values = self
            .view
            .sequence(handle, false)
            .map_err(|e| e.to_string())?
            .to_vec();
        self.output.push('[');
        for (index, value) in values.into_iter().enumerate() {
            self.separator(index, depth + 1);
            self.value(value, depth + 1)?;
        }
        self.close_collection(values_len_hint(handle, &self.view, false)?, depth, ']');
        self.active.remove(&handle);
        Ok(())
    }

    fn dict(&mut self, handle: Handle, depth: usize) -> Result<(), String> {
        if !self.active.insert(handle) {
            return Err("JSON cannot encode cyclic values".into());
        }
        let (fields, values) = self.view.dict_parts(handle).map_err(|e| e.to_string())?;
        let entries = fields
            .iter()
            .zip(values)
            .map(|(field, value)| Ok((self.view.text(*field)?.to_owned(), *value)))
            .collect::<Result<Vec<_>, crate::heap::HeapError>>()
            .map_err(|e| e.to_string())?;
        self.output.push('{');
        for (index, (field, value)) in entries.iter().enumerate() {
            self.separator(index, depth + 1);
            self.string(field);
            self.output.push(':');
            if self.indent.is_some() {
                self.output.push(' ');
            }
            self.value(*value, depth + 1)?;
        }
        self.close_collection(entries.len(), depth, '}');
        self.active.remove(&handle);
        Ok(())
    }

    fn separator(&mut self, index: usize, depth: usize) {
        if index > 0 {
            self.output.push(',');
        }
        if let Some(indent) = self.indent {
            self.output.push('\n');
            self.output
                .extend(std::iter::repeat_n(' ', indent.saturating_mul(depth)));
        }
    }

    fn close_collection(&mut self, len: usize, depth: usize, close: char) {
        if len > 0
            && let Some(indent) = self.indent
        {
            self.output.push('\n');
            self.output
                .extend(std::iter::repeat_n(' ', indent.saturating_mul(depth)));
        }
        self.output.push(close);
    }

    fn string(&mut self, value: &str) {
        self.output.push('"');
        for character in value.chars() {
            match character {
                '"' => self.output.push_str("\\\""),
                '\\' => self.output.push_str("\\\\"),
                '\u{08}' => self.output.push_str("\\b"),
                '\u{0c}' => self.output.push_str("\\f"),
                '\n' => self.output.push_str("\\n"),
                '\r' => self.output.push_str("\\r"),
                '\t' => self.output.push_str("\\t"),
                c if c <= '\u{1f}' => {
                    let _ = write!(self.output, "\\u{:04x}", c as u32);
                }
                c => self.output.push(c),
            }
        }
        self.output.push('"');
    }
}

fn values_len_hint(handle: Handle, view: &HeapView<'_>, tuple: bool) -> Result<usize, String> {
    view.sequence(handle, tuple)
        .map(|values| values.len())
        .map_err(|e| e.to_string())
}

const DEBUG_MAX_DEPTH: usize = 8;
const DEBUG_MAX_ITEMS: usize = 32;
const DEBUG_MAX_BYTES: usize = 4_096;
fn run_core_diagnostic(
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &Arc<BytecodeFunction>,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let message = view
        .string_text(arguments[0])
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                function,
                pc,
            )
        })?
        .ok_or_else(|| runtime_type_error("String", &arguments[0], &view, function, pc))?;
    let message = message.as_str().to_owned();
    let subjects = match arguments[1].value() {
        DecodedValue::Tuple(handle) => view
            .sequence(handle, true)
            .map_err(|heap_error| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            })?
            .iter()
            .filter_map(|value| value.loc())
            .collect::<Vec<_>>(),
        _ => arguments[1].loc().into_iter().collect(),
    };
    let fallback = instruction_location(function, pc);
    let primary = fallback.or_else(|| subjects.first().copied());
    let mut diagnostic = primary.map_or_else(
        || Diagnostic {
            severity: crate::source::Severity::Warning,
            message: message.clone(),
            labels: Vec::new(),
            notes: Vec::new(),
        },
        |location| Diagnostic::new(crate::source::Severity::Warning, &message, location),
    );
    for related in subjects {
        if primary != Some(related) {
            diagnostic = diagnostic.with_secondary("related value", related);
        }
    }
    account.diagnostics.push(diagnostic);
    Ok(VmAction::Return {
        value: Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
        return_target,
    })
}

