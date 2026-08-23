#[allow(clippy::too_many_arguments)]
fn run_core_path(
    operation: CorePathFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let call_loc = instruction_location(function, pc);
    let input = if operation == CorePathFunction::Join {
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
        let view = HeapView {
            current,
            background: Some(background),
        };
        let items = view
            .sequence(handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
        let mut joined = String::new();
        for (index, item) in items.iter().copied().enumerate() {
            propagate_direct_failure(&item, function, pc)?;
            let part = view
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
            if part.starts_with('/') {
                joined.clear();
                joined.push_str(part.as_str());
            } else {
                if !joined.is_empty() && !joined.ends_with('/') {
                    joined.push('/');
                }
                joined.push_str(part.as_str());
            }
        }
        joined
    } else {
        let view = HeapView {
            current,
            background: Some(background),
        };
        view.string_text(arguments[0])
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            .map(|text| text.as_str().to_owned())
            .ok_or_else(|| runtime_type_error("String", &arguments[0], &view, function, pc))?
    };
    let normalized = normalize_lexical_path(&input);
    let result = match operation {
        CorePathFunction::Join | CorePathFunction::Normalize => Some(normalized),
        CorePathFunction::Parent => match normalized.as_str() {
            "." | "/" => None,
            value if value.starts_with('/') => value
                .rfind('/')
                .map(|index| if index == 0 { "/" } else { &value[..index] })
                .map(str::to_owned),
            value => Some(
                value
                    .rfind('/')
                    .map_or(".", |index| &value[..index])
                    .to_owned(),
            ),
        },
        CorePathFunction::FileName => match normalized.as_str() {
            "." | "/" | ".." => None,
            value => Some(value.rsplit('/').next().expect("non-empty path").to_owned()),
        },
    };
    let value = if matches!(
        operation,
        CorePathFunction::Parent | CorePathFunction::FileName
    ) {
        if let Some(result) = result {
            let bytes = (result.len() as u64)
                .checked_add(
                    logical_value_bytes(2).map_err(|native_error| {
                        allocation_error(native_error.message, function, pc)
                    })?,
                )
                .ok_or_else(|| allocation_error("Path allocation size overflowed", function, pc))?;
            charge_allocation(account, bytes, function, pc)?;
            let payload = Val::new(current.string(Some(background), &result), call_loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), call_loc),
                    payload,
                })),
                call_loc,
            )
        } else {
            Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), call_loc)
        }
    } else {
        let result = result.expect("path String operation returns a value");
        charge_allocation(account, result.len() as u64, function, pc)?;
        Val::new(current.string(Some(background), &result), call_loc)
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

