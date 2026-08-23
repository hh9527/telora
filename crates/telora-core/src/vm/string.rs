#[allow(clippy::too_many_arguments)]
fn run_core_string(
    operation: CoreStringFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let argument = |index: usize| -> Result<String, RuntimeError> {
        let view = HeapView {
            current,
            background: Some(background),
        };
        view.string_text(arguments[index])
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            .map(|text| text.as_str().to_owned())
            .ok_or_else(|| runtime_type_error("String", &arguments[index], &view, function, pc))
    };
    let call_loc = instruction_location(function, pc);
    let value = match operation {
        CoreStringFunction::Length => {
            let length = i64::try_from(argument(0)?.chars().count()).map_err(|_| {
                error(
                    RuntimeErrorKind::IntegerOverflow,
                    "String length does not fit Int",
                    function,
                    pc,
                )
            })?;
            Val::new(DecodedValue::Int(length), call_loc)
        }
        CoreStringFunction::Join | CoreStringFunction::JoinLines => {
            let DecodedValue::Array(handle) = arguments[0].value() else {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                return Err(runtime_type_error(
                    "Array(String)",
                    &arguments[0],
                    &view,
                    function,
                    pc,
                ));
            };
            let separator = if operation == CoreStringFunction::Join {
                argument(1)?
            } else {
                "\n".to_owned()
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let items = view
                .sequence(handle, false)
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
            let mut strings = Vec::with_capacity(items.len());
            for (index, item) in items.iter().copied().enumerate() {
                propagate_direct_failure(&item, function, pc)?;
                let text = view
                    .string_text(item)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            format!("{} item {index} must be a String", operation.name()),
                            function,
                            pc,
                        )
                    })?;
                strings.push(text.as_str().to_owned());
            }
            let output = strings.join(&separator);
            charge_allocation(account, output.len() as u64, function, pc)?;
            Val::new(current.string(Some(background), &output), call_loc)
        }
        CoreStringFunction::Split | CoreStringFunction::Lines => {
            let source = argument(0)?;
            let pieces = if operation == CoreStringFunction::Lines {
                source
                    .split('\n')
                    .map(|line| line.strip_suffix('\r').unwrap_or(line))
                    .collect::<Vec<_>>()
            } else {
                let separator = argument(1)?;
                source.split(&separator).collect::<Vec<_>>()
            };
            let text_bytes = pieces.iter().try_fold(0u64, |total, piece| {
                total.checked_add(piece.len() as u64).ok_or_else(|| {
                    allocation_error("String split allocation size overflowed", function, pc)
                })
            })?;
            let slot_bytes = logical_value_bytes(pieces.len())
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
            charge_allocation(
                account,
                text_bytes.checked_add(slot_bytes).ok_or_else(|| {
                    allocation_error("String split allocation size overflowed", function, pc)
                })?,
                function,
                pc,
            )?;
            let values = pieces
                .into_iter()
                .map(|piece| Val::new(current.string(Some(background), piece), call_loc))
                .collect::<Box<[_]>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(values))),
                call_loc,
            )
        }
        CoreStringFunction::StartsWith
        | CoreStringFunction::EndsWith
        | CoreStringFunction::Contains => {
            let source = argument(0)?;
            let needle = argument(1)?;
            let result = match operation {
                CoreStringFunction::StartsWith => source.starts_with(&needle),
                CoreStringFunction::EndsWith => source.ends_with(&needle),
                CoreStringFunction::Contains => source.contains(&needle),
                _ => unreachable!(),
            };
            Val::new(
                DecodedValue::BuiltinAtom(if result {
                    BuiltinAtom::True
                } else {
                    BuiltinAtom::False
                }),
                call_loc,
            )
        }
        CoreStringFunction::Replace => {
            let output = argument(0)?.replace(&argument(1)?, &argument(2)?);
            charge_allocation(account, output.len() as u64, function, pc)?;
            Val::new(current.string(Some(background), &output), call_loc)
        }
        CoreStringFunction::Indent => {
            let source = argument(0)?;
            let DecodedValue::Int(width) = arguments[1].value() else {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                return Err(runtime_type_error(
                    "Int",
                    &arguments[1],
                    &view,
                    function,
                    pc,
                ));
            };
            let width = usize::try_from(width).map_err(|_| {
                error(
                    RuntimeErrorKind::TypeMismatch,
                    "String indentation width must be non-negative",
                    function,
                    pc,
                )
            })?;
            let indented_lines = source
                .split_inclusive('\n')
                .filter(|line| !line.trim_matches(['\r', '\n']).is_empty())
                .count();
            let output_bytes = width
                .checked_mul(indented_lines)
                .and_then(|added| source.len().checked_add(added))
                .and_then(|bytes| u64::try_from(bytes).ok())
                .ok_or_else(|| {
                    allocation_error("String indentation size overflowed", function, pc)
                })?;
            charge_allocation(account, output_bytes, function, pc)?;
            let prefix = " ".repeat(width);
            let mut output = String::with_capacity(output_bytes as usize);
            for line in source.split_inclusive('\n') {
                if !line.trim_matches(['\r', '\n']).is_empty() {
                    output.push_str(&prefix);
                }
                output.push_str(line);
            }
            Val::new(current.string(Some(background), &output), call_loc)
        }
        CoreStringFunction::EnsureTrailingNewline => {
            let mut output = argument(0)?;
            if !output.ends_with('\n') {
                output.push('\n');
            }
            charge_allocation(account, output.len() as u64, function, pc)?;
            Val::new(current.string(Some(background), &output), call_loc)
        }
        CoreStringFunction::TrimMargin => {
            let source = argument(0)?;
            let margin = argument(1)?;
            if margin.is_empty() {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "String margin marker must not be empty",
                    function,
                    pc,
                ));
            }
            let mut output = String::new();
            for line in source.split_inclusive('\n') {
                let content_end = line.trim_end_matches(['\r', '\n']).len();
                let content = &line[..content_end];
                let newline = &line[content_end..];
                let marker = content
                    .bytes()
                    .take_while(|byte| matches!(byte, b' ' | b'\t'))
                    .count();
                output.push_str(content[marker..].strip_prefix(&margin).unwrap_or(content));
                output.push_str(newline);
            }
            charge_allocation(account, output.len() as u64, function, pc)?;
            Val::new(current.string(Some(background), &output), call_loc)
        }
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

fn normalize_lexical_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut components: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." if components.last().is_some_and(|last| *last != "..") => {
                components.pop();
            }
            ".." if !absolute => components.push(component),
            ".." => {}
            _ => components.push(component),
        }
    }
    if absolute {
        if components.is_empty() {
            "/".into()
        } else {
            format!("/{}", components.join("/"))
        }
    } else if components.is_empty() {
        ".".into()
    } else {
        components.join("/")
    }
}

