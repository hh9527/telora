fn run_core_result(
    _operation: CoreResultFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<VmAction, RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let DecodedValue::Tagged(handle) = arguments[0].value() else {
        return Err(runtime_type_error(
            "'Ok(value) or 'Err(message)",
            &arguments[0],
            &view,
            function,
            pc,
        ));
    };
    let (tag, payload) = view
        .tagged(handle)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
    let tag = view.atom_text(tag).map_err(|heap_error| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            heap_error.to_string(),
            function,
            pc,
        )
    })?;
    match tag.as_ref().map(crate::TextRef::as_str) {
        Some("Ok") => Ok(VmAction::Return {
            value: payload,
            return_target,
        }),
        Some("Err") => {
            let (message, data_location, rule_location) = if let Some(message) =
                view.string_text(payload).map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        pc,
                    )
                })? {
                (message.as_str().to_owned(), payload.loc(), None)
            } else if let DecodedValue::Dict(handle) = payload.value() {
                let message = view
                    .dict_get_text(handle, "message")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .and_then(|message| view.string_text(message).ok().flatten())
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "structured Err payload message must be a String",
                            function,
                            pc,
                        )
                    })?
                    .as_str()
                    .to_owned();
                let data = view
                    .dict_get_text(handle, "data")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "structured Err payload is missing data",
                            function,
                            pc,
                        )
                    })?;
                let rule = view
                    .dict_get_text(handle, "rule")
                    .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "structured Err payload is missing rule",
                            function,
                            pc,
                        )
                    })?;
                (message, data.loc(), rule.loc())
            } else {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/result.unwrap Err payload must be a String or diagnostic Dict",
                    function,
                    pc,
                ));
            };
            let mut runtime_error = error(RuntimeErrorKind::TypeMismatch, message, function, pc);
            runtime_error.set_locations(data_location, rule_location);
            Err(runtime_error)
        }
        _ => Err(error(
            RuntimeErrorKind::TypeMismatch,
            "std/result.unwrap expects 'Ok(value) or 'Err(message)",
            function,
            pc,
        )),
    }
}

