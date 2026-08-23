#[allow(clippy::too_many_arguments)]
fn run_core_dict(
    operation: CoreDictFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let value = match operation {
        CoreDictFunction::Get => {
            let DecodedValue::Dict(handle) = arguments[0].value() else {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                return Err(runtime_type_error(
                    "Dict",
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
            let Some(key) = view
                .string_text(arguments[1])
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            else {
                return Err(runtime_type_error(
                    "String",
                    &arguments[1],
                    &view,
                    function,
                    pc,
                ));
            };
            match view
                .dict_get_text(handle, key.as_str())
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            {
                Some(payload) => {
                    charge_allocation(
                        account,
                        logical_value_bytes(2).map_err(|native_error| {
                            allocation_error(native_error.message, function, pc)
                        })?,
                        function,
                        pc,
                    )?;
                    Val::new(
                        DecodedValue::Tagged(current.allocate(Object::Tagged {
                            tag: Val::new(
                                DecodedValue::BuiltinAtom(BuiltinAtom::Some),
                                arguments[0].loc(),
                            ),
                            payload,
                        })),
                        arguments[0].loc(),
                    )
                }
                None => Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::None),
                    arguments[0].loc(),
                ),
            }
        }
        CoreDictFunction::Keys => {
            let entries =
                core_dict_entries(arguments[0], "Dict", function, pc, current, background)?;
            charge_core_dict_output(
                entries.len(),
                entries.iter().map(|(field, _)| field.len()),
                function,
                pc,
                account,
            )?;
            let values = entries
                .into_iter()
                .map(|(field, _)| {
                    Val::new(current.string(Some(background), &field), arguments[0].loc())
                })
                .collect::<Box<[_]>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(values))),
                instruction_location(function, pc),
            )
        }
        CoreDictFunction::Values => {
            let entries =
                core_dict_entries(arguments[0], "Dict", function, pc, current, background)?;
            charge_core_dict_output(entries.len(), std::iter::empty(), function, pc, account)?;
            let values = entries
                .into_iter()
                .map(|(_, value)| value)
                .collect::<Box<[_]>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(values))),
                instruction_location(function, pc),
            )
        }
        CoreDictFunction::Pairs => {
            let entries =
                core_dict_entries(arguments[0], "Dict", function, pc, current, background)?;
            let slot_count = entries.len().checked_mul(3).ok_or_else(|| {
                allocation_error("std/dict.pairs allocation size overflowed", function, pc)
            })?;
            charge_core_dict_output(
                slot_count,
                entries.iter().map(|(field, _)| field.len()),
                function,
                pc,
                account,
            )?;
            let pairs = entries
                .into_iter()
                .map(|(field, value)| {
                    let field =
                        Val::new(current.string(Some(background), &field), arguments[0].loc());
                    Val::new(
                        DecodedValue::Tuple(
                            current.allocate(Object::Tuple(vec![field, value].into())),
                        ),
                        arguments[0].loc(),
                    )
                })
                .collect::<Box<[_]>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(pairs))),
                instruction_location(function, pc),
            )
        }
        CoreDictFunction::FromPairs => {
            let DecodedValue::Array(handle) = arguments[0].value() else {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                return Err(runtime_type_error(
                    "Array",
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
            let mut entries = Vec::with_capacity(items.len());
            for (index, item) in items.iter().copied().enumerate() {
                let DecodedValue::Tuple(pair) = item.value() else {
                    if let DecodedValue::Failed(failure) = item.value() {
                        return Err(propagated_failure_error(failure, item.loc(), function, pc));
                    }
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!("std/dict.from_pairs item {index} must be a two-element Tuple"),
                        function,
                        pc,
                    ));
                };
                let pair = view
                    .sequence(pair, true)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
                if pair.len() != 2 {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!("std/dict.from_pairs item {index} must be a two-element Tuple"),
                        function,
                        pc,
                    ));
                }
                let Some(field) = view
                    .string_text(pair[0])
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                else {
                    propagate_direct_failure(&pair[0], function, pc)?;
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!("std/dict.from_pairs item {index} key must be a String"),
                        function,
                        pc,
                    ));
                };
                entries.push((field.as_str().to_owned(), pair[1]));
            }
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            if let Some(duplicate) = entries
                .windows(2)
                .find(|pair| pair[0].0 == pair[1].0)
                .map(|pair| pair[0].0.as_str())
            {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    format!("std/dict.from_pairs contains duplicate field {duplicate:?}"),
                    function,
                    pc,
                ));
            }
            allocate_core_dict(entries, function, pc, current, account)?
        }
        CoreDictFunction::Merge => {
            let left =
                core_dict_entries(arguments[0], "left Dict", function, pc, current, background)?;
            let right = core_dict_entries(
                arguments[1],
                "right Dict",
                function,
                pc,
                current,
                background,
            )?;
            let mut merged = Vec::with_capacity(left.len().saturating_add(right.len()));
            let (mut left_index, mut right_index) = (0, 0);
            while left_index < left.len() && right_index < right.len() {
                match left[left_index].0.cmp(&right[right_index].0) {
                    std::cmp::Ordering::Less => {
                        merged.push(left[left_index].clone());
                        left_index += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        merged.push(right[right_index].clone());
                        left_index += 1;
                        right_index += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        merged.push(right[right_index].clone());
                        right_index += 1;
                    }
                }
            }
            merged.extend_from_slice(&left[left_index..]);
            merged.extend_from_slice(&right[right_index..]);
            allocate_core_dict(merged, function, pc, current, account)?
        }
        CoreDictFunction::MapValues | CoreDictFunction::Filter | CoreDictFunction::Fold => {
            unreachable!("callback Dict operations use continuations")
        }
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn start_dict_continuation(
    function: CoreDictFunction,
    arguments: Vec<Val>,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let entries = core_dict_entries(
        arguments[0],
        "Dict",
        &call_function,
        call_pc,
        current,
        background,
    )?;
    let callback_index = if function == CoreDictFunction::Fold {
        2
    } else {
        1
    };
    let callback = arguments[callback_index];
    let view = HeapView {
        current,
        background: Some(background),
    };
    let Some(actual_arity) = view
        .resolved_function_arity(callback)
        .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?
    else {
        return Err(runtime_type_error(
            "Func",
            &callback,
            &view,
            &call_function,
            call_pc,
        ));
    };
    let expected_arity = if function == CoreDictFunction::Fold {
        3
    } else {
        1
    };
    if actual_arity != expected_arity {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            format!(
                "{} callback must accept {expected_arity} arguments, got {actual_arity}",
                function.name()
            ),
            &call_function,
            call_pc,
        ));
    }
    let accumulator = (function == CoreDictFunction::Fold).then_some(arguments[1]);
    next_dict_action(
        DictContinuation {
            function,
            entries,
            callback,
            next_index: 0,
            accumulator,
            output: Vec::new(),
            failed: None,
            return_target,
            trace_frame: RuntimeFrame {
                function: function.name().into(),
                instruction: 0,
                origin: call_function.origin_at(call_pc),
            },
            call_function,
            call_pc,
        },
        current,
        background,
        account,
    )
}

fn resume_dict_continuation(
    mut continuation: DictContinuation,
    value: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let entry_index = continuation.next_index - 1;
    match continuation.function {
        CoreDictFunction::MapValues => {
            let key = continuation.entries[entry_index].0.clone();
            charge_core_dict_output(
                1,
                std::iter::once(key.len()),
                &continuation.call_function,
                continuation.call_pc,
                account,
            )?;
            continuation.output.push((key, value));
        }
        CoreDictFunction::Filter => match value.value() {
            DecodedValue::BuiltinAtom(BuiltinAtom::True) => {
                charge_core_dict_output(
                    1,
                    std::iter::once(continuation.entries[entry_index].0.len()),
                    &continuation.call_function,
                    continuation.call_pc,
                    account,
                )?;
                continuation
                    .output
                    .push(continuation.entries[entry_index].clone());
            }
            DecodedValue::BuiltinAtom(BuiltinAtom::False) => {}
            _ => {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/dict.filter predicate must return 'True or 'False",
                    &continuation.call_function,
                    continuation.call_pc,
                ));
            }
        },
        CoreDictFunction::Fold => continuation.accumulator = Some(value),
        _ => unreachable!("only callback Dict operations suspend"),
    }
    next_dict_action(continuation, current, background, account)
}

fn resume_dict_failure(
    mut continuation: DictContinuation,
    failure: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    match continuation.function {
        CoreDictFunction::MapValues => {
            let key = continuation.entries[continuation.next_index - 1].0.clone();
            continuation.output.push((key, failure));
        }
        CoreDictFunction::Filter => {
            continuation.failed.get_or_insert(failure);
        }
        CoreDictFunction::Fold => {
            return Ok(VmAction::Return {
                value: failure,
                return_target: continuation.return_target,
            });
        }
        _ => unreachable!("only callback Dict operations suspend"),
    }
    next_dict_action(continuation, current, background, account)
}

fn next_dict_action(
    mut continuation: DictContinuation,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    if continuation.next_index >= continuation.entries.len() {
        if let Some(failure) = continuation.failed {
            return Ok(VmAction::Return {
                value: failure,
                return_target: continuation.return_target,
            });
        }
        let value = if continuation.function == CoreDictFunction::Fold {
            continuation
                .accumulator
                .expect("fold continuation has an accumulator")
        } else {
            allocate_core_dict_unchecked(
                continuation.output,
                current,
                instruction_location(&continuation.call_function, continuation.call_pc),
            )
        };
        return Ok(VmAction::Return {
            value,
            return_target: continuation.return_target,
        });
    }

    let (key, value) = continuation.entries[continuation.next_index].clone();
    continuation.next_index += 1;
    if matches!(value.value(), DecodedValue::Failed(_)) {
        return resume_dict_failure(continuation, value, current, background, account);
    }
    let arguments = if continuation.function == CoreDictFunction::Fold {
        charge_allocation(
            account,
            key.len() as u64,
            &continuation.call_function,
            continuation.call_pc,
        )?;
        vec![
            continuation
                .accumulator
                .expect("fold continuation has an accumulator"),
            Val::new(current.string(Some(background), &key), value.loc()),
            value,
        ]
    } else {
        vec![value]
    };
    let callee = continuation.callback;
    let call_function = Arc::clone(&continuation.call_function);
    let call_pc = continuation.call_pc;
    Ok(VmAction::Call {
        callee,
        arguments,
        return_target: ReturnTarget::Native(Box::new(continuation)),
        call_function,
        call_pc,
        rule_boundary: None,
    })
}

fn core_dict_entries(
    value: Val,
    expected: &str,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<Vec<(String, Val)>, RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let DecodedValue::Dict(handle) = value.value() else {
        return Err(runtime_type_error(expected, &value, &view, function, pc));
    };
    let (fields, values) = view
        .dict_parts(handle)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
    fields
        .iter()
        .zip(values)
        .map(|(field, value)| {
            Ok((
                view.text(*field)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .to_owned(),
                *value,
            ))
        })
        .collect()
}

fn allocate_core_dict(
    entries: Vec<(String, Val)>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    account: &mut QuotaAccount,
) -> Result<Val, RuntimeError> {
    charge_core_dict_output(
        entries.len(),
        entries.iter().map(|(field, _)| field.len()),
        function,
        pc,
        account,
    )?;
    Ok(allocate_core_dict_unchecked(
        entries,
        current,
        instruction_location(function, pc),
    ))
}

fn allocate_core_dict_unchecked(
    entries: Vec<(String, Val)>,
    current: &mut Heap,
    loc: Option<crate::Loc>,
) -> Val {
    let (fields, values): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .map(|(field, value)| (current.intern(&field), value))
        .unzip();
    let shape = current.intern_shape(fields);
    Val::new(
        DecodedValue::Dict(current.allocate(Object::Dict {
            shape,
            values: values.into(),
        })),
        loc,
    )
}

fn charge_core_dict_output(
    value_slots: usize,
    mut text_lengths: impl Iterator<Item = usize>,
    function: &BytecodeFunction,
    pc: usize,
    account: &mut QuotaAccount,
) -> Result<(), RuntimeError> {
    let text_bytes = text_lengths.try_fold(0u64, |total, length| {
        total
            .checked_add(length as u64)
            .ok_or_else(|| allocation_error("std/dict allocation size overflowed", function, pc))
    })?;
    let value_bytes = logical_value_bytes(value_slots)
        .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
    let bytes = text_bytes
        .checked_add(value_bytes)
        .ok_or_else(|| allocation_error("std/dict allocation size overflowed", function, pc))?;
    charge_allocation(account, bytes, function, pc)
}

fn core_dict_heap_error(
    heap_error: crate::heap::HeapError,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    error(
        RuntimeErrorKind::InvalidBytecode,
        heap_error.to_string(),
        function,
        pc,
    )
}

