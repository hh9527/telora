#[allow(clippy::too_many_arguments)]
fn transform_codec_enum(
    schema: &CodecType,
    properties: &CodecProperties,
    variants: &BTreeMap<String, CodecEnumVariant>,
    value: Val,
    direction: CodecDirection,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let plan = plan_enum(schema, variants, value, path, &view)?;
    if plan.untagged {
        return transform_untagged_enum(
            &plan,
            properties,
            value,
            direction,
            path,
            current,
            background,
        );
    }
    match direction {
        CodecDirection::Decode => {
            if let Some(tag) = view
                .string_text(value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
            {
                let Some(variant) = plan
                    .variants
                    .iter()
                    .find(|variant| variant.external_name == tag)
                else {
                    return Err(CodecFailure::new(
                        format!("{path}: unknown Enum variant {tag:?}"),
                        value,
                        schema.rule,
                    ));
                };
                if variant.payload.is_some() {
                    return Err(CodecFailure::new(
                        format!("{path}: variant {tag:?} requires a payload"),
                        value,
                        variant.rule,
                    ));
                }
                return Ok(CodecNode::NamedAtom(
                    variant.internal_name.clone(),
                    value.loc(),
                ));
            }
            let DecodedValue::Dict(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected an Enum tag String or single-entry Dict"),
                    value,
                    schema.rule,
                ));
            };
            let (names, values) = view
                .dict_parts(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            if names.len() != 1 {
                return Err(CodecFailure::new(
                    format!("{path}: externally tagged Enum object must have one field"),
                    value,
                    schema.rule,
                ));
            }
            let tag = view
                .text(names[0])
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            let Some(variant) = plan
                .variants
                .iter()
                .find(|variant| variant.external_name == tag)
            else {
                return Err(CodecFailure::new(
                    format!("{path}: unknown Enum variant {tag:?}"),
                    value,
                    schema.rule,
                ));
            };
            let Some(payload) = &variant.payload else {
                return Err(CodecFailure::new(
                    format!("{path}: unit variant {tag:?} must be a String"),
                    value,
                    variant.rule,
                ));
            };
            Ok(CodecNode::Tagged {
                tag: Box::new(CodecNode::NamedAtom(
                    variant.internal_name.clone(),
                    value.loc(),
                )),
                payload: Box::new(transform_codec(
                    payload,
                    properties,
                    values[0],
                    direction,
                    &format!("{path}.{tag}"),
                    current,
                    background,
                )?),
                loc: value.loc(),
            })
        }
        CodecDirection::Encode => {
            if let Some(tag) = view
                .atom_text(value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
            {
                let Some(variant) = plan
                    .variants
                    .iter()
                    .find(|variant| variant.internal_name == tag)
                else {
                    return Err(CodecFailure::new(
                        format!("{path}: unknown Enum tag '{tag}"),
                        value,
                        schema.rule,
                    ));
                };
                if variant.payload.is_some() {
                    return Err(CodecFailure::new(
                        format!("{path}: variant '{tag} requires a payload"),
                        value,
                        variant.rule,
                    ));
                }
                return Ok(CodecNode::String(
                    variant.external_name.clone(),
                    value.loc(),
                ));
            }
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected canonical Enum value"),
                    value,
                    schema.rule,
                ));
            };
            let (tag_value, payload_value) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            let tag = view
                .atom_text(tag_value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .ok_or_else(|| {
                    CodecFailure::new(
                        format!("{path}: Enum tuple tag must be an Atom"),
                        value,
                        schema.rule,
                    )
                })?;
            let Some(variant) = plan
                .variants
                .iter()
                .find(|variant| variant.internal_name == tag)
            else {
                return Err(CodecFailure::new(
                    format!("{path}: unknown Enum tag '{tag}"),
                    value,
                    schema.rule,
                ));
            };
            let Some(payload) = &variant.payload else {
                return Err(CodecFailure::new(
                    format!("{path}: unit variant '{tag} must not have a payload"),
                    value,
                    variant.rule,
                ));
            };
            Ok(CodecNode::Dict(
                vec![(
                    variant.external_name.clone(),
                    transform_codec(
                        payload,
                        properties,
                        payload_value,
                        direction,
                        &format!("{path}.{tag}"),
                        current,
                        background,
                    )?,
                )],
                value.loc(),
            ))
        }
    }
}

fn transform_untagged_enum(
    plan: &EnumPlan,
    properties: &CodecProperties,
    value: Val,
    direction: CodecDirection,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    match direction {
        CodecDirection::Decode => {
            let mut matches = Vec::new();
            let mut errors = Vec::new();
            for variant in &plan.variants {
                let payload = variant.payload.as_ref().expect("planned untagged payload");
                match transform_codec(
                    payload,
                    properties,
                    value,
                    direction,
                    path,
                    current,
                    background,
                ) {
                    Ok(node) => matches.push((variant, node)),
                    Err(failure) => errors.push(failure.message),
                }
            }
            match matches.as_slice() {
                [(variant, node)] => Ok(CodecNode::Tagged {
                    tag: Box::new(CodecNode::NamedAtom(
                        variant.internal_name.clone(),
                        value.loc(),
                    )),
                    payload: Box::new(node.clone()),
                    loc: value.loc(),
                }),
                [] => Err(CodecFailure::new(
                    format!(
                        "{path}: value matches no untagged Enum variant ({})",
                        errors.join("; ")
                    ),
                    value,
                    plan.variants
                        .first()
                        .map(|variant| variant.rule)
                        .unwrap_or(value),
                )),
                _ => Err(CodecFailure::new(
                    format!("{path}: value ambiguously matches multiple untagged Enum variants"),
                    value,
                    matches[1].0.rule,
                )),
            }
        }
        CodecDirection::Encode => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected ('Variant, payload)"),
                    value,
                    plan.variants
                        .first()
                        .map(|variant| variant.rule)
                        .unwrap_or(value),
                ));
            };
            let (tag_value, payload_value) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?;
            let tag = view
                .atom_text(tag_value)
                .map_err(|error| CodecFailure::new(error.to_string(), value, value))?
                .ok_or_else(|| {
                    CodecFailure::new(
                        format!("{path}: Enum tuple tag must be an Atom"),
                        value,
                        value,
                    )
                })?;
            let variant = plan
                .variants
                .iter()
                .find(|variant| variant.internal_name == tag)
                .ok_or_else(|| {
                    CodecFailure::new(format!("{path}: unknown Enum tag '{tag}"), value, value)
                })?;
            transform_codec(
                variant.payload.as_ref().expect("planned untagged payload"),
                properties,
                payload_value,
                direction,
                path,
                current,
                background,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_struct_fields(
    plan: &StructPlan,
    properties: &CodecProperties,
    input: &BTreeMap<String, Val>,
    consumed: &mut HashSet<String>,
    container: Val,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<Vec<(String, CodecNode)>, CodecFailure> {
    let mut output = Vec::with_capacity(plan.fields.len());
    for field in &plan.fields {
        let external_path = format!("{path}.{}", field.external_name);
        let node = if let Some(value) = input.get(&field.external_name).copied() {
            consumed.insert(field.external_name.clone());
            transform_codec_field(
                &field.schema,
                properties,
                value,
                CodecDirection::Decode,
                &external_path,
                current,
                background,
            )?
        } else if option_item(&field.schema).is_some() {
            CodecNode::Atom(BuiltinAtom::None, container.loc())
        } else {
            return Err(CodecFailure::new(
                format!("{external_path}: missing required field"),
                container,
                field.schema.rule,
            ));
        };
        output.push((field.internal_name.clone(), node));
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn encode_struct_fields(
    plan: &StructPlan,
    properties: &CodecProperties,
    input: &BTreeMap<String, Val>,
    emitted: &mut BTreeMap<String, CodecNode>,
    container: Val,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<(), CodecFailure> {
    let expected = plan
        .fields
        .iter()
        .map(|field| field.internal_name.as_str())
        .collect::<HashSet<_>>();
    if let Some(unknown) = input.keys().find(|name| !expected.contains(name.as_str())) {
        return Err(CodecFailure::new(
            format!("{path}.{unknown}: unknown internal field"),
            input[unknown],
            plan.fields
                .first()
                .map(|field| field.schema.rule)
                .unwrap_or(container),
        ));
    }
    for field in &plan.fields {
        let field_path = format!("{path}.{}", field.internal_name);
        let Some(value) = input.get(&field.internal_name).copied() else {
            return Err(CodecFailure::new(
                format!("{field_path}: missing required field"),
                container,
                field.schema.rule,
            ));
        };
        let node = transform_codec_field(
            &field.schema,
            properties,
            value,
            CodecDirection::Encode,
            &field_path,
            current,
            background,
        )?;
        if emitted.insert(field.external_name.clone(), node).is_some() {
            return Err(CodecFailure::new(
                format!("{path}.{}: duplicate encoded field", field.external_name),
                value,
                field.schema.rule,
            ));
        }
    }
    Ok(())
}

fn transform_codec_field(
    schema: &CodecType,
    properties: &CodecProperties,
    value: Val,
    direction: CodecDirection,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    let Some(item) = option_item(schema) else {
        return transform_codec(
            schema,
            properties,
            value,
            direction,
            path,
            current,
            background,
        );
    };
    if value.value() == DecodedValue::BuiltinAtom(BuiltinAtom::None) {
        return Ok(CodecNode::Atom(BuiltinAtom::None, value.loc()));
    }
    match direction {
        CodecDirection::Decode => Ok(CodecNode::Tagged {
            tag: Box::new(CodecNode::Atom(BuiltinAtom::Some, value.loc())),
            payload: Box::new(transform_codec(
                item,
                properties,
                value,
                direction,
                path,
                current,
                background,
            )?),
            loc: value.loc(),
        }),
        CodecDirection::Encode => {
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err(CodecFailure::new(
                    format!("{path}: expected Option"),
                    value,
                    schema.rule,
                ));
            };
            let view = HeapView {
                current,
                background: Some(background),
            };
            let (tag, payload) = view
                .tagged(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?;
            if view
                .atom_text(tag)
                .map_err(|error| CodecFailure::new(error.to_string(), value, schema.rule))?
                .is_none_or(|tag| tag != "Some")
            {
                return Err(CodecFailure::new(
                    format!("{path}: expected Option"),
                    value,
                    schema.rule,
                ));
            }
            transform_codec(
                item,
                properties,
                payload,
                direction,
                path,
                current,
                background,
            )
        }
    }
}
