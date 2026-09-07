fn transform_codec(
    schema: &CodecType,
    properties: &CodecProperties,
    value: Val,
    direction: CodecDirection,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    transform_codec_with_input(schema, properties, value, direction, path, current, background, None)
}

#[allow(clippy::too_many_arguments)]
fn transform_codec_with_input(
    schema: &CodecType,
    properties: &CodecProperties,
    value: Val,
    direction: CodecDirection,
    path: &str,
    current: &Heap,
    background: &Heap,
    input: Option<Val>,
) -> Result<CodecNode, CodecFailure> {
    if matches!(direction, CodecDirection::Decode) {
        return Ok(CodecNode::Decode {
            schema: Box::new(schema.clone()), properties: *properties,
            value, path: path.to_owned(), input,
        });
    }
    transform_codec_inner(schema, properties, value, direction, path, current, background, input)
        .map_err(|mut failure| {
            if failure.input.is_none() {
                failure.input = input;
            }
            failure
        })
}

fn codec_input_field(input: Option<Val>, name: &str, view: &HeapView<'_>) -> Option<Val> {
    let value = ValueRef { value: input?, view: *view };
    value.tagged_parts()?.1.dict_get(name).map(ValueRef::runtime)
}

fn codec_input_item(input: Option<Val>, index: usize, view: &HeapView<'_>) -> Option<Val> {
    let value = ValueRef { value: input?, view: *view };
    value.tagged_parts()?.1.sequence_get(index).map(ValueRef::runtime)
}

#[allow(clippy::too_many_arguments)]
fn transform_codec_inner(
    schema: &CodecType,
    properties: &CodecProperties,
    value: Val,
    direction: CodecDirection,
    path: &str,
    current: &Heap,
    background: &Heap,
    input: Option<Val>,
) -> Result<CodecNode, CodecFailure> {
    if let Some(owner) = schema.declared_owner {
        let view = HeapView {
            current,
            background: Some(background),
        };
        let metadata = ValueRef { value: owner, view };
        let bridged = text_codec_bridge(metadata, properties).map_err(|message| {
            CodecFailure::new(format!("{path}: {message}"), value, schema.rule)
        })?;
        if bridged {
            return match direction {
                CodecDirection::Decode => {
                    let source = view
                        .string_text(value)
                        .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                        .ok_or_else(|| {
                            CodecFailure::new(
                                format!("{path}: expected String text representation"),
                                value,
                                schema.rule,
                            )
                        })?;
                    crate::regex::parse_value(metadata, source.as_str(), properties.parse_by)
                        .map(|parsed| parsed_codec_node(parsed, value.loc()))
                        .map_err(|message| {
                            CodecFailure::new(format!("{path}: {message}"), value, schema.rule)
                        })
                }
                CodecDirection::Encode => {
                    let function = metadata
                        .type_property(properties.display_by)
                        .and_then(|property| property.dict_get("display"))
                        .map(ValueRef::runtime)
                        .ok_or_else(|| {
                            CodecFailure::new(
                                format!("{path}: DisplayBy property has no prepared display"),
                                value,
                                schema.rule,
                            )
                        })?;
                    Ok(CodecNode::PreparedDisplay {
                        function,
                        descriptor: owner,
                        value,
                        loc: value.loc(),
                    })
                }
            };
        }
        let mut structural = schema.clone();
        structural.declared_owner = None;
        apply_codec_type_properties(&mut structural, metadata, properties)
            .map_err(|message| CodecFailure::new(message, value, schema.rule))?;
        return match direction {
            CodecDirection::Decode => transform_codec_with_input(
                &structural,
                properties,
                value,
                direction,
                path,
                current,
                background,
                input,
            )
            .map(|payload| CodecNode::Declared {
                owner,
                payload: Box::new(payload),
                loc: value.loc(),
            }),
            CodecDirection::Encode => {
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                let Some(actual_owner) = view
                    .type_witness(value)
                    .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                else {
                    return Err(CodecFailure::new(
                        format!("{path}: expected a declared value"),
                        value,
                        schema.rule,
                    ));
                };
                let same_owner = view
                    .values_equal(actual_owner, owner)
                    .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
                if !same_owner {
                    return Err(CodecFailure::new(
                        format!("{path}: declared type identity does not match codec"),
                        value,
                        schema.rule,
                    ));
                }
                transform_codec(
                    &structural,
                    properties,
                    value.without_type_id(),
                    direction,
                    path,
                    current,
                    background,
                )
            }
        };
    }
    if option_item(schema).is_some() {
        return transform_codec_field_with_input(
            schema,
            properties,
            value,
            direction,
            path,
            current,
            background,
            input,
        );
    }
    let view = HeapView {
        current,
        background: Some(background),
    };
    match &schema.kind {
        CodecKind::Bound | CodecKind::Named => Err(CodecFailure::new(
            format!("{path}: codec requires a concrete type"),
            value,
            schema.rule,
        )),
        CodecKind::TypeSlot(handle) => {
            let resolved = view
                .type_slot(*handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .ok_or_else(|| {
                    CodecFailure::new("recursive type link is not initialized", value, schema.rule)
                })?;
            let resolved = decode_runtime_type(resolved, current, background)
                .map_err(|message| CodecFailure::new(message, value, schema.rule))?;
            transform_codec_with_input(
                &resolved,
                properties,
                value,
                direction,
                path,
                current,
                background,
                input,
            )
        }
        CodecKind::TypeRef(handle) => {
            let Object::DeclaredType { body, .. } = view
                .object(*handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
            else {
                return Err(CodecFailure::new(
                    "type ref is not sealed",
                    value,
                    schema.rule,
                ));
            };
            let mut resolved = decode_runtime_type(*body, current, background)
                .map_err(|message| CodecFailure::new(message, value, schema.rule))?;
            resolved.declared_owner = Some(Val::unknown(DecodedValue::DeclaredType(*handle)));
            transform_codec_with_input(
                &resolved,
                properties,
                value,
                direction,
                path,
                current,
                background,
                input,
            )
        }
        CodecKind::Type => decode_runtime_type(value, current, background)
            .map(|_| CodecNode::Existing(value))
            .map_err(|message| CodecFailure::new(message, value, schema.rule)),
        CodecKind::Dyn if matches!(value.value(), DecodedValue::Dyn(_)) => {
            Ok(CodecNode::Existing(value))
        }
        CodecKind::Int if matches!(value.value(), DecodedValue::Int(_)) => {
            Ok(CodecNode::Existing(value))
        }
        CodecKind::Float if matches!(value.value(), DecodedValue::Float(_)) => {
            Ok(CodecNode::Existing(value))
        }
        CodecKind::String
            if view
                .string_text(value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .is_some() =>
        {
            Ok(CodecNode::Existing(value))
        }
        CodecKind::Array(item) => {
            let DecodedValue::Array(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected Array"),
                    value,
                    schema.rule,
                ));
            };
            let values = view
                .sequence(handle, false)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .to_vec();
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    transform_codec_with_input(
                        item,
                        properties,
                        value,
                        direction,
                        &format!("{path}[{index}]"),
                        current,
                        background,
                        codec_input_item(input, index, &view),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|items| CodecNode::Array(items, value.loc()))
        }
        CodecKind::Dict(item) => {
            let DecodedValue::Dict(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected Dict"),
                    value,
                    schema.rule,
                ));
            };
            let (names, values) = view
                .dict_parts(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            names
                .iter()
                .zip(values)
                .map(|(name, item_value)| {
                    let name = view
                        .text(*name)
                        .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                        .to_owned();
                    let node = transform_codec_with_input(
                        item,
                        properties,
                        *item_value,
                        direction,
                        &format!("{path}.{name}"),
                        current,
                        background,
                        codec_input_field(input, &name, &view),
                    )?;
                    Ok((name, node))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|fields| CodecNode::Dict(fields, value.loc()))
        }
        CodecKind::Tuple(items) => {
            let (handle, input_is_tuple) = match (direction, value.value()) {
                (CodecDirection::Decode, DecodedValue::Array(handle)) => (handle, false),
                (CodecDirection::Encode, DecodedValue::Tuple(handle)) => (handle, true),
                (CodecDirection::Decode, _) => {
                    return Err(CodecFailure::new(
                        format!("{path}: expected Array"),
                        value,
                        schema.rule,
                    ));
                }
                (CodecDirection::Encode, _) => {
                    return Err(CodecFailure::new(
                        format!("{path}: expected Tuple"),
                        value,
                        schema.rule,
                    ));
                }
            };
            let values = view
                .sequence(handle, input_is_tuple)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .to_vec();
            if values.len() != items.len() {
                return Err(CodecFailure::new(
                    format!("{path}: expected {} items", items.len()),
                    value,
                    schema.rule,
                ));
            }
            let nodes = items
                .iter()
                .zip(values)
                .enumerate()
                .map(|(index, (item, value))| {
                    transform_codec_with_input(
                        item,
                        properties,
                        value,
                        direction,
                        &format!("{path}[{index}]"),
                        current,
                        background,
                        codec_input_item(input, index, &view),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(match direction {
                CodecDirection::Decode => CodecNode::Tuple(nodes, value.loc()),
                CodecDirection::Encode => CodecNode::Array(nodes, value.loc()),
            })
        }
        CodecKind::Newtype(payload) => {
            let raw = match direction {
                CodecDirection::Decode => value,
                CodecDirection::Encode => {
                    let DecodedValue::Tuple(handle) = value.value() else {
                        return Err(CodecFailure::new(
                            format!("{path}: expected newtype payload container"), value, schema.rule,
                        ));
                    };
                    let values = view.sequence(handle, true)
                        .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
                    let [payload] = values else {
                        return Err(CodecFailure::new(
                            format!("{path}: expected one newtype payload"), value, schema.rule,
                        ));
                    };
                    *payload
                }
            };
            let node = transform_codec_with_input(
                payload, properties, raw, direction, path, current, background, input,
            )?;
            Ok(match direction {
                CodecDirection::Decode => CodecNode::Tuple(vec![node], value.loc()),
                CodecDirection::Encode => node,
            })
        }
        CodecKind::Struct(fields) => transform_codec_struct(
            schema,
            properties,
            fields,
            value,
            direction,
            path,
            current,
            background,
            input,
        ),
        CodecKind::Enum(variants) if is_bool_enum(variants) => {
            if matches!(
                value.value(),
                DecodedValue::BuiltinAtom(BuiltinAtom::True | BuiltinAtom::False)
            ) {
                Ok(CodecNode::Existing(value))
            } else {
                Err(CodecFailure::new(
                    format!("{path}: expected Bool"),
                    value,
                    schema.rule,
                ))
            }
        }
        CodecKind::Enum(variants) => transform_codec_enum(
            schema,
            properties,
            variants,
            value,
            direction,
            path,
            current,
            background,
            input,
        ),
        CodecKind::Bytes => Err(CodecFailure::new(
            format!("{path}: Bytes has no JSON codec"),
            value,
            schema.rule,
        )),
        CodecKind::Opaque => Err(CodecFailure::new(
            format!("{path}: Opaque has no JSON codec"),
            value,
            schema.rule,
        )),
        CodecKind::Function => Err(CodecFailure::new(
            format!("{path}: Function has no JSON codec"),
            value,
            schema.rule,
        )),
        _ => Err(CodecFailure::new(
            format!("{path}: expected {}", codec_type_name(schema)),
            value,
            schema.rule,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn transform_codec_struct(
    schema: &CodecType,
    properties: &CodecProperties,
    fields: &BTreeMap<String, CodecType>,
    value: Val,
    direction: CodecDirection,
    path: &str,
    current: &Heap,
    background: &Heap,
    semantic_input: Option<Val>,
) -> Result<CodecNode, CodecFailure> {
    let DecodedValue::Dict(handle) = value.value() else {
        return Err(CodecFailure::new(
            format!("{path}: expected Dict"),
            value,
            schema.rule,
        ));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let (names, values) = view
        .dict_parts(handle)
        .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
    let input = names
        .iter()
        .zip(values)
        .map(|(name, value)| Ok((view.text(*name)?.to_owned(), *value)))
        .collect::<Result<BTreeMap<_, _>, crate::heap::HeapError>>()
        .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
    let plan = plan_struct(schema, fields, value, path, &view)?;
    match direction {
        CodecDirection::Decode => {
            let mut consumed = HashSet::new();
            let output = decode_struct_fields(
                &plan,
                properties,
                &input,
                &mut consumed,
                value,
                path,
                current,
                background,
                semantic_input,
            )?;
            if let Some(unknown) = input.keys().find(|name| !consumed.contains(*name)) {
                let mut failure = CodecFailure::new(
                    format!("{path}.{unknown}: unknown field"),
                    input[unknown],
                    schema.rule,
                );
                failure.input = codec_input_field(semantic_input, unknown, &view);
                return Err(failure);
            }
            Ok(CodecNode::Dict(output, value.loc()))
        }
        CodecDirection::Encode => {
            let mut emitted = BTreeMap::new();
            encode_struct_fields(
                &plan,
                properties,
                &input,
                &mut emitted,
                value,
                path,
                current,
                background,
            )?;
            Ok(CodecNode::Dict(emitted.into_iter().collect(), value.loc()))
        }
    }
}

#[derive(Clone, Debug)]
struct StructFieldPlan {
    internal_name: String,
    external_name: String,
    schema: CodecType,
}

#[derive(Clone, Debug)]
struct StructPlan {
    fields: Vec<StructFieldPlan>,
}
