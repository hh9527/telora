#[allow(clippy::too_many_arguments)]
fn run_core_codec(
    operation: CoreCodecFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    rule_boundary: Option<crate::Loc>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let properties = decode_codec_properties(arguments[0], current, background)
        .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
    let direction = match operation {
        CoreCodecFunction::Decode => CodecDirection::Decode,
        CoreCodecFunction::Encode => CodecDirection::Encode,
    };
    if matches!(direction, CodecDirection::Encode) {
        let source_owner = {
            let view = HeapView {
                current,
                background: Some(background),
            };
            propagate_data_failures(&[arguments[2]], &view, function, pc)?;
            view.type_witness(arguments[2]).map_err(|heap_error| {
                error(
                    RuntimeErrorKind::TypeMismatch,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            })?
        };
        if source_owner.is_none() {
            return continue_json_encode(
                JsonEncodeInput::Dynamic {
                    properties,
                    value: arguments[2],
                },
                arguments[1],
                arguments[2],
                return_target,
                rule_boundary,
                Arc::new(function.clone()),
                pc,
                current,
                background,
                account,
            );
        }
    }
    let (schema_owner, value_owner) = {
        let view = HeapView {
            current,
            background: Some(background),
        };
        propagate_data_failures(&[arguments[2]], &view, function, pc)?;
        match direction {
            CodecDirection::Decode => {
                let owner = view
                    .type_witness(arguments[2])
                    .map_err(|heap_error| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            heap_error.to_string(),
                            function,
                            pc,
                        )
                    })?
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            "std/codec.decode expects std/value.Value input",
                            function,
                            pc,
                        )
                    })?;
                (arguments[1], owner)
            }
            CodecDirection::Encode => {
                let owner = view
                    .type_witness(arguments[2])
                    .map_err(|heap_error| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            heap_error.to_string(),
                            function,
                            pc,
                        )
                    })?
                    .expect("unowned encode inputs returned above");
                (owner, arguments[1])
            }
        }
    };
    let identity = {
        let view = HeapView {
            current,
            background: Some(background),
        };
        matches!(
            (
                view.declared_type_id(schema_owner),
                view.declared_type_id(value_owner),
            ),
            (Ok(schema), Ok(value)) if schema == value
        )
    };
    if identity {
        return finish_codec_result(
            Ok(CodecNode::Existing(arguments[2])),
            arguments[2],
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    let schema = decode_runtime_type(schema_owner, current, background)
        .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
    assert_codec_graph_ready(&schema, current, background).map_err(
        |graph_error| match graph_error {
            CodecGraphError::Pending => error(
                RuntimeErrorKind::UninitializedDefinition,
                "codec was invoked before recursive type metadata was sealed",
                function,
                pc,
            ),
            CodecGraphError::Invalid(message) => {
                error(RuntimeErrorKind::TypeMismatch, message, function, pc)
            }
        },
    )?;
    if matches!(direction, CodecDirection::Encode) {
        return continue_json_encode(
            JsonEncodeInput::Typed {
                schema,
                properties,
                value: arguments[2],
            },
            value_owner,
            arguments[2],
            return_target,
            rule_boundary,
            Arc::new(function.clone()),
            pc,
            current,
            background,
            account,
        );
    }
    let unwrap_bytes =
        semantic_value_unwrap_bytes(current, Some(background), arguments[2], value_owner).map_err(
            |heap_error| {
                error(
                    RuntimeErrorKind::TypeMismatch,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            },
        )?;
    charge_allocation(account, unwrap_bytes, function, pc)?;
    let raw = unwrap_semantic_value(current, Some(background), arguments[2], value_owner).map_err(
        |heap_error| {
            error(
                RuntimeErrorKind::TypeMismatch,
                heap_error.to_string(),
                function,
                pc,
            )
        },
    )?;
    let result = transform_codec(
        &schema,
        &properties,
        raw,
        direction,
        "$",
        current,
        background,
    );
    finish_codec_result(
        result,
        arguments[2],
        return_target,
        function,
        pc,
        current,
        background,
        account,
    )
}
