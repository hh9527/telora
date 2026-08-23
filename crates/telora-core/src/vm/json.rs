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
        CoreJsonFunction::Parse
            | CoreJsonFunction::ParseYaml
            | CoreJsonFunction::ParseToml
            | CoreJsonFunction::Decode
    ) {
        let (input_index, properties) = if operation == CoreJsonFunction::Decode {
            let properties = decode_codec_properties(arguments[0], current, background)
                .map_err(|message| {
                    error(RuntimeErrorKind::TypeMismatch, message, function, pc)
                })?;
            (2, Some(properties))
        } else {
            (1, None)
        };
        let view = HeapView {
            current,
            background: Some(background),
        };
        let Some(source) = ValueRef {
            value: arguments[input_index],
            view,
        }
        .as_str() else {
            return Err(runtime_type_error(
                "String",
                &arguments[input_index],
                &view,
                function,
                pc,
            ));
        };
        let parsed = match operation {
            CoreJsonFunction::Parse | CoreJsonFunction::Decode => {
                crate::json::parse_json("<json string>", source.as_str())
                    .map_err(|error| error.message)
            }
            CoreJsonFunction::ParseYaml => {
                let mut sources = SourceDatabase::default();
                let source_id = sources.add("<yaml string>", source.as_str());
                let parsed = crate::yaml::parse_yaml_registered(&sources, source_id);
                parsed.value.map(|value| value.value).ok_or_else(|| {
                    parsed.diagnostics.first().map_or_else(
                        || "invalid YAML".into(),
                        |diagnostic| diagnostic.message.clone(),
                    )
                })
            }
            CoreJsonFunction::ParseToml => {
                let mut sources = SourceDatabase::default();
                let source_id = sources.add("<toml string>", source.as_str());
                let parsed = crate::toml::parse_toml_registered(&sources, source_id);
                parsed.value.map(|value| value.value).ok_or_else(|| {
                    parsed.diagnostics.first().map_or_else(
                        || "invalid TOML".into(),
                        |diagnostic| diagnostic.message.clone(),
                    )
                })
            }
            _ => unreachable!(),
        };
        let parsed = match parsed {
            Ok(value) => {
                charge_allocation(account, source.len() as u64, function, pc)?;
                value
                    .relocate_into(current, background)
                    .map_err(|heap_error| {
                        error(
                            RuntimeErrorKind::TypeMismatch,
                            heap_error.to_string(),
                            function,
                            pc,
                        )
                    })?
            }
            Err(parse_error) => {
                let rule = Val::new(
                    current.atom(
                        Some(background),
                        match operation {
                            CoreJsonFunction::ParseYaml => "Yaml",
                            CoreJsonFunction::ParseToml => "Toml",
                            _ => "Json",
                        },
                    ),
                    arguments[input_index].loc(),
                );
                return finish_codec_result(
                    Err(CodecFailure {
                        message: parse_error,
                        data: arguments[input_index],
                        rule,
                    }),
                    arguments[input_index],
                    return_target,
                    function,
                    pc,
                    current,
                    background,
                    account,
                );
            }
        };
        if matches!(
            operation,
            CoreJsonFunction::Parse | CoreJsonFunction::ParseYaml | CoreJsonFunction::ParseToml
        ) {
            let wrapper_bytes = semantic_value_wrapper_bytes(current, Some(background), parsed)
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::TypeMismatch,
                        heap_error.to_string(),
                        function,
                        pc,
                    )
                })?;
            charge_allocation(account, wrapper_bytes, function, pc)?;
            let parsed = wrap_semantic_value(current, Some(background), parsed, arguments[0])
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::TypeMismatch,
                        heap_error.to_string(),
                        function,
                        pc,
                    )
                })?;
            return finish_codec_result(
                Ok(CodecNode::Existing(parsed)),
                arguments[input_index],
                return_target,
                function,
                pc,
                current,
                background,
                account,
            );
        }
        let schema_index = if operation == CoreJsonFunction::Decode { 1 } else { 0 };
        let schema = decode_runtime_type(arguments[schema_index], current, background)
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        assert_codec_graph_ready(&schema, current, background).map_err(|graph_error| {
            match graph_error {
                CodecGraphError::Pending => error(
                    RuntimeErrorKind::UninitializedDefinition,
                    "codec was invoked before recursive type metadata was sealed",
                    function,
                    pc,
                ),
                CodecGraphError::Invalid(message) => {
                    error(RuntimeErrorKind::TypeMismatch, message, function, pc)
                }
            }
        })?;
        let result = transform_codec(
            &schema,
            properties.as_ref().expect("decode initializes codec properties"),
            parsed,
            CodecDirection::Decode,
            "$",
            current,
            background,
        );
        return finish_codec_result(
            result,
            arguments[input_index],
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    if operation == CoreJsonFunction::Schema {
        let properties = decode_codec_properties(arguments[0], current, background)
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        let schema = decode_runtime_type(arguments[1], current, background)
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        assert_codec_graph_ready(&schema, current, background).map_err(|graph_error| {
            match graph_error {
                CodecGraphError::Pending => error(
                    RuntimeErrorKind::UninitializedDefinition,
                    "schema generation was invoked before recursive type metadata was sealed",
                    function,
                    pc,
                ),
                CodecGraphError::Invalid(message) => {
                    error(RuntimeErrorKind::TypeMismatch, message, function, pc)
                }
            }
        })?;
        let mut node = generate_json_schema(&schema, &properties, arguments[1], current, background).map_err(
            |failure| {
                let mut runtime = error(
                    RuntimeErrorKind::TypeMismatch,
                    failure.message,
                    function,
                    pc,
                );
                runtime.set_locations(failure.data.loc(), failure.rule.loc());
                runtime
            },
        )?;
        let CodecNode::Dict(fields, _) = &mut node else {
            unreachable!("root schema is always an object")
        };
        fields.push((
            "$schema".into(),
            CodecNode::String(
                "https://json-schema.org/draft/2020-12/schema".into(),
                arguments[1].loc(),
            ),
        ));
        let bytes = codec_node_bytes(&node, current, background)
            .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
        charge_allocation(account, bytes, function, pc)?;
        let value = materialize_codec_node(node, current, background);
        return Ok(VmAction::Return {
            value,
            return_target,
        });
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
        | CoreJsonFunction::Decode
        | CoreJsonFunction::Schema => unreachable!(),
    };
    let owner = {
        let view = HeapView {
            current,
            background: Some(background),
        };
        propagate_data_failures(&[arguments[0]], &view, function, pc)?;
        view.type_witness(arguments[0])
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
                    "std/json.stringify expects std/value.Value",
                    function,
                    pc,
                )
            })?
    };
    let unwrap_bytes = semantic_value_unwrap_bytes(current, Some(background), arguments[0], owner)
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::TypeMismatch,
                heap_error.to_string(),
                function,
                pc,
            )
        })?;
    charge_allocation(account, unwrap_bytes, function, pc)?;
    let raw = unwrap_semantic_value(current, Some(background), arguments[0], owner).map_err(
        |heap_error| {
            error(
                RuntimeErrorKind::TypeMismatch,
                heap_error.to_string(),
                function,
                pc,
            )
        },
    )?;
    let mut writer = JsonWriter::new(
        HeapView {
            current,
            background: Some(background),
        },
        indent,
    );
    writer
        .value(raw, 0)
        .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
    let output = writer.output;
    charge_allocation(account, output.len() as u64, function, pc)?;
    let value = Val::new(
        current.string(Some(background), &output),
        instruction_location(function, pc),
    );
    Ok(VmAction::Return {
        value,
        return_target,
    })
}
