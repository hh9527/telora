#[allow(clippy::too_many_arguments)]
fn run_core_attributes(
    operation: CoreAttributesFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let (inner, mut attributes) =
        flatten_attributes(arguments[0], "value", function, pc, current, background)?;
    let call_loc = instruction_location(function, pc);
    let value = match operation {
        CoreAttributesFunction::Normalize => allocate_attributes_wrapper(
            inner, attributes, call_loc, function, pc, current, account,
        )?,
        CoreAttributesFunction::Add => {
            let additions = core_dict_entries(
                arguments[1],
                "attributes Dict",
                function,
                pc,
                current,
                background,
            )?;
            for (key, value) in additions {
                attributes.insert(key, value);
            }
            allocate_attributes_wrapper(
                inner, attributes, call_loc, function, pc, current, account,
            )?
        }
        CoreAttributesFunction::Get | CoreAttributesFunction::Has => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            let key = view
                .string_text(arguments[1])
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                .ok_or_else(|| {
                    runtime_type_error("String key", &arguments[1], &view, function, pc)
                })?;
            let found = attributes.get(key.as_str()).copied();
            if operation == CoreAttributesFunction::Has {
                Val::new(
                    DecodedValue::BuiltinAtom(if found.is_some() {
                        BuiltinAtom::True
                    } else {
                        BuiltinAtom::False
                    }),
                    call_loc,
                )
            } else if let Some(payload) = found {
                charge_allocation(
                    account,
                    logical_value_bytes(2)
                        .map_err(|error| allocation_error(error.message, function, pc))?,
                    function,
                    pc,
                )?;
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
        }
        CoreAttributesFunction::All => allocate_core_dict(
            attributes.into_iter().collect(),
            function,
            pc,
            current,
            account,
        )?,
        CoreAttributesFunction::Strip => inner,
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

fn flatten_attributes(
    mut value: Val,
    path: &str,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<(Val, BTreeMap<String, Val>), RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let mut layers = Vec::new();
    while let DecodedValue::Dict(handle) = value.value() {
        let Some(kind) = view
            .dict_get_text(handle, "kind")
            .map_err(|error| core_dict_heap_error(error, function, pc))?
        else {
            break;
        };
        let Some(kind) = view
            .atom_text(kind)
            .map_err(|error| core_dict_heap_error(error, function, pc))?
        else {
            break;
        };
        if kind != "WithAttributes" {
            break;
        }
        let fields = view
            .dict_fields(handle)
            .map_err(|error| core_dict_heap_error(error, function, pc))?;
        if fields != ["attributes", "inner", "kind"] {
            return Err(error(
                RuntimeErrorKind::TypeMismatch,
                format!(
                    "{path} WithAttributes wrapper must have exactly attributes, inner, and kind fields"
                ),
                function,
                pc,
            ));
        }
        let inner = view
            .dict_get_text(handle, "inner")
            .map_err(|error| core_dict_heap_error(error, function, pc))?
            .expect("validated wrapper field");
        let attributes = view
            .dict_get_text(handle, "attributes")
            .map_err(|error| core_dict_heap_error(error, function, pc))?
            .expect("validated wrapper field");
        let DecodedValue::Dict(attributes) = attributes.value() else {
            return Err(error(
                RuntimeErrorKind::TypeMismatch,
                format!("{path}.attributes must be a Dict"),
                function,
                pc,
            ));
        };
        let (names, values) = view
            .dict_parts(attributes)
            .map_err(|error| core_dict_heap_error(error, function, pc))?;
        let layer = names
            .iter()
            .zip(values)
            .map(|(name, value)| {
                Ok((
                    view.text(*name)
                        .map_err(|error| core_dict_heap_error(error, function, pc))?
                        .to_owned(),
                    *value,
                ))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;
        layers.push(layer);
        value = inner;
    }
    let mut merged = BTreeMap::new();
    for layer in layers.into_iter().rev() {
        merged.extend(layer);
    }
    Ok((value, merged))
}

#[allow(clippy::too_many_arguments)]
fn allocate_attributes_wrapper(
    inner: Val,
    attributes: BTreeMap<String, Val>,
    loc: Option<crate::Loc>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    account: &mut QuotaAccount,
) -> Result<Val, RuntimeError> {
    let attributes = allocate_core_dict(
        attributes.into_iter().collect(),
        function,
        pc,
        current,
        account,
    )?;
    allocate_core_dict(
        vec![
            ("attributes".into(), attributes),
            ("inner".into(), inner),
            (
                "kind".into(),
                Val::new(DecodedValue::Atom(current.intern("WithAttributes")), loc),
            ),
        ],
        function,
        pc,
        current,
        account,
    )
}

