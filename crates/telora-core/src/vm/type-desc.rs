#[allow(clippy::too_many_arguments)]
fn run_core_type_desc(
    operation: CoreTypeDescFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let input = arguments[0];
    let view = HeapView {
        current,
        background: Some(background),
    };
    match operation {
        CoreTypeDescFunction::Kind => {
            let kind = if matches!(
                input.value(),
                DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_)
            ) {
                "Ref".to_owned()
            } else if matches!(input.value(), DecodedValue::NativeType(_)) {
                "Opaque".to_owned()
            } else if matches!(input.value(), DecodedValue::TypeSlot(_)) {
                "Ref".to_owned()
            } else {
                let DecodedValue::Dict(handle) = input.value() else {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        "std/type-desc.kind expects Type metadata",
                        function,
                        pc,
                    ));
                };
                let kind = view
                    .dict_get_text(handle, "kind")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .and_then(|value| view.atom_text(value).ok().flatten())
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "std/type-desc.kind expects canonical Type metadata",
                            function,
                            pc,
                        )
                    })?;
                const KINDS: &[&str] = &[
                    "Any",
                    "Never",
                    "Type",
                    "TypeOf",
                    "Int",
                    "Float",
                    "String",
                    "Bytes",
                    "Opaque",
                    "Atom",
                    "Array",
                    "Dict",
                    "Tagged",
                    "Tuple",
                    "Struct",
                    "Enum",
                    "Union",
                    "Func",
                    "WithAttributes",
                    "Bound",
                    "Named",
                    "Dyn",
                ];
                if !KINDS.contains(&kind.as_str()) {
                    return Err(error(
                        RuntimeErrorKind::TypeMismatch,
                        format!("unknown Type metadata kind '{kind}"),
                        function,
                        pc,
                    ));
                }
                kind.as_str().to_owned()
            };
            Ok(VmAction::Return {
                value: Val::new(DecodedValue::Atom(current.intern(&kind)), input.loc()),
                return_target,
            })
        }
        CoreTypeDescFunction::Children => {
            let children = type_desc_children(input, &view)
                .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
            charge_allocation(
                account,
                logical_value_bytes(children.len())
                    .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
                function,
                pc,
            )?;
            Ok(VmAction::Return {
                value: Val::new(
                    DecodedValue::Array(current.allocate(Object::Array(children.into()))),
                    input.loc(),
                ),
                return_target,
            })
        }
        CoreTypeDescFunction::OpaqueName => {
            let name = if let DecodedValue::NativeType(id) = input.value() {
                Some(
                    view.native_type(id)
                        .map_err(|error| core_dict_heap_error(error, function, pc))?
                        .qualified_name()
                        .to_owned(),
                )
            } else if let DecodedValue::Dict(handle) = input.value() {
                let kind = view
                    .dict_get_text(handle, "kind")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .and_then(|value| view.atom_text(value).ok().flatten());
                if kind.is_some_and(|kind| kind == "Opaque") {
                    view.dict_get_text(handle, "name")
                        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                        .and_then(|value| view.string_text(value).ok().flatten())
                        .map(|text| text.as_str().to_owned())
                } else {
                    None
                }
            } else {
                None
            };
            let value = if let Some(name) = name {
                charge_allocation(
                    account,
                    logical_value_bytes(2)
                        .map_err(|error| allocation_error(error.message, function, pc))?
                        .saturating_add(name.len() as u64),
                    function,
                    pc,
                )?;
                let payload = Val::new(current.string(Some(background), &name), input.loc());
                DecodedValue::Tagged(current.allocate(Object::Tagged {
                    tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Some), input.loc()),
                    payload,
                }))
            } else {
                DecodedValue::BuiltinAtom(BuiltinAtom::None)
            };
            Ok(VmAction::Return {
                value: Val::new(value, input.loc()),
                return_target,
            })
        }
        CoreTypeDescFunction::Resolve => {
            let result = if matches!(
                input.value(),
                DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_)
            ) {
                declared_type_body(input, &view).map_err(|message| {
                    error(RuntimeErrorKind::InvalidBytecode, message, function, pc)
                })?
            } else if let DecodedValue::TypeSlot(handle) = input.value() {
                view.type_slot(handle)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::InvalidBytecode,
                            "recursive Type reference is not initialized",
                            function,
                            pc,
                        )
                    })?
            } else {
                return type_desc_resolve_error(
                    input,
                    return_target,
                    function,
                    pc,
                    current,
                    background,
                    account,
                );
            };
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
                        tag: Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::Ok), input.loc()),
                        payload: result,
                    })),
                    input.loc(),
                ),
                return_target,
            })
        }
    }
}

