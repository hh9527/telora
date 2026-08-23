#[allow(clippy::too_many_arguments)]
fn start_array_continuation(
    function: CoreArrayFunction,
    arguments: Vec<Val>,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let source = arguments[0];
    let DecodedValue::Array(source_handle) = source.value() else {
        let view = HeapView {
            current,
            background: Some(background),
        };
        return Err(runtime_type_error(
            "Array",
            &source,
            &view,
            &call_function,
            call_pc,
        ));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let length = view
        .sequence(source_handle, false)
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                &call_function,
                call_pc,
            )
        })?
        .len();
    if function == CoreArrayFunction::Length {
        let length = i64::try_from(length).map_err(|_| {
            error(
                RuntimeErrorKind::IntegerOverflow,
                "Array length does not fit Int",
                &call_function,
                call_pc,
            )
        })?;
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Int(length),
                instruction_location(&call_function, call_pc),
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Get {
        let DecodedValue::Int(index) = arguments[1].value() else {
            return Err(runtime_type_error(
                "Int",
                &arguments[1],
                &view,
                &call_function,
                call_pc,
            ));
        };
        let values = view
            .sequence(source_handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?;
        let value = usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index).copied());
        let Some(payload) = value else {
            return Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::None),
                    instruction_location(&call_function, call_pc),
                ),
                return_target,
            });
        };
        if matches!(payload.value(), DecodedValue::Failed(_)) {
            return Ok(VmAction::Return {
                value: payload,
                return_target,
            });
        }
        charge_allocation(
            account,
            logical_value_bytes(2)
                .map_err(|error| allocation_error(error.message, &call_function, call_pc))?,
            &call_function,
            call_pc,
        )?;
        let location = instruction_location(&call_function, call_pc);
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), location),
                    payload,
                })),
                location,
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Enumerate {
        let values = view
            .sequence(source_handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?
            .to_vec();
        i64::try_from(values.len()).map_err(|_| {
            error(
                RuntimeErrorKind::IntegerOverflow,
                "Array enumeration index does not fit Int",
                &call_function,
                call_pc,
            )
        })?;
        let output_slots = values.len().checked_mul(3).ok_or_else(|| {
            allocation_error("Array enumeration size overflowed", &call_function, call_pc)
        })?;
        charge_allocation(
            account,
            logical_value_bytes(output_slots)
                .map_err(|error| allocation_error(error.message, &call_function, call_pc))?,
            &call_function,
            call_pc,
        )?;
        let location = instruction_location(&call_function, call_pc);
        let output = values
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let index = i64::try_from(index).expect("enumeration length checked above");
                Val::new(
                    DecodedValue::Tuple(current.allocate(Object::Tuple(
                        vec![Val::new(DecodedValue::Int(index), location), value].into(),
                    ))),
                    location,
                )
            })
            .collect();
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Array(current.allocate(Object::Array(output))),
                location,
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Push {
        let source_values = view.sequence(source_handle, false).map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                &call_function,
                call_pc,
            )
        })?;
        let output_len = source_values.len().checked_add(1).ok_or_else(|| {
            allocation_error("Array push length overflowed", &call_function, call_pc)
        })?;
        let bytes = logical_value_bytes(output_len).map_err(|native_error| {
            allocation_error(native_error.message, &call_function, call_pc)
        })?;
        charge_allocation(account, bytes, &call_function, call_pc)?;
        let mut output = Vec::with_capacity(output_len);
        output.extend_from_slice(source_values);
        output.push(arguments[1]);
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Array(current.allocate(Object::Array(output.into()))),
                instruction_location(&call_function, call_pc),
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Concat {
        let arrays = view.sequence(source_handle, false).map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                &call_function,
                call_pc,
            )
        })?;
        let mut output = Vec::new();
        for (index, array) in arrays.iter().copied().enumerate() {
            let DecodedValue::Array(handle) = array.value() else {
                if let DecodedValue::Failed(failure) = array.value() {
                    return Err(propagated_failure_error(
                        failure,
                        array.loc(),
                        &call_function,
                        call_pc,
                    ));
                }
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    format!("std/array.concat item {index} must be an Array"),
                    &call_function,
                    call_pc,
                ));
            };
            output.extend_from_slice(view.sequence(handle, false).map_err(|heap_error| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    heap_error.to_string(),
                    &call_function,
                    call_pc,
                )
            })?);
        }
        let bytes = logical_value_bytes(output.len()).map_err(|native_error| {
            allocation_error(native_error.message, &call_function, call_pc)
        })?;
        charge_allocation(account, bytes, &call_function, call_pc)?;
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Array(current.allocate(Object::Array(output.into()))),
                instruction_location(&call_function, call_pc),
            ),
            return_target,
        });
    }
    if function == CoreArrayFunction::Zip {
        let DecodedValue::Array(right_handle) = arguments[1].value() else {
            return Err(runtime_type_error(
                "Array",
                &arguments[1],
                &view,
                &call_function,
                call_pc,
            ));
        };
        let left = view
            .sequence(source_handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?;
        let right = view
            .sequence(right_handle, false)
            .map_err(|heap_error| core_dict_heap_error(heap_error, &call_function, call_pc))?;
        if left.len() != right.len() {
            return Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::None),
                    instruction_location(&call_function, call_pc),
                ),
                return_target,
            });
        }
        let pairs = left
            .iter()
            .copied()
            .zip(right.iter().copied())
            .collect::<Vec<_>>();
        charge_allocation(
            account,
            logical_value_bytes(2 + pairs.len() * 3)
                .map_err(|error| allocation_error(error.message, &call_function, call_pc))?,
            &call_function,
            call_pc,
        )?;
        let pairs = pairs
            .into_iter()
            .map(|(left, right)| {
                Val::new(
                    DecodedValue::Tuple(current.allocate(Object::Tuple(vec![left, right].into()))),
                    left.loc(),
                )
            })
            .collect();
        let pairs = Val::new(
            DecodedValue::Array(current.allocate(Object::Array(pairs))),
            source.loc(),
        );
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), source.loc()),
                    payload: pairs,
                })),
                source.loc(),
            ),
            return_target,
        });
    }

    let controlled_fold = function == CoreArrayFunction::FoldControl;
    let callback_index = if function == CoreArrayFunction::Fold || controlled_fold {
        2
    } else {
        1
    };
    let callback = arguments[callback_index];
    let Some(actual_callback_arity) = view
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
    let expected_callback_arity = if function == CoreArrayFunction::Fold || controlled_fold {
        2
    } else {
        1
    };
    if actual_callback_arity != expected_callback_arity {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            format!(
                "{} callback must accept {expected_callback_arity} arguments, got {actual_callback_arity}",
                core_array_name(function)
            ),
            &call_function,
            call_pc,
        ));
    }

    let accumulator =
        (function == CoreArrayFunction::Fold || controlled_fold).then_some(arguments[1]);
    let continuation = ArrayContinuation {
        function,
        source,
        callback,
        next_index: 0,
        accumulator,
        output: Vec::new(),
        failed: None,
        return_target,
        trace_frame: RuntimeFrame {
            function: core_array_name(function).into(),
            instruction: 0,
            origin: call_function.origin_at(call_pc),
        },
        call_function,
        call_pc,
    };
    next_array_action(continuation, current, background, account)
}

fn resume_array_continuation(
    mut continuation: ArrayContinuation,
    value: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    match continuation.function {
        CoreArrayFunction::Length
        | CoreArrayFunction::Get
        | CoreArrayFunction::Enumerate
        | CoreArrayFunction::Push
        | CoreArrayFunction::Concat
        | CoreArrayFunction::Zip => {
            unreachable!("non-callback array operation does not suspend")
        }
        CoreArrayFunction::Map => {
            charge_array_output(&continuation, account, 1)?;
            continuation.output.push(value);
        }
        CoreArrayFunction::Filter => match value.value() {
            DecodedValue::BuiltinAtom(BuiltinAtom::True) => {
                let item = array_item(
                    continuation.source,
                    continuation.next_index - 1,
                    current,
                    background,
                    &continuation.call_function,
                    continuation.call_pc,
                )?;
                charge_array_output(&continuation, account, 1)?;
                continuation.output.push(item);
            }
            DecodedValue::BuiltinAtom(BuiltinAtom::False) => {}
            _ => {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/array.filter predicate must return 'True or 'False",
                    &continuation.call_function,
                    continuation.call_pc,
                ));
            }
        },
        CoreArrayFunction::FlatMap => {
            let DecodedValue::Array(handle) = value.value() else {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/array.flat_map callback must return an Array",
                    &continuation.call_function,
                    continuation.call_pc,
                ));
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let values = view
                .sequence(handle, false)
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        &continuation.call_function,
                        continuation.call_pc,
                    )
                })?
                .to_vec();
            charge_array_output(&continuation, account, values.len())?;
            continuation.output.extend(values);
        }
        CoreArrayFunction::Fold => continuation.accumulator = Some(value),
        CoreArrayFunction::FoldControl => {
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/array.fold_control callback must return 'Continue(value) or 'Break(value)",
                    &continuation.call_function,
                    continuation.call_pc,
                ));
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let (tag, payload) = view.tagged(handle).map_err(|heap_error| {
                core_dict_heap_error(
                    heap_error,
                    &continuation.call_function,
                    continuation.call_pc,
                )
            })?;
            let tag = view.atom_text(tag).map_err(|heap_error| {
                core_dict_heap_error(
                    heap_error,
                    &continuation.call_function,
                    continuation.call_pc,
                )
            })?;
            match tag.as_ref().map(crate::TextRef::as_str) {
                Some("Continue") => continuation.accumulator = Some(payload),
                Some("Break") => {
                    return Ok(VmAction::Return {
                        value,
                        return_target: continuation.return_target,
                    });
                }
                _ => {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        "std/array.fold_control callback must return 'Continue(value) or 'Break(value)",
                        &continuation.call_function,
                        continuation.call_pc,
                    ));
                }
            }
        }
        CoreArrayFunction::Any | CoreArrayFunction::All | CoreArrayFunction::Find => {
            let matched = match value.value() {
                DecodedValue::BuiltinAtom(BuiltinAtom::True) => true,
                DecodedValue::BuiltinAtom(BuiltinAtom::False) => false,
                _ => {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!(
                            "{} predicate must return 'True or 'False",
                            continuation.function.name()
                        ),
                        &continuation.call_function,
                        continuation.call_pc,
                    ));
                }
            };
            if continuation.function == CoreArrayFunction::Any && matched {
                return Ok(VmAction::Return {
                    value: Val::new(
                        DecodedValue::BuiltinAtom(BuiltinAtom::True),
                        instruction_location(&continuation.call_function, continuation.call_pc),
                    ),
                    return_target: continuation.return_target,
                });
            }
            if continuation.function == CoreArrayFunction::All && !matched {
                return Ok(VmAction::Return {
                    value: Val::new(
                        DecodedValue::BuiltinAtom(BuiltinAtom::False),
                        instruction_location(&continuation.call_function, continuation.call_pc),
                    ),
                    return_target: continuation.return_target,
                });
            }
            if continuation.function == CoreArrayFunction::Find && matched {
                if let Some(failure) = continuation.failed {
                    return Ok(VmAction::Return {
                        value: failure,
                        return_target: continuation.return_target,
                    });
                }
                let item = array_item(
                    continuation.source,
                    continuation.next_index - 1,
                    current,
                    background,
                    &continuation.call_function,
                    continuation.call_pc,
                )?;
                charge_array_output(&continuation, account, 2)?;
                return Ok(VmAction::Return {
                    value: Val::new(
                        DecodedValue::Tagged(current.allocate(Object::Tagged {
                            tag: Val::new(
                                DecodedValue::BuiltinAtom(BuiltinAtom::Some),
                                instruction_location(
                                    &continuation.call_function,
                                    continuation.call_pc,
                                ),
                            ),
                            payload: item,
                        })),
                        instruction_location(&continuation.call_function, continuation.call_pc),
                    ),
                    return_target: continuation.return_target,
                });
            }
        }
    }
    next_array_action(continuation, current, background, account)
}

fn resume_array_failure(
    mut continuation: ArrayContinuation,
    failure: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    match continuation.function {
        CoreArrayFunction::Map => continuation.output.push(failure),
        CoreArrayFunction::Fold | CoreArrayFunction::FoldControl => {
            return Ok(VmAction::Return {
                value: failure,
                return_target: continuation.return_target,
            });
        }
        CoreArrayFunction::Filter
        | CoreArrayFunction::FlatMap
        | CoreArrayFunction::Any
        | CoreArrayFunction::All
        | CoreArrayFunction::Find => {
            continuation.failed.get_or_insert(failure);
        }
        CoreArrayFunction::Length
        | CoreArrayFunction::Get
        | CoreArrayFunction::Enumerate
        | CoreArrayFunction::Push
        | CoreArrayFunction::Concat
        | CoreArrayFunction::Zip => unreachable!("non-callback operation cannot resume"),
    }
    next_array_action(continuation, current, background, account)
}

fn next_array_action(
    mut continuation: ArrayContinuation,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let DecodedValue::Array(handle) = continuation.source.value() else {
        unreachable!("validated Array continuation source")
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let length = view
        .sequence(handle, false)
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                &continuation.call_function,
                continuation.call_pc,
            )
        })?
        .len();
    if continuation.next_index >= length {
        if let Some(failure) = continuation.failed {
            return Ok(VmAction::Return {
                value: failure,
                return_target: continuation.return_target,
            });
        }
        let value = match continuation.function {
            CoreArrayFunction::Fold => continuation
                .accumulator
                .expect("fold continuation has an accumulator"),
            CoreArrayFunction::FoldControl => {
                let accumulator = continuation
                    .accumulator
                    .expect("controlled fold continuation has an accumulator");
                charge_allocation(
                    account,
                    logical_value_bytes(2).map_err(|error| {
                        allocation_error(
                            error.message,
                            &continuation.call_function,
                            continuation.call_pc,
                        )
                    })?,
                    &continuation.call_function,
                    continuation.call_pc,
                )?;
                let continue_tag = current.intern("Continue");
                Val::new(
                    DecodedValue::Tagged(current.allocate(Object::Tagged {
                        tag: Val::new(
                            DecodedValue::Atom(continue_tag),
                            instruction_location(&continuation.call_function, continuation.call_pc),
                        ),
                        payload: accumulator,
                    })),
                    instruction_location(&continuation.call_function, continuation.call_pc),
                )
            }
            CoreArrayFunction::Any => Val::new(
                DecodedValue::BuiltinAtom(BuiltinAtom::False),
                instruction_location(&continuation.call_function, continuation.call_pc),
            ),
            CoreArrayFunction::All => Val::new(
                DecodedValue::BuiltinAtom(BuiltinAtom::True),
                instruction_location(&continuation.call_function, continuation.call_pc),
            ),
            CoreArrayFunction::Find => Val::new(
                DecodedValue::BuiltinAtom(BuiltinAtom::None),
                instruction_location(&continuation.call_function, continuation.call_pc),
            ),
            CoreArrayFunction::Map | CoreArrayFunction::Filter | CoreArrayFunction::FlatMap => {
                Val::new(
                    DecodedValue::Array(
                        current.allocate(Object::Array(continuation.output.into())),
                    ),
                    instruction_location(&continuation.call_function, continuation.call_pc),
                )
            }
            CoreArrayFunction::Length
            | CoreArrayFunction::Get
            | CoreArrayFunction::Enumerate
            | CoreArrayFunction::Push
            | CoreArrayFunction::Concat
            | CoreArrayFunction::Zip => {
                unreachable!()
            }
        };
        return Ok(VmAction::Return {
            value,
            return_target: continuation.return_target,
        });
    }

    let item = array_item(
        continuation.source,
        continuation.next_index,
        current,
        background,
        &continuation.call_function,
        continuation.call_pc,
    )?;
    continuation.next_index += 1;
    if matches!(item.value(), DecodedValue::Failed(_)) {
        return resume_array_failure(continuation, item, current, background, account);
    }
    let arguments = if matches!(
        continuation.function,
        CoreArrayFunction::Fold | CoreArrayFunction::FoldControl
    ) {
        vec![
            continuation
                .accumulator
                .expect("fold continuation has an accumulator"),
            item,
        ]
    } else {
        vec![item]
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

fn array_item(
    source: Val,
    index: usize,
    current: &Heap,
    background: &Heap,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    let DecodedValue::Array(handle) = source.value() else {
        unreachable!("validated Array source")
    };
    HeapView {
        current,
        background: Some(background),
    }
    .sequence(handle, false)
    .map_err(|heap_error| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            heap_error.to_string(),
            function,
            pc,
        )
    })?
    .get(index)
    .copied()
    .ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "Array continuation index is out of bounds",
            function,
            pc,
        )
    })
}

fn charge_array_output(
    continuation: &ArrayContinuation,
    account: &mut QuotaAccount,
    count: usize,
) -> Result<(), RuntimeError> {
    let bytes = logical_value_bytes(count).map_err(|native_error| {
        allocation_error(
            native_error.message,
            &continuation.call_function,
            continuation.call_pc,
        )
    })?;
    charge_allocation(
        account,
        bytes,
        &continuation.call_function,
        continuation.call_pc,
    )
}

const fn core_array_name(function: CoreArrayFunction) -> &'static str {
    function.name()
}

