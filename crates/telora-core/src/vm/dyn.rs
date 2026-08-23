#[allow(clippy::too_many_arguments)]
fn run_core_dyn(
    operation: CoreDynFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    if operation == CoreDynFunction::Pack {
        decode_runtime_type(arguments[0], current, background).map_err(|message| {
            error(
                RuntimeErrorKind::TypeMismatch,
                format!("std/dyn.pack expects canonical Type metadata: {message}"),
                function,
                pc,
            )
        })?;
        charge_allocation(
            account,
            logical_value_bytes(2)
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
            function,
            pc,
        )?;
        return Ok(VmAction::Return {
            value: arguments[1].with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
                identity: Arc::new(()),
                descriptor: arguments[0],
                value: arguments[1],
                scheme: None,
                origin: None,
            }))),
            return_target,
        });
    }

    if operation == CoreDynFunction::ProjectWith {
        let DecodedValue::Dyn(handle) = arguments[1].value() else {
            return Err(runtime_shallow_type_error(
                "Dyn",
                arguments[1],
                function,
                pc,
            ));
        };
        let view = HeapView {
            current,
            background: Some(background),
        };
        let (_, packaged_descriptor, payload) = view
            .dyn_parts(handle)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
        let target_id = current
            .canonical_type_value_id(
                ValueRef::work(arguments[0], current, background),
                "std/dyn.project_with target",
            )
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
        let packaged_id = current
            .canonical_type_value_id(
                ValueRef::work(packaged_descriptor, current, background),
                "std/dyn.project_with package",
            )
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
        if target_id != packaged_id {
            return Ok(VmAction::Return {
                value: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), payload.loc()),
                return_target,
            });
        }
        charge_allocation(
            account,
            logical_value_bytes(2)
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
            function,
            pc,
        )?;
        return Ok(VmAction::Return {
            value: Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), payload.loc()),
                    payload,
                })),
                payload.loc(),
            ),
            return_target,
        });
    }

    let DecodedValue::Dyn(handle) = arguments[0].value() else {
        return Err(runtime_shallow_type_error(
            "Dyn",
            arguments[0],
            function,
            pc,
        ));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let (_, descriptor, value) = view
        .dyn_parts(handle)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
    if operation == CoreDynFunction::GetFieldValue {
        let index = dyn_member_index(arguments[1], function, pc)?;
        let (child_descriptor, child_value) = dyn_field_by_index(descriptor, value, index, &view)
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        charge_allocation(
            account,
            logical_value_bytes(3)
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
            function,
            pc,
        )?;
        return Ok(VmAction::Return {
            value: child_value.with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
                identity: Arc::new(()),
                descriptor: child_descriptor,
                value: child_value,
                scheme: None,
                origin: None,
            }))),
            return_target,
        });
    }
    if matches!(
        operation,
        CoreDynFunction::GetVariantIndex | CoreDynFunction::GetVariantPayload
    ) {
        let (index, payload) = dyn_variant_by_index(descriptor, value, &view)
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        if operation == CoreDynFunction::GetVariantIndex {
            return Ok(VmAction::Return {
                value: Val::new(DecodedValue::Int(i64::from(index)), value.loc()),
                return_target,
            });
        }
        let expected = dyn_member_index(arguments[1], function, pc)?;
        if expected != index {
            return Err(error(
                RuntimeErrorKind::TypeMismatch,
                format!("Dyn variant index is {index}, not {expected}"),
                function,
                pc,
            ));
        }
        charge_allocation(
            account,
            logical_value_bytes(if payload.is_some() { 5 } else { 2 })
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
            function,
            pc,
        )?;
        let payload = match payload {
            Some((child_descriptor, child_value)) => {
                let child = child_value.with_value(DecodedValue::Dyn(current.allocate(
                    Object::Dyn {
                        identity: Arc::new(()),
                        descriptor: child_descriptor,
                        value: child_value,
                        scheme: None,
                        origin: None,
                    },
                )));
                Val::new(
                    DecodedValue::Tagged(current.allocate(Object::Tagged {
                        tag: Val::new(
                            DecodedValue::BuiltinAtom(BuiltinAtom::Some),
                            value.loc(),
                        ),
                        payload: child,
                    })),
                    value.loc(),
                )
            }
            None => Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), value.loc()),
        };
        return Ok(VmAction::Return {
            value: payload,
            return_target,
        });
    }
    match operation {
        CoreDynFunction::Pack
        | CoreDynFunction::ProjectWith
        | CoreDynFunction::GetFieldValue
        | CoreDynFunction::GetVariantIndex
        | CoreDynFunction::GetVariantPayload => {
            unreachable!("operation handled above")
        }
        CoreDynFunction::Desc => Ok(VmAction::Return {
            value: descriptor,
            return_target,
        }),
        CoreDynFunction::Kind => {
            let kind = match value.value() {
                DecodedValue::Failed(failure) => {
                    return Err(propagated_failure_error(failure, value.loc(), function, pc));
                }
                DecodedValue::Int(_) => "Int",
                DecodedValue::Float(_) => "Float",
                DecodedValue::InlineString(_) | DecodedValue::ShortString(_) => "String",
                DecodedValue::Bytes(_) => "Bytes",
                DecodedValue::Opaque(_) => "Opaque",
                DecodedValue::NativeType(_) => "Type",
                DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_) => "Type",
                DecodedValue::Dict(_) => "Dict",
                DecodedValue::Array(_) => "Array",
                DecodedValue::BuiltinAtom(_)
                | DecodedValue::InlineAtom(_)
                | DecodedValue::Atom(_) => "Atom",
                DecodedValue::Tagged(_) => "Tagged",
                DecodedValue::Tuple(_) => "Tuple",
                DecodedValue::Func(_) => "Func",
                DecodedValue::FuncRef(_) => "Func",
                DecodedValue::Dyn(_) => "Dyn",
                DecodedValue::Module(_) => {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        "Dyn cannot contain a Module object",
                        function,
                        pc,
                    ));
                }
                DecodedValue::TypeSlot(_) => {
                    return Err(error(
                        RuntimeErrorKind::InvalidBytecode,
                        "Dyn payload cannot be an internal up-link",
                        function,
                        pc,
                    ));
                }
            };
            Ok(VmAction::Return {
                value: Val::new(DecodedValue::Atom(current.intern(kind)), value.loc()),
                return_target,
            })
        }
        CoreDynFunction::CheckInt
        | CoreDynFunction::CheckFloat
        | CoreDynFunction::CheckString
        | CoreDynFunction::CheckBytes => {
            let expected = match operation {
                CoreDynFunction::CheckInt => "Int",
                CoreDynFunction::CheckFloat => "Float",
                CoreDynFunction::CheckString => "String",
                CoreDynFunction::CheckBytes => "Bytes",
                _ => unreachable!(),
            };
            let descriptor_kind = dyn_descriptor_leaf_kind(descriptor, &view)
                .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
            let value_matches = match operation {
                CoreDynFunction::CheckInt => matches!(value.value(), DecodedValue::Int(_)),
                CoreDynFunction::CheckFloat => matches!(value.value(), DecodedValue::Float(_)),
                CoreDynFunction::CheckString => matches!(
                    value.value(),
                    DecodedValue::InlineString(_) | DecodedValue::ShortString(_)
                ),
                CoreDynFunction::CheckBytes => matches!(value.value(), DecodedValue::Bytes(_)),
                _ => unreachable!(),
            };
            if descriptor_kind != expected || !value_matches {
                return Ok(VmAction::Return {
                    value: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), value.loc()),
                    return_target,
                });
            }
            charge_allocation(
                account,
                logical_value_bytes(2)
                    .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
                function,
                pc,
            )?;
            Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::Tagged(current.allocate(Object::Tagged {
                        tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), value.loc()),
                        payload: value,
                    })),
                    value.loc(),
                ),
                return_target,
            })
        }
        CoreDynFunction::Field
        | CoreDynFunction::Fields
        | CoreDynFunction::ArrayItems
        | CoreDynFunction::TupleItems
        | CoreDynFunction::Tag
        | CoreDynFunction::Payload => {
            let field = if operation == CoreDynFunction::Field {
                Some(
                    view.string_text(arguments[1])
                        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                        .ok_or_else(|| {
                            runtime_shallow_type_error("String", arguments[1], function, pc)
                        })?
                        .to_owned(),
                )
            } else {
                None
            };
            let observation =
                observe_dyn_structure(operation, descriptor, value, field.as_deref(), &view);
            finish_dyn_observation(
                operation,
                arguments[0],
                observation,
                return_target,
                function,
                pc,
                current,
                background,
                account,
            )
        }
    }
}

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

fn observe_dyn_structure(
    operation: CoreDynFunction,
    descriptor: Val,
    value: Val,
    field: Option<&str>,
    view: &HeapView<'_>,
) -> Result<DynObservation, String> {
    let descriptor = normalize_dyn_descriptor(descriptor, view)?;
    let value = view
        .unwrap_declared(value)
        .map_err(|error| error.to_string())?;
    let DecodedValue::Dict(type_handle) = descriptor.value() else {
        return Err("Dyn descriptor is not Type metadata".into());
    };
    let kind = view
        .dict_get_text(type_handle, "kind")
        .map_err(|error| error.to_string())?
        .and_then(|kind| view.atom_text(kind).ok().flatten())
        .ok_or_else(|| "Dyn descriptor has no Atom kind".to_owned())?;
    let type_field = |name: &str| {
        view.dict_get_text(type_handle, name)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("{kind} descriptor is missing {name}"))
    };
    match operation {
        CoreDynFunction::Field => {
            let name = field.expect("field operation has a name");
            let child_value = match value.value() {
                DecodedValue::Dict(value_handle) => view
                    .dict_get_text(value_handle, name)
                    .map_err(|error| error.to_string())?,
                DecodedValue::Module(value_handle) => view
                    .module_get_text(value_handle, name)
                    .map_err(|error| error.to_string())?,
                _ => return Err(format!("dyn.field expected {kind} runtime record")),
            };
            let child_value =
                child_value.ok_or_else(|| format!("dyn.field could not find field {name:?}"))?;
            let child_desc = match kind.as_str() {
                "Struct" => {
                    let DecodedValue::Dict(fields) = type_field("fields")?.value() else {
                        return Err("Struct.fields descriptor must be a Dict".into());
                    };
                    view.dict_get_text(fields, name)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("Struct has no declared field {name:?}"))?
                }
                "Dict" => type_field("item")?,
                _ => return Err(format!("dyn.field does not support descriptor kind {kind}")),
            };
            Ok(DynObservation::Child(child_desc, child_value))
        }
        CoreDynFunction::Fields => {
            let value_fields = match value.value() {
                DecodedValue::Dict(value_handle) => {
                    let (fields, values) = view
                        .dict_parts(value_handle)
                        .map_err(|error| error.to_string())?;
                    fields
                        .iter()
                        .zip(values)
                        .map(|(name, value)| {
                            view.text(*name)
                                .map(|name| (name.to_owned(), *value))
                                .map_err(|error| error.to_string())
                        })
                        .collect::<Result<Vec<_>, String>>()?
                }
                DecodedValue::Module(value_handle) => view
                    .module_fields(value_handle)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .map(|name| {
                        view.module_get_text(value_handle, name)
                            .map_err(|error| error.to_string())?
                            .map(|value| (name.to_owned(), value))
                            .ok_or_else(|| "Module export disappeared while iterating".to_owned())
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                _ => return Err(format!("dyn.fields expected {kind} runtime record")),
            };
            let descriptors = match kind.as_str() {
                "Struct" => {
                    let DecodedValue::Dict(fields) = type_field("fields")?.value() else {
                        return Err("Struct.fields descriptor must be a Dict".into());
                    };
                    let (names, descriptors) =
                        view.dict_parts(fields).map_err(|error| error.to_string())?;
                    let names = names
                        .iter()
                        .map(|name| view.text(*name).map(str::to_owned))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| error.to_string())?;
                    if names
                        != value_fields
                            .iter()
                            .map(|(name, _)| name.clone())
                            .collect::<Vec<_>>()
                    {
                        return Err(
                            "Struct descriptor and runtime value have different fields".into()
                        );
                    }
                    descriptors.to_vec()
                }
                "Dict" => vec![type_field("item")?; value_fields.len()],
                _ => {
                    return Err(format!(
                        "dyn.fields does not support descriptor kind {kind}"
                    ));
                }
            };
            let fields = value_fields
                .into_iter()
                .zip(descriptors)
                .map(|((name, value), descriptor)| (name, descriptor, value))
                .collect();
            Ok(DynObservation::NamedChildren(fields))
        }
        CoreDynFunction::ArrayItems => {
            if kind != "Array" {
                return Err(format!("dyn.array_items expected Array, got {kind}"));
            }
            let DecodedValue::Array(handle) = value.value() else {
                return Err("dyn.array_items expected runtime Array".into());
            };
            let item = type_field("item")?;
            let values = view
                .sequence(handle, false)
                .map_err(|error| error.to_string())?;
            Ok(DynObservation::Children(
                values.iter().map(|value| (item, *value)).collect(),
            ))
        }
        CoreDynFunction::TupleItems => {
            if kind != "Tuple" {
                return Err(format!("dyn.tuple_items expected Tuple, got {kind}"));
            }
            let DecodedValue::Tuple(handle) = value.value() else {
                return Err("dyn.tuple_items expected runtime Tuple".into());
            };
            let DecodedValue::Array(items) = type_field("items")?.value() else {
                return Err("Tuple.items descriptor must be an Array".into());
            };
            let descriptors = view
                .sequence(items, false)
                .map_err(|error| error.to_string())?;
            let values = view
                .sequence(handle, true)
                .map_err(|error| error.to_string())?;
            if descriptors.len() != values.len() {
                return Err("Tuple descriptor and runtime value have different lengths".into());
            }
            Ok(DynObservation::Children(
                descriptors
                    .iter()
                    .copied()
                    .zip(values.iter().copied())
                    .collect(),
            ))
        }
        CoreDynFunction::Tag => {
            let (tag, _) = dyn_tagged_parts(kind.as_str(), type_handle, value, view)?;
            Ok(DynObservation::Tag(tag))
        }
        CoreDynFunction::Payload => {
            let (_, payload) = dyn_tagged_parts(kind.as_str(), type_handle, value, view)?;
            Ok(DynObservation::Payload(payload))
        }
        _ => unreachable!("only structural operations reach observer"),
    }
}

fn normalize_dyn_descriptor(mut descriptor: Val, view: &HeapView<'_>) -> Result<Val, String> {
    loop {
        descriptor = declared_type_body(descriptor, view)?;
        if let DecodedValue::TypeSlot(handle) = descriptor.value() {
            descriptor = view
                .type_slot(handle)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Dyn descriptor reference is not initialized".to_owned())?;
            continue;
        }
        let DecodedValue::Dict(handle) = descriptor.value() else {
            return Err("Dyn descriptor is not canonical Type metadata".into());
        };
        let kind = view
            .dict_get_text(handle, "kind")
            .map_err(|error| error.to_string())?
            .and_then(|kind| view.atom_text(kind).ok().flatten())
            .ok_or_else(|| "Dyn descriptor is missing an Atom kind".to_owned())?;
        if kind != "WithAttributes" {
            return Ok(descriptor);
        }
        descriptor = view
            .dict_get_text(handle, "inner")
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "WithAttributes descriptor is missing inner".to_owned())?;
    }
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

fn dyn_field_by_index(
    descriptor: Val,
    value: Val,
    index: u32,
    view: &HeapView<'_>,
) -> Result<(Val, Val), String> {
    let descriptor = normalize_dyn_descriptor(descriptor, view)?;
    let value = view
        .unwrap_declared(value)
        .map_err(|error| error.to_string())?;
    let DecodedValue::Dict(type_handle) = descriptor.value() else {
        return Err("Dyn descriptor is not Type metadata".into());
    };
    let kind = view
        .dict_get_text(type_handle, "kind")
        .map_err(|error| error.to_string())?
        .and_then(|kind| view.atom_text(kind).ok().flatten())
        .ok_or_else(|| "Dyn descriptor has no Atom kind".to_owned())?;
    if kind != "Struct" {
        return Err(format!("dyn.get_field_value expects Struct, got {kind}"));
    }
    let DecodedValue::Dict(fields) = view
        .dict_get_text(type_handle, "fields")
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Struct descriptor has no fields".to_owned())?
        .value()
    else {
        return Err("Struct.fields descriptor must be a Dict".into());
    };
    let (field_names, descriptors) = view.dict_parts(fields).map_err(|error| error.to_string())?;
    let DecodedValue::Dict(values) = value.value() else {
        return Err("dyn.get_field_value expects runtime Struct record".into());
    };
    let (value_names, values) = view.dict_parts(values).map_err(|error| error.to_string())?;
    if field_names != value_names || descriptors.len() != values.len() {
        return Err("Struct descriptor and runtime value have different fields".into());
    }
    let index = usize::try_from(index).map_err(|_| "field index exceeds usize".to_owned())?;
    let descriptor = descriptors
        .get(index)
        .copied()
        .ok_or_else(|| format!("Struct field index {index} is out of range"))?;
    Ok((descriptor, values[index]))
}

fn dyn_variant_by_index(
    descriptor: Val,
    value: Val,
    view: &HeapView<'_>,
) -> Result<(u32, Option<(Val, Val)>), String> {
    let descriptor = normalize_dyn_descriptor(descriptor, view)?;
    let value = view
        .unwrap_declared(value)
        .map_err(|error| error.to_string())?;
    let DecodedValue::Dict(type_handle) = descriptor.value() else {
        return Err("Dyn descriptor is not Type metadata".into());
    };
    let kind = view
        .dict_get_text(type_handle, "kind")
        .map_err(|error| error.to_string())?
        .and_then(|kind| view.atom_text(kind).ok().flatten())
        .ok_or_else(|| "Dyn descriptor has no Atom kind".to_owned())?;
    if kind != "Enum" {
        return Err(format!("dyn.get_variant_index expects Enum, got {kind}"));
    }
    let (tag, payload) = dyn_tagged_parts("Enum", type_handle, value, view)?;
    let DecodedValue::Dict(variants) = view
        .dict_get_text(type_handle, "variants")
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Enum descriptor has no variants".to_owned())?
        .value()
    else {
        return Err("Enum.variants descriptor must be a Dict".into());
    };
    let (names, _) = view
        .dict_parts(variants)
        .map_err(|error| error.to_string())?;
    let index = names
        .iter()
        .position(|name| view.text(*name).is_ok_and(|name| name == tag))
        .ok_or_else(|| format!("Enum has no variant {tag:?}"))?;
    let index = u32::try_from(index).map_err(|_| "variant index exceeds u32".to_owned())?;
    Ok((index, payload))
}

fn dyn_tagged_parts(
    kind: &str,
    type_handle: Handle,
    value: Val,
    view: &HeapView<'_>,
) -> Result<(String, Option<(Val, Val)>), String> {
    let runtime = match value.value() {
        DecodedValue::BuiltinAtom(_) | DecodedValue::InlineAtom(_) | DecodedValue::Atom(_) => {
            let tag = view
                .atom_text(value)
                .map_err(|error| error.to_string())?
                .expect("Atom has text")
                .as_str()
                .to_owned();
            (tag, None)
        }
        DecodedValue::Tagged(handle) => {
            let (tag, payload) = view.tagged(handle).map_err(|error| error.to_string())?;
            let tag = view
                .atom_text(tag)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Tagged runtime tag is not an Atom".to_owned())?
                .as_str()
                .to_owned();
            (tag, Some(payload))
        }
        _ => {
            return Err(format!(
                "dyn tagged observer expected Atom or Tagged for {kind}"
            ));
        }
    };
    match kind {
        "Atom" => {
            let expected = view
                .dict_get_text(type_handle, "tag")
                .map_err(|error| error.to_string())?
                .and_then(|tag| view.atom_text(tag).ok().flatten())
                .ok_or_else(|| "Atom descriptor has no tag".to_owned())?;
            if runtime.0 != expected.as_str() || runtime.1.is_some() {
                return Err(format!("expected unit tag '{expected}"));
            }
            Ok((runtime.0, None))
        }
        "Tagged" => {
            let expected = view
                .dict_get_text(type_handle, "tag")
                .map_err(|error| error.to_string())?
                .and_then(|tag| view.atom_text(tag).ok().flatten())
                .ok_or_else(|| "Tagged descriptor has no tag".to_owned())?;
            if runtime.0 != expected.as_str() {
                return Err(format!("expected tag '{expected}"));
            }
            let payload = runtime
                .1
                .ok_or_else(|| format!("tag '{expected} requires a payload"))?;
            let payload_desc = view
                .dict_get_text(type_handle, "payload")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Tagged descriptor has no payload".to_owned())?;
            Ok((runtime.0, Some((payload_desc, payload))))
        }
        "Enum" => {
            let DecodedValue::Dict(variants) = view
                .dict_get_text(type_handle, "variants")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Enum descriptor has no variants".to_owned())?
                .value()
            else {
                return Err("Enum.variants descriptor must be a Dict".into());
            };
            let variant = view
                .dict_get_text(variants, &runtime.0)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("Enum has no variant {:?}", runtime.0))?;
            let inner = strip_runtime_attributes(variant, "Dyn.enum.variant", view)?;
            let unit = view
                .atom_text(inner)
                .ok()
                .flatten()
                .is_some_and(|atom| atom == "None");
            match (unit, runtime.1) {
                (true, None) => Ok((runtime.0, None)),
                (true, Some(_)) => Err(format!("unit variant {:?} has a payload", runtime.0)),
                (false, Some(payload)) => Ok((runtime.0, Some((variant, payload)))),
                (false, None) => Err(format!("variant {:?} requires a payload", runtime.0)),
            }
        }
        _ => Err(format!(
            "dyn tagged observer does not support descriptor kind {kind}"
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_dyn_observation(
    operation: CoreDynFunction,
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
                        scheme: None,
                        origin: None,
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
                                scheme: None,
                                origin: None,
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
                                    scheme: None,
                                    origin: None,
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
                            scheme: None,
                            origin: None,
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
            let rule = operation.name().trim_start_matches("std/");
            let bytes = logical_value_bytes(6)
                .and_then(|bytes| {
                    bytes
                        .checked_add(u64::try_from(message.len() + rule.len()).unwrap_or(u64::MAX))
                        .ok_or_else(|| {
                            NativeError::allocation_limit("Dyn observer error size overflowed")
                        })
                })
                .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
            charge_allocation(account, bytes, function, pc)?;
            let message = Val::new(current.string(Some(background), &message), input.loc());
            let rule = Val::new(current.string(Some(background), rule), input.loc());
            let fields = ["data", "message", "rule"]
                .into_iter()
                .map(|field| current.intern(field))
                .collect();
            let shape = current.intern_shape(fields);
            let blame = Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: vec![input, message, rule].into(),
                })),
                input.loc(),
            );
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Err), input.loc()),
                    payload: blame,
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

fn dyn_descriptor_leaf_kind(mut descriptor: Val, view: &HeapView<'_>) -> Result<String, String> {
    loop {
        descriptor = declared_type_body(descriptor, view)?;
        if let DecodedValue::TypeSlot(handle) = descriptor.value() {
            descriptor = view
                .type_slot(handle)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Dyn descriptor reference is not initialized".to_owned())?;
            continue;
        }
        let DecodedValue::Dict(handle) = descriptor.value() else {
            return Err("Dyn descriptor is not canonical Type metadata".into());
        };
        let kind = view
            .dict_get_text(handle, "kind")
            .map_err(|error| error.to_string())?
            .and_then(|kind| view.atom_text(kind).ok().flatten())
            .ok_or_else(|| "Dyn descriptor is missing an Atom kind".to_owned())?;
        if kind != "WithAttributes" {
            return Ok(kind.as_str().to_owned());
        }
        descriptor = view
            .dict_get_text(handle, "inner")
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "WithAttributes descriptor is missing inner".to_owned())?;
    }
}

fn declared_type_body(value: Val, view: &HeapView<'_>) -> Result<Val, String> {
    let handle = match value.value() {
        DecodedValue::DeclaredType(handle) | DecodedValue::SymbolicType(handle) => handle,
        _ => return Ok(value),
    };
    let body = match view.object(handle).map_err(|error| error.to_string())? {
        Object::DeclaredType { body, .. } | Object::SymbolicType { body, .. } => body,
        _ => return Err("declared Type handle refers to another object kind".into()),
    };
    Ok(*body)
}

fn type_desc_children(input: Val, view: &HeapView<'_>) -> Result<Vec<Val>, String> {
    if matches!(
        input.value(),
        DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_)
    ) {
        return Ok(Vec::new());
    }
    if matches!(input.value(), DecodedValue::NativeType(_)) {
        return Ok(Vec::new());
    }
    if matches!(input.value(), DecodedValue::TypeSlot(_)) {
        return Ok(Vec::new());
    }
    let DecodedValue::Dict(handle) = input.value() else {
        return Err("std/type-desc.children expects Type metadata".into());
    };
    let kind = view
        .dict_get_text(handle, "kind")
        .map_err(|error| error.to_string())?
        .and_then(|value| view.atom_text(value).ok().flatten())
        .ok_or_else(|| "Type metadata is missing an Atom kind".to_owned())?;
    let get = |field: &str| {
        view.dict_get_text(handle, field)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("{kind} Type metadata is missing {field}"))
    };
    match kind.as_str() {
        "TypeOf" => Ok(vec![get("instance")?]),
        "Array" | "Dict" => Ok(vec![get("item")?]),
        "Tagged" => Ok(vec![get("payload")?]),
        "WithAttributes" => Ok(vec![get("inner")?]),
        "Tuple" | "Union" => {
            let field = if kind == "Tuple" { "items" } else { "variants" };
            let DecodedValue::Array(items) = get(field)?.value() else {
                return Err(format!("{kind}.{field} must be an Array"));
            };
            view.sequence(items, false)
                .map(|items| items.to_vec())
                .map_err(|error| error.to_string())
        }
        "Struct" => {
            let DecodedValue::Dict(fields) = get("fields")?.value() else {
                return Err("Struct.fields must be a Dict".into());
            };
            view.dict_parts(fields)
                .map(|(_, values)| values.to_vec())
                .map_err(|error| error.to_string())
        }
        "Enum" => {
            let DecodedValue::Dict(variants) = get("variants")?.value() else {
                return Err("Enum.variants must be a Dict".into());
            };
            let (_, values) = view
                .dict_parts(variants)
                .map_err(|error| error.to_string())?;
            values
                .iter()
                .filter_map(|value| {
                    let stripped = strip_runtime_attributes(*value, "Type.variants", view);
                    match stripped {
                        Ok(inner)
                            if view
                                .atom_text(inner)
                                .ok()
                                .flatten()
                                .is_some_and(|atom| atom == "None") =>
                        {
                            None
                        }
                        Ok(inner) => Some(Ok(inner)),
                        Err(error) => Some(Err(error)),
                    }
                })
                .collect()
        }
        "Any" | "Never" | "Type" | "Dyn" | "Int" | "Float" | "String" | "Bytes" | "Opaque"
        | "Atom" | "Func" | "Bound" | "Named" => Ok(Vec::new()),
        other => Err(format!("unknown Type metadata kind '{other}")),
    }
}

fn type_desc_members(
    input: Val,
    variants: bool,
    view: &HeapView<'_>,
) -> Result<Vec<(String, Option<Val>)>, String> {
    let descriptor = normalize_dyn_descriptor(input, view)?;
    let DecodedValue::Dict(handle) = descriptor.value() else {
        return Err("Type descriptor is not canonical metadata".into());
    };
    let kind = view
        .dict_get_text(handle, "kind")
        .map_err(|error| error.to_string())?
        .and_then(|value| view.atom_text(value).ok().flatten())
        .ok_or_else(|| "Type descriptor has no Atom kind".to_owned())?;
    let expected = if variants { "Enum" } else { "Struct" };
    if kind != expected {
        return Err(format!(
            "std/type-desc.{} expects {expected}, got {kind}",
            if variants { "variants" } else { "fields" }
        ));
    }
    let member_field = if variants { "variants" } else { "fields" };
    let DecodedValue::Dict(members) = view
        .dict_get_text(handle, member_field)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{expected}.{member_field} is missing"))?
        .value()
    else {
        return Err(format!("{expected}.{member_field} must be a Dict"));
    };
    let (names, values) = view
        .dict_parts(members)
        .map_err(|error| error.to_string())?;
    names
        .iter()
        .zip(values)
        .map(|(name, value)| {
            let name = view
                .text(*name)
                .map(str::to_owned)
                .map_err(|error| error.to_string())?;
            if !variants {
                return Ok((name, Some(*value)));
            }
            let inner = strip_runtime_attributes(*value, "Type.variant", view)?;
            let unit = view
                .atom_text(inner)
                .ok()
                .flatten()
                .is_some_and(|atom| atom == "None");
            Ok((name, (!unit).then_some(inner)))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn type_desc_resolve_error(
    input: Val,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let message = "type descriptor is not a recursive reference";
    let rule = "type-desc.resolve";
    let bytes = logical_value_bytes(6)
        .and_then(|bytes| {
            bytes
                .checked_add(u64::try_from(message.len() + rule.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| NativeError::allocation_limit("TypeDesc error size overflowed"))
        })
        .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
    charge_allocation(account, bytes, function, pc)?;
    let message = Val::new(current.string(Some(background), message), input.loc());
    let rule = Val::new(current.string(Some(background), rule), input.loc());
    let fields = ["data", "message", "rule"]
        .into_iter()
        .map(|field| current.intern(field))
        .collect();
    let shape = current.intern_shape(fields);
    let blame = Val::new(
        DecodedValue::Dict(current.allocate(Object::Dict {
            shape,
            values: vec![input, message, rule].into(),
        })),
        input.loc(),
    );
    Ok(VmAction::Return {
        value: Val::new(
            DecodedValue::Tagged(current.allocate(Object::Tagged {
                tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Err), input.loc()),
                payload: blame,
            })),
            input.loc(),
        ),
        return_target,
    })
}
