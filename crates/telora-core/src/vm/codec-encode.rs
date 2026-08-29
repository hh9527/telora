#[allow(clippy::too_many_arguments)]
fn continue_json_encode(
    input: JsonEncodeInput,
    value_owner: Val,
    diagnostic_input: Val,
    return_target: ReturnTarget,
    rule_boundary: Option<crate::Loc>,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let result = match &input {
        JsonEncodeInput::Typed {
            schema,
            properties,
            value,
        } => transform_codec(
            schema,
            properties,
            *value,
            CodecDirection::Encode,
            "$",
            current,
            background,
        ),
        JsonEncodeInput::Dynamic { properties, value } => transform_dynamic_encode(
            *value,
            properties,
            "$",
            current,
            background,
            &mut HashSet::new(),
        )
        .map(|(node, _)| node),
    };
    match result {
        Ok(raw) => continue_codec_displays(
            CodecNode::SemanticValue {
                owner: value_owner,
                raw: Box::new(raw),
            },
            diagnostic_input,
            return_target,
            rule_boundary,
            call_function,
            call_pc,
            current,
            background,
            account,
        ),
        Err(failure) => finish_codec_result(
            Err(failure),
            diagnostic_input,
            return_target,
            &call_function,
            call_pc,
            current,
            background,
            account,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn continue_codec_displays(
    node: CodecNode,
    diagnostic_input: Val,
    return_target: ReturnTarget,
    rule_boundary: Option<crate::Loc>,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let Some((function, descriptor, value)) = first_prepared_display(&node) else {
        return finish_codec_result(
            Ok(node),
            diagnostic_input,
            return_target,
            &call_function,
            call_pc,
            current,
            background,
            account,
        );
    };
    charge_allocation(
        account,
        logical_value_bytes(2).map_err(|native_error| {
            allocation_error(native_error.message, &call_function, call_pc)
        })?,
        &call_function,
        call_pc,
    )?;
    let argument = value.with_value(DecodedValue::Dyn(current.allocate(Object::Dyn {
        identity: Arc::new(()),
        descriptor,
        value,
        scheme: None,
        origin: None,
    })));
    let trace_frame = RuntimeFrame {
        function: call_function.name().to_owned(),
        instruction: call_pc,
        origin: call_function.origin_at(call_pc),
    };
    Ok(VmAction::Call {
        callee: function,
        arguments: vec![argument],
        return_target: ReturnTarget::Native(Box::new(CodecDisplayContinuation {
            node,
            diagnostic_input,
            return_target,
            rule_boundary,
            call_function: Arc::clone(&call_function),
            call_pc,
            trace_frame,
        })),
        call_function,
        call_pc,
        rule_boundary,
    })
}

fn first_prepared_display(node: &CodecNode) -> Option<(Val, Val, Val)> {
    match node {
        CodecNode::PreparedDisplay {
            function,
            descriptor,
            value,
            ..
        } => Some((*function, *descriptor, *value)),
        CodecNode::SemanticValue { raw, .. } => first_prepared_display(raw),
        CodecNode::Declared { payload, .. } => first_prepared_display(payload),
        CodecNode::Array(items, _) | CodecNode::Tuple(items, _) => {
            items.iter().find_map(first_prepared_display)
        }
        CodecNode::Tagged { tag, payload, .. } => {
            first_prepared_display(tag).or_else(|| first_prepared_display(payload))
        }
        CodecNode::Dict(fields, _) => fields
            .iter()
            .find_map(|(_, value)| first_prepared_display(value)),
        CodecNode::Existing(_)
        | CodecNode::Atom(_, _)
        | CodecNode::NamedAtom(_, _)
        | CodecNode::String(_, _) => None,
    }
}

fn replace_first_prepared_display(node: &mut CodecNode, text: String) -> bool {
    match node {
        CodecNode::PreparedDisplay { loc, .. } => {
            *node = CodecNode::String(text, *loc);
            true
        }
        CodecNode::SemanticValue { raw, .. } => replace_first_prepared_display(raw, text),
        CodecNode::Declared { payload, .. } => replace_first_prepared_display(payload, text),
        CodecNode::Array(items, _) | CodecNode::Tuple(items, _) => {
            replace_first_in_nodes(items, text)
        }
        CodecNode::Tagged { tag, payload, .. } => {
            if first_prepared_display(tag).is_some() {
                replace_first_prepared_display(tag, text)
            } else {
                replace_first_prepared_display(payload, text)
            }
        }
        CodecNode::Dict(fields, _) => {
            let Some((_, value)) = fields
                .iter_mut()
                .find(|(_, value)| first_prepared_display(value).is_some())
            else {
                return false;
            };
            replace_first_prepared_display(value, text)
        }
        CodecNode::Existing(_)
        | CodecNode::Atom(_, _)
        | CodecNode::NamedAtom(_, _)
        | CodecNode::String(_, _) => false,
    }
}

fn replace_first_in_nodes(nodes: &mut [CodecNode], text: String) -> bool {
    let Some(node) = nodes
        .iter_mut()
        .find(|node| first_prepared_display(node).is_some())
    else {
        return false;
    };
    replace_first_prepared_display(node, text)
}

fn resume_codec_display(
    mut continuation: CodecDisplayContinuation,
    value: Val,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let text = crate::fmt::render_value(ValueRef::work(value, current, background)).map_err(
        |native_error| {
            error(
                RuntimeErrorKind::TypeMismatch,
                format!("std/fmt.render: {}", native_error.message),
                &continuation.call_function,
                continuation.call_pc,
            )
        },
    )?;
    if !replace_first_prepared_display(&mut continuation.node, text) {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "codec Display continuation has no pending prepared display",
            &continuation.call_function,
            continuation.call_pc,
        ));
    }
    continue_codec_displays(
        continuation.node,
        continuation.diagnostic_input,
        continuation.return_target,
        continuation.rule_boundary,
        continuation.call_function,
        continuation.call_pc,
        current,
        background,
        account,
    )
}

fn transform_dynamic_encode(
    value: Val,
    properties: &CodecProperties,
    path: &str,
    current: &Heap,
    background: &Heap,
    active: &mut HashSet<Handle>,
) -> Result<(CodecNode, bool), CodecFailure> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    if let Some(owner) = view
        .type_witness(value)
        .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
    {
        let schema = decode_runtime_type(owner, current, background)
            .map_err(|message| CodecFailure::new(message, value, owner))?;
        assert_codec_graph_ready(&schema, current, background).map_err(|error| {
            let message = match error {
                CodecGraphError::Pending => {
                    "codec was invoked before recursive type metadata was sealed".into()
                }
                CodecGraphError::Invalid(message) => message,
            };
            CodecFailure::new(message, value, owner)
        })?;
        return transform_codec(
            &schema,
            properties,
            value,
            CodecDirection::Encode,
            path,
            current,
            background,
        )
        .map(|node| (node, true));
    }


    match value.value() {
        DecodedValue::BuiltinAtom(BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False)
        | DecodedValue::Int(_)
        | DecodedValue::InlineString(_)
        | DecodedValue::ShortString(_)
        | DecodedValue::Bytes(_) => Ok((CodecNode::Existing(value), false)),
        DecodedValue::Float(number) if number.is_finite() => {
            Ok((CodecNode::Existing(value), false))
        }
        DecodedValue::Float(_) => Err(CodecFailure::new(
            "semantic Value cannot contain a non-finite Float",
            value,
            value,
        )),
        DecodedValue::Array(handle) => {
            if !active.insert(handle) {
                return Err(CodecFailure::new(
                    "semantic Value cannot contain a cycle",
                    value,
                    value,
                ));
            }
            let result = view
                .sequence(handle, false)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    transform_dynamic_encode(
                        *item,
                        properties,
                        &format!("{path}[{index}]"),
                        current,
                        background,
                        active,
                    )
                })
                .collect::<Result<Vec<_>, _>>();
            active.remove(&handle);
            result.map(|items| {
                if items.iter().any(|(_, transformed)| *transformed) {
                    (
                        CodecNode::Array(
                            items.into_iter().map(|(item, _)| item).collect(),
                            value.loc(),
                        ),
                        true,
                    )
                } else {
                    (CodecNode::Existing(value), false)
                }
            })
        }
        DecodedValue::Dict(handle) => {
            if !active.insert(handle) {
                return Err(CodecFailure::new(
                    "semantic Value cannot contain a cycle",
                    value,
                    value,
                ));
            }
            let (fields, values) = view
                .dict_parts(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?;
            let result = fields
                .iter()
                .zip(values)
                .map(|(field, item)| {
                    let name = view
                        .text(*field)
                        .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                        .to_owned();
                    transform_dynamic_encode(
                        *item,
                        properties,
                        &format!("{path}.{name}"),
                        current,
                        background,
                        active,
                    )
                    .map(|(item, transformed)| (name, item, transformed))
                })
                .collect::<Result<Vec<_>, _>>();
            active.remove(&handle);
            result.map(|fields| {
                if fields.iter().any(|(_, _, transformed)| *transformed) {
                    (
                        CodecNode::Dict(
                            fields
                                .into_iter()
                                .map(|(name, item, _)| (name, item))
                                .collect(),
                            value.loc(),
                        ),
                        true,
                    )
                } else {
                    (CodecNode::Existing(value), false)
                }
            })
        }
        DecodedValue::Tagged(handle) => {
            let (tag, payload) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?;
            if tag.value() == DecodedValue::BuiltinAtom(BuiltinAtom::Some) {
                return transform_dynamic_encode(
                    payload, properties, path, current, background, active,
                )
                .map(|(node, _)| (node, true));
            }
            let temporal = view
                .atom_text(tag)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                .is_some_and(|tag| {
                    matches!(
                        tag.as_str(),
                        "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
                    )
                })
                && view
                    .string_text(payload)
                    .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                    .is_some();
            if temporal {
                Ok((CodecNode::Existing(value), false))
            } else {
                Err(CodecFailure::new(
                    "raw data graph contains unsupported tagged value",
                    value,
                    value,
                ))
            }
        }
        DecodedValue::NativeType(_)
        | DecodedValue::DeclaredType(_)
        | DecodedValue::SymbolicType(_)
        | DecodedValue::TypeSlot(_) => Err(CodecFailure::new(
            "semantic Value cannot encode Type",
            value,
            value,
        )),
        _ => Err(CodecFailure::new(
            format!("raw data graph contains unsupported {:?}", value.value()),
            value,
            value,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_codec_result(
    result: Result<CodecNode, CodecFailure>,
    input: Val,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let (tag, payload) = match result {
        Ok(node) => (BuiltinAtom::Ok, node),
        Err(failure) => {
            let loc = failure.data.loc();
            (
                BuiltinAtom::Err,
                CodecNode::Dict(
                    vec![
                        ("message".into(), CodecNode::String(failure.message, loc)),
                        ("data".into(), CodecNode::Existing(failure.data)),
                        ("rule".into(), CodecNode::Existing(failure.rule)),
                    ],
                    loc,
                ),
            )
        }
    };
    let bytes = codec_node_bytes(&payload, current, background)
        .and_then(|bytes| {
            bytes
                .checked_add(logical_value_bytes(2)?)
                .ok_or_else(|| NativeError::allocation_limit("codec Result size overflowed"))
        })
        .map_err(|native_error| match native_error.limit() {
            Some(_) => allocation_error(native_error.message, function, pc),
            None => error(
                RuntimeErrorKind::TypeMismatch,
                native_error.message,
                function,
                pc,
            ),
        })?;
    charge_allocation(account, bytes, function, pc)?;
    let payload = materialize_codec_node(payload, current, background);
    let value = Val::new(
        DecodedValue::Tagged(current.allocate(Object::Tagged {
            tag: Val::new(DecodedValue::BuiltinAtom(tag), input.loc()),
            payload,
        })),
        input.loc(),
    );
    Ok(VmAction::Return {
        value,
        return_target,
    })
}
