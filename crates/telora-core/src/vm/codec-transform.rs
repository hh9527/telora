fn transform_codec(
    schema: &CodecType,
    value: Val,
    direction: CodecDirection,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    if let Some(owner) = schema.declared_owner {
        let mut structural = schema.clone();
        structural.declared_owner = None;
        return match direction {
            CodecDirection::Decode => transform_codec(
                &structural,
                value,
                direction,
                path,
                predicate_decisions,
                current,
                background,
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
                    value.without_type_id(),
                    direction,
                    path,
                    predicate_decisions,
                    current,
                    background,
                )
            }
        };
    }
    if option_item(schema).is_some() {
        return transform_codec_field(
            schema,
            value,
            direction,
            path,
            predicate_decisions,
            current,
            background,
        );
    }
    let view = HeapView {
        current,
        background: Some(background),
    };
    if !matches!(schema.kind, CodecKind::TypeSlot(_) | CodecKind::TypeRef(_)) {
        let bridged = text_codec_bridge(schema, &view).map_err(|message| {
            CodecFailure::new(format!("{path}: {message}"), value, schema.rule)
        })?;
        if bridged {
            let metadata = ValueRef {
                value: schema.rule,
                view,
            };
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
                    crate::regex::parse_value(metadata, source.as_str())
                        .map(|parsed| parsed_codec_node(parsed, value.loc()))
                        .map_err(|message| {
                            CodecFailure::new(format!("{path}: {message}"), value, schema.rule)
                        })
                }
                CodecDirection::Encode => {
                    crate::fmt::display_value(metadata, ValueRef { value, view })
                        .map(|text| CodecNode::String(text, value.loc()))
                        .map_err(|error| {
                            CodecFailure::new(
                                format!("{path}: {}", error.message),
                                value,
                                schema.rule,
                            )
                        })
                }
            };
        }
    }
    match &schema.kind {
        CodecKind::TypeSlot(handle) => {
            let resolved = view
                .type_slot(*handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .ok_or_else(|| {
                    CodecFailure::new("recursive type link is not initialized", value, schema.rule)
                })?;
            let resolved = decode_runtime_type(resolved, current, background)
                .map_err(|message| CodecFailure::new(message, value, schema.rule))?;
            transform_codec(
                &resolved,
                value,
                direction,
                path,
                predicate_decisions,
                current,
                background,
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
            transform_codec(
                &resolved,
                value,
                direction,
                path,
                predicate_decisions,
                current,
                background,
            )
        }
        CodecKind::Any => Ok(CodecNode::Existing(value)),
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
        CodecKind::Atom(expected) => {
            let actual = view
                .atom_text(value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            if actual.is_some_and(|actual| actual.as_str() == expected) {
                Ok(CodecNode::Existing(value))
            } else {
                Err(CodecFailure::new(
                    format!("{path}: expected '{expected}"),
                    value,
                    schema.rule,
                ))
            }
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
                    transform_codec(
                        item,
                        value,
                        direction,
                        &format!("{path}[{index}]"),
                        predicate_decisions,
                        current,
                        background,
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
                    let node = transform_codec(
                        item,
                        *item_value,
                        direction,
                        &format!("{path}.{name}"),
                        predicate_decisions,
                        current,
                        background,
                    )?;
                    Ok((name, node))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|fields| CodecNode::Dict(fields, value.loc()))
        }
        CodecKind::Tagged { tag, payload } => {
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected '{tag}(payload)"),
                    value,
                    schema.rule,
                ));
            };
            let (actual_tag, actual_payload) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            if view
                .atom_text(actual_tag)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .is_none_or(|actual| actual.as_str() != tag)
            {
                return Err(CodecFailure::new(
                    format!("{path}: expected tag '{tag}"),
                    value,
                    schema.rule,
                ));
            }
            Ok(CodecNode::Tagged {
                tag: Box::new(CodecNode::NamedAtom(tag.clone(), value.loc())),
                payload: Box::new(transform_codec(
                    payload,
                    actual_payload,
                    direction,
                    path,
                    predicate_decisions,
                    current,
                    background,
                )?),
                loc: value.loc(),
            })
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
                    transform_codec(
                        item,
                        value,
                        direction,
                        &format!("{path}[{index}]"),
                        predicate_decisions,
                        current,
                        background,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(match direction {
                CodecDirection::Decode => CodecNode::Tuple(nodes, value.loc()),
                CodecDirection::Encode => CodecNode::Array(nodes, value.loc()),
            })
        }
        CodecKind::Struct(fields) => transform_codec_struct(
            schema,
            fields,
            value,
            direction,
            path,
            predicate_decisions,
            current,
            background,
        ),
        CodecKind::Union(variants) => {
            let mut errors = Vec::new();
            for variant in variants {
                match transform_codec(
                    variant,
                    value,
                    direction,
                    path,
                    predicate_decisions,
                    current,
                    background,
                ) {
                    Ok(node) => return Ok(node),
                    Err(failure) if failure.predicate.is_some() => return Err(failure),
                    Err(failure) => errors.push(failure.message),
                }
            }
            Err(CodecFailure::new(
                format!(
                    "{path}: value matches no Union variant ({})",
                    errors.join("; ")
                ),
                value,
                schema.rule,
            ))
        }
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
            variants,
            value,
            direction,
            path,
            predicate_decisions,
            current,
            background,
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

fn validate_codec_value_without_skipping(
    schema: &CodecType,
    value: Val,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    let mut decisions = BTreeMap::new();
    loop {
        match transform_codec(
            schema,
            value,
            CodecDirection::Encode,
            path,
            &decisions,
            current,
            background,
        ) {
            Ok(node) => return Ok(node),
            Err(failure) => {
                let Some(request) = &failure.predicate else {
                    return Err(failure);
                };
                decisions.insert(request.path.clone(), false);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn transform_codec_struct(
    schema: &CodecType,
    fields: &BTreeMap<String, CodecType>,
    value: Val,
    direction: CodecDirection,
    path: &str,
    predicate_decisions: &BTreeMap<String, bool>,
    current: &Heap,
    background: &Heap,
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
                &input,
                &mut consumed,
                value,
                path,
                predicate_decisions,
                current,
                background,
            )?;
            if let Some(unknown) = input.keys().find(|name| !consumed.contains(*name)) {
                return Err(CodecFailure::new(
                    format!("{path}.{unknown}: unknown field"),
                    input[unknown],
                    schema.rule,
                ));
            }
            Ok(CodecNode::Dict(output, value.loc()))
        }
        CodecDirection::Encode => {
            let mut emitted = BTreeMap::new();
            encode_struct_fields(
                &plan,
                &input,
                &mut emitted,
                value,
                path,
                predicate_decisions,
                current,
                background,
            )?;
            Ok(CodecNode::Dict(emitted.into_iter().collect(), value.loc()))
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum SkipPolicy {
    None,
    False,
    Empty,
    Function(Val),
}

#[derive(Clone, Debug)]
struct StructFieldPlan {
    internal_name: String,
    external_name: Option<String>,
    schema: CodecType,
    flattened: Option<Box<StructPlan>>,
    default: Option<Val>,
    skip: Option<SkipPolicy>,
    config_rule: Val,
}

#[derive(Clone, Debug)]
struct StructPlan {
    fields: Vec<StructFieldPlan>,
}

