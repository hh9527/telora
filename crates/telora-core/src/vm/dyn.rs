#[allow(clippy::too_many_arguments)]
fn run_core_eq(
    operation: CoreEqFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<VmAction, RuntimeError> {
    match operation {
        CoreEqFunction::Equal => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            propagate_data_failures(arguments, &view, function, pc)?;
            let equal = view
                .values_equal(arguments[0], arguments[1])
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        pc,
                    )
                })?;
            Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::BuiltinAtom(if equal {
                        BuiltinAtom::True
                    } else {
                        BuiltinAtom::False
                    }),
                    instruction_location(function, pc),
                ),
                return_target,
            })
        }
    }
}

enum DynObservation {
    Child(Val, Val),
    Children(Vec<(Val, Val)>),
    NamedChildren(Vec<(String, Val, Val)>),
    Tag(String),
    Payload(Option<(Val, Val)>),
}

fn dyn_member_index(
    value: Val,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<u32, RuntimeError> {
    let DecodedValue::Int(index) = value.value() else {
        return Err(runtime_shallow_type_error("Int", value, function, pc));
    };
    u32::try_from(index).map_err(|_| {
        error(
            RuntimeErrorKind::TypeMismatch,
            "member index must be a non-negative u32",
            function,
            pc,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn finish_dyn_observation(
    input: Val,
    observation: Result<DynObservation, String>,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let payload = match observation {
        Ok(observation) => {
            let units = match &observation {
                DynObservation::Child(_, _) => 3,
                DynObservation::Children(children) => 2 + children.len() * 3,
                DynObservation::NamedChildren(children) => {
                    2 + children
                        .iter()
                        .map(|(name, _, _)| 5 + name.len())
                        .sum::<usize>()
                }
                DynObservation::Tag(tag) => 2 + tag.len(),
                DynObservation::Payload(None) => 2,
                DynObservation::Payload(Some(_)) => 5,
            };
            charge_allocation(
                account,
                logical_value_bytes(units)
                    .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
                function,
                pc,
            )?;
            let value = match observation {
                DynObservation::Child(descriptor, value) => {
                    value.with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
                        identity: Arc::new(()),
                        descriptor,
                        value,
                    })))
                }
                DynObservation::Children(children) => {
                    let children = children
                        .into_iter()
                        .map(|(descriptor, value)| {
                            value.with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
                                identity: Arc::new(()),
                                descriptor,
                                value,
                            })))
                        })
                        .collect();
                    Val::new(
                        DecodedValue::Array(current.allocate(Object::Array(children))),
                        input.loc(),
                    )
                }
                DynObservation::NamedChildren(children) => {
                    let children = children
                        .into_iter()
                        .map(|(name, descriptor, value)| {
                            let name =
                                Val::new(current.string(Some(background), &name), input.loc());
                            let child = value.with_value(DecodedValue::Dyn(current.allocate(
                                Object::Dyn {
                                    identity: Arc::new(()),
                                    descriptor,
                                    value,
                                },
                            )));
                            Val::new(
                                DecodedValue::Tuple(
                                    current.allocate(Object::Tuple(vec![name, child].into())),
                                ),
                                value.loc(),
                            )
                        })
                        .collect();
                    Val::new(
                        DecodedValue::Array(current.allocate(Object::Array(children))),
                        input.loc(),
                    )
                }
                DynObservation::Tag(tag) => {
                    Val::new(current.string(Some(background), &tag), input.loc())
                }
                DynObservation::Payload(None) => {
                    Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), input.loc())
                }
                DynObservation::Payload(Some((descriptor, value))) => {
                    let child =
                        value.with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
                            identity: Arc::new(()),
                            descriptor,
                            value,
                        })));
                    Val::new(
                        DecodedValue::Tagged(current.allocate(Object::Tagged {
                            tag: Val::new(
                                DecodedValue::BuiltinAtom(BuiltinAtom::Some),
                                input.loc(),
                            ),
                            payload: child,
                        })),
                        input.loc(),
                    )
                }
            };
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Ok), input.loc()),
                    payload: value,
                })),
                input.loc(),
            )
        }
        Err(message) => {
            let bytes = logical_value_bytes(2)
                .and_then(|bytes| {
                    bytes
                        .checked_add(u64::try_from(message.len()).unwrap_or(u64::MAX))
                        .ok_or_else(|| {
                            NativeError::allocation_limit("Dyn observer error size overflowed")
                        })
                })
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
            charge_allocation(account, bytes, function, pc)?;
            let message = Val::new(current.string(Some(background), &message), input.loc());
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Err), input.loc()),
                    payload: message,
                })),
                input.loc(),
            )
        }
    };
    Ok(VmAction::Return {
        value: payload,
        return_target,
    })
}
