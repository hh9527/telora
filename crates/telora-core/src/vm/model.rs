#[allow(clippy::too_many_arguments)]
fn run_core_model(
    operation: CoreModelFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    validate_model_context(arguments[0], function, pc, current, background)?;
    if operation == CoreModelFunction::Union {
        return run_core_union_model(
            arguments[1],
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    let member_name = match operation {
        CoreModelFunction::Struct => "fields",
        CoreModelFunction::Enum => "variants",
        CoreModelFunction::Union => unreachable!("Union handled above"),
    };
    let entries = core_dict_entries(
        arguments[1],
        &format!("{member_name} Dict"),
        function,
        pc,
        current,
        background,
    )?;
    if operation == CoreModelFunction::Enum && entries.is_empty() {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            "enum requires at least one variant",
            function,
            pc,
        ));
    }

    let mut normalized = Vec::with_capacity(entries.len());
    for (name, member) in entries {
        let path = format!("{member_name}.{name}");
        match operation {
            CoreModelFunction::Struct => {
                if !matches!(
                    member.value(),
                    DecodedValue::DeclaredType(_)
                        | DecodedValue::SymbolicType(_)
                        | DecodedValue::TypeSlot(_)
                ) {
                    decode_runtime_type_at(member, &path, current, background).map_err(
                        |message| error(RuntimeErrorKind::TypeMismatch, message, function, pc),
                    )?;
                }
            }
            CoreModelFunction::Enum => {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                let unit = view
                    .atom_text(member)
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .is_some_and(|atom| atom == "None");
                if !unit
                    && !matches!(
                        member.value(),
                        DecodedValue::DeclaredType(_)
                            | DecodedValue::SymbolicType(_)
                            | DecodedValue::TypeSlot(_)
                    )
                {
                    decode_runtime_type_at(member, &path, current, background).map_err(
                        |message| error(RuntimeErrorKind::TypeMismatch, message, function, pc),
                    )?;
                }
            }
            CoreModelFunction::Union => unreachable!("Union handled above"),
        }
        normalized.push((name, member));
    }

    let members = allocate_core_dict(normalized, function, pc, current, account)?;
    let kind_name = match operation {
        CoreModelFunction::Struct => "Struct",
        CoreModelFunction::Enum => "Enum",
        CoreModelFunction::Union => unreachable!("Union handled above"),
    };
    let value = allocate_core_dict(
        BTreeMap::from([
            (
                "kind".to_owned(),
                Val::new(
                    DecodedValue::Atom(current.intern(kind_name)),
                    instruction_location(function, pc),
                ),
            ),
            (member_name.to_owned(), members),
        ])
        .into_iter()
        .collect(),
        function,
        pc,
        current,
        account,
    )?;
    Ok(VmAction::Return {
        value,
        return_target,
    })
}
