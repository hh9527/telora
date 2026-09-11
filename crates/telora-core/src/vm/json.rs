#[allow(clippy::too_many_arguments)]
fn run_core_json(
    operation: CoreJsonFunction,
    arguments: &[Val],
    upvalues: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    if matches!(
        operation,
        CoreJsonFunction::Parse | CoreJsonFunction::ParseYaml | CoreJsonFunction::ParseToml
    ) {
        return run_solved_parse(
            operation,
            arguments,
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    if operation == CoreJsonFunction::Schema {
        return run_solved_json_schema(
            arguments,
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    if operation == CoreJsonFunction::StringifyPretty {
        let DecodedValue::Int(indent) = arguments[0].value() else {
            let view = HeapView {
                current,
                background: Some(background),
            };
            return Err(runtime_type_error(
                "Int",
                &arguments[0],
                &view,
                function,
                pc,
            ));
        };
        if !(0..=16).contains(&indent) {
            return Err(error(
                RuntimeErrorKind::TypeMismatch,
                "std/json.stringify_pretty indent must be between 0 and 16",
                function,
                pc,
            ));
        }
        charge_allocation(
            account,
            logical_value_bytes(1).map_err(|e| allocation_error(e.message, function, pc))?,
            function,
            pc,
        )?;
        let closure = Val::new(
            DecodedValue::Func(current.allocate(Object::Closure {
                identity: Arc::new(()),
                prototype: crate::heap::RuntimePrototype::Native(crate::NativeFunction::core_json(
                    CoreJsonFunction::StringifyPrettyValue,
                )),
                upvalues: vec![Val::new(DecodedValue::Int(indent), arguments[0].loc())].into(),
            })),
            instruction_location(function, pc),
        );
        return Ok(VmAction::Return {
            value: closure,
            return_target,
        });
    }
    let indent = match operation {
        CoreJsonFunction::Stringify => None,
        CoreJsonFunction::StringifyPrettyValue => match upvalues {
            [value] if matches!(value.value(), DecodedValue::Int(_)) => {
                let DecodedValue::Int(indent) = value.value() else {
                    unreachable!()
                };
                Some(indent as usize)
            }
            _ => {
                return Err(error(
                    RuntimeErrorKind::InvalidBytecode,
                    "configured JSON formatter has invalid upvalues",
                    function,
                    pc,
                ));
            }
        },
        CoreJsonFunction::StringifyPretty
        | CoreJsonFunction::Parse
        | CoreJsonFunction::ParseYaml
        | CoreJsonFunction::ParseToml
        | CoreJsonFunction::Schema => unreachable!(),
    };
    let types = background.solved_types.as_ref().ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "JSON formatter has no linked type image",
            function,
            pc,
        )
    })?;
    let expected = types.json_value_type.ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "JSON formatter has no statically linked Value contract",
            function,
            pc,
        )
    })?;
    let view = HeapView {
        current,
        background: Some(background),
    };
    propagate_data_failures(&[arguments[0]], &view, function, pc)?;
    let output = write_solved_json(view, arguments[0], expected, indent)
        .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
    charge_allocation(account, output.len() as u64, function, pc)?;
    return Ok(VmAction::Return {
        value: Val::new(
            current.string(Some(background), &output),
            instruction_location(function, pc),
        ),
        return_target,
    });
}
