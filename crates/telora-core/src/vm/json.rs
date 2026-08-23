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
        let input_index = 1;
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
                        predicate: None,
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
        let schema = decode_runtime_type(arguments[0], current, background)
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
            parsed,
            CodecDirection::Decode,
            "$",
            &BTreeMap::new(),
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
    if matches!(
        operation,
        CoreJsonFunction::Rename
            | CoreJsonFunction::RenameAll
            | CoreJsonFunction::Default
            | CoreJsonFunction::SkipSerializingIf
    ) {
        validate_json_attribute_configuration(
            operation,
            arguments[0],
            function,
            pc,
            current,
            background,
        )?;
        let configured = match operation {
            CoreJsonFunction::Rename => CoreJsonFunction::RenameDecorator,
            CoreJsonFunction::RenameAll => CoreJsonFunction::RenameAllDecorator,
            CoreJsonFunction::Default => CoreJsonFunction::DefaultDecorator,
            CoreJsonFunction::SkipSerializingIf => CoreJsonFunction::SkipSerializingIfDecorator,
            _ => unreachable!(),
        };
        charge_allocation(
            account,
            logical_value_bytes(1)
                .map_err(|error| allocation_error(error.message, function, pc))?,
            function,
            pc,
        )?;
        let value = Val::new(
            DecodedValue::Func(current.allocate(Object::Closure {
                identity: Arc::new(()),
                prototype: crate::heap::RuntimePrototype::Native(crate::NativeFunction::core_json(
                    configured,
                )),
                upvalues: vec![arguments[0]].into(),
            })),
            instruction_location(function, pc),
        );
        return Ok(VmAction::Return {
            value,
            return_target,
        });
    }
    if operation == CoreJsonFunction::Schema {
        let schema = decode_runtime_type(arguments[0], current, background)
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
        let mut node = generate_json_schema(&schema, arguments[0], current, background).map_err(
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
                arguments[0].loc(),
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
    if matches!(
        operation,
        CoreJsonFunction::Flatten
            | CoreJsonFunction::Untagged
            | CoreJsonFunction::RenameDecorator
            | CoreJsonFunction::RenameAllDecorator
            | CoreJsonFunction::DefaultDecorator
            | CoreJsonFunction::SkipSerializingIfDecorator
    ) {
        let (key, payload) = match operation {
            CoreJsonFunction::Flatten => (
                "std/json.flatten",
                Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::True),
                    instruction_location(function, pc),
                ),
            ),
            CoreJsonFunction::Untagged => (
                "std/json.untagged",
                Val::new(
                    DecodedValue::BuiltinAtom(BuiltinAtom::True),
                    instruction_location(function, pc),
                ),
            ),
            CoreJsonFunction::RenameDecorator => (
                "std/json.rename",
                configured_json_attribute(upvalues, function, pc)?,
            ),
            CoreJsonFunction::RenameAllDecorator => (
                "std/json.rename_all",
                configured_json_attribute(upvalues, function, pc)?,
            ),
            CoreJsonFunction::DefaultDecorator => (
                "std/json.default",
                configured_json_attribute(upvalues, function, pc)?,
            ),
            CoreJsonFunction::SkipSerializingIfDecorator => (
                "std/json.skip_serializing_if",
                configured_json_attribute(upvalues, function, pc)?,
            ),
            _ => unreachable!(),
        };
        let (inner, mut attributes) = flatten_attributes(
            arguments[1],
            "decorated value",
            function,
            pc,
            current,
            background,
        )?;
        attributes.insert(key.to_owned(), payload);
        let value = allocate_attributes_wrapper(
            inner,
            attributes,
            instruction_location(function, pc),
            function,
            pc,
            current,
            account,
        )?;
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
        | CoreJsonFunction::Rename
        | CoreJsonFunction::RenameDecorator
        | CoreJsonFunction::RenameAll
        | CoreJsonFunction::RenameAllDecorator
        | CoreJsonFunction::Flatten
        | CoreJsonFunction::Untagged
        | CoreJsonFunction::Schema
        | CoreJsonFunction::Default
        | CoreJsonFunction::DefaultDecorator
        | CoreJsonFunction::SkipSerializingIf
        | CoreJsonFunction::SkipSerializingIfDecorator => unreachable!(),
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

fn configured_json_attribute(
    upvalues: &[Val],
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    match upvalues {
        [payload] => Ok(*payload),
        _ => Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "configured JSON decorator has invalid upvalues",
            function,
            pc,
        )),
    }
}

fn validate_json_attribute_configuration(
    operation: CoreJsonFunction,
    payload: Val,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<(), RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let valid = match operation {
        CoreJsonFunction::Rename => view
            .string_text(payload)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            .is_some(),
        CoreJsonFunction::RenameAll => view
            .atom_text(payload)
            .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
            .is_some_and(|atom| atom == "CamelCase"),
        CoreJsonFunction::Default => true,
        CoreJsonFunction::SkipSerializingIf => {
            view.atom_text(payload)
                .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
                .is_some_and(|atom| matches!(atom.as_str(), "None" | "False" | "Empty"))
                || view
                    .resolved_function_arity(payload)
                    .is_ok_and(|arity| arity == Some(1))
        }
        _ => unreachable!(),
    };
    if valid {
        return Ok(());
    }
    let message = match operation {
        CoreJsonFunction::Rename => "std/json.rename expects a String",
        CoreJsonFunction::RenameAll => "std/json.rename_all currently expects 'CamelCase",
        CoreJsonFunction::SkipSerializingIf => {
            "std/json.skip_serializing_if expects 'None, 'False, 'Empty, or a unary Func"
        }
        _ => unreachable!(),
    };
    Err(error(RuntimeErrorKind::TypeMismatch, message, function, pc))
}

