fn schema_dict(fields: Vec<(&str, CodecNode)>, loc: Option<crate::Loc>) -> CodecNode {
    CodecNode::Dict(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
        loc,
    )
}

fn schema_string(value: &str, loc: Option<crate::Loc>) -> CodecNode {
    CodecNode::String(value.to_owned(), loc)
}

type SchemaProperties = Vec<(String, CodecNode)>;

fn generate_json_schema(
    schema: &CodecType,
    properties: &CodecProperties,
    data: Val,
    current: &Heap,
    background: &Heap,
) -> Result<CodecNode, CodecFailure> {
    let mut links = HashMap::new();
    let mut definitions = BTreeMap::new();
    let mut root = generate_json_schema_node(
        schema,
        properties,
        data,
        current,
        background,
        &mut links,
        &mut definitions,
    )?;
    if !definitions.is_empty() {
        let CodecNode::Dict(fields, _) = &mut root else {
            unreachable!("every generated schema is an object")
        };
        fields.push((
            "$defs".into(),
            CodecNode::Dict(definitions.into_iter().collect(), schema.rule.loc()),
        ));
    }
    Ok(root)
}

#[allow(clippy::too_many_arguments)]
fn generate_json_schema_node(
    schema: &CodecType,
    properties: &CodecProperties,
    data: Val,
    current: &Heap,
    background: &Heap,
    links: &mut HashMap<Handle, String>,
    definitions: &mut BTreeMap<String, CodecNode>,
) -> Result<CodecNode, CodecFailure> {
    let loc = schema.rule.loc();
    if let Some(owner) = schema.declared_owner {
        let view = HeapView {
            current,
            background: Some(background),
        };
        let metadata = ValueRef { value: owner, view };
        if text_codec_bridge(metadata, properties)
            .map_err(|message| CodecFailure::new(message, data, schema.rule))?
        {
            return Ok(schema_dict(
                vec![("type", schema_string("string", loc))],
                loc,
            ));
        }
        let mut structural = schema.clone();
        structural.declared_owner = None;
        apply_codec_type_properties(&mut structural, metadata, properties)
            .map_err(|message| CodecFailure::new(message, data, schema.rule))?;
        return generate_json_schema_node(
            &structural,
            properties,
            data,
            current,
            background,
            links,
            definitions,
        );
    }
    if let Some(item) = option_item(schema) {
        return Ok(schema_dict(
            vec![(
                "anyOf",
                CodecNode::Array(
                    vec![
                        schema_dict(vec![("type", schema_string("null", loc))], loc),
                        generate_json_schema_node(
                            item,
                            properties,
                            data,
                            current,
                            background,
                            links,
                            definitions,
                        )?,
                    ],
                    loc,
                ),
            )],
            loc,
        ));
    }
    match &schema.kind {
        CodecKind::TypeSlot(handle) => {
            if let Some(name) = links.get(handle) {
                return Ok(schema_dict(
                    vec![("$ref", schema_string(&format!("#/$defs/{name}"), loc))],
                    loc,
                ));
            }
            let name = format!("Type{}", links.len());
            links.insert(*handle, name.clone());
            let view = HeapView {
                current,
                background: Some(background),
            };
            let resolved = view
                .type_slot(*handle)
                .map_err(|error| CodecFailure::new(error.to_string(), data, schema.rule))?
                .ok_or_else(|| {
                    CodecFailure::new("recursive type link is not initialized", data, schema.rule)
                })?;
            let resolved = decode_runtime_type(resolved, current, background)
                .map_err(|message| CodecFailure::new(message, data, schema.rule))?;
            let definition = generate_json_schema_node(
                &resolved,
                properties,
                data,
                current,
                background,
                links,
                definitions,
            )?;
            definitions.insert(name.clone(), definition);
            Ok(schema_dict(
                vec![("$ref", schema_string(&format!("#/$defs/{name}"), loc))],
                loc,
            ))
        }
        CodecKind::TypeRef(handle) => {
            if let Some(name) = links.get(handle) {
                return Ok(schema_dict(
                    vec![("$ref", schema_string(&format!("#/$defs/{name}"), loc))],
                    loc,
                ));
            }
            let name = format!("Type{}", links.len());
            links.insert(*handle, name.clone());
            let view = HeapView {
                current,
                background: Some(background),
            };
            let metadata = ValueRef {
                value: Val::unknown(DecodedValue::DeclaredType(*handle)),
                view,
            };
            let bridged = text_codec_bridge(metadata, properties)
                .map_err(|message| CodecFailure::new(message, data, schema.rule))?;
            if bridged {
                return Ok(schema_dict(
                    vec![("type", schema_string("string", loc))],
                    loc,
                ));
            }
            let Object::DeclaredType { body, .. } = view
                .object(*handle)
                .map_err(|error| CodecFailure::new(error.to_string(), data, schema.rule))?
            else {
                return Err(CodecFailure::new(
                    "type ref is not sealed",
                    data,
                    schema.rule,
                ));
            };
            let mut resolved = decode_runtime_type(*body, current, background)
                .map_err(|message| CodecFailure::new(message, data, schema.rule))?;
            apply_codec_type_properties(&mut resolved, metadata, properties)
                .map_err(|message| CodecFailure::new(message, data, schema.rule))?;
            let definition = generate_json_schema_node(
                &resolved,
                properties,
                data,
                current,
                background,
                links,
                definitions,
            )?;
            definitions.insert(name.clone(), definition);
            Ok(schema_dict(
                vec![("$ref", schema_string(&format!("#/$defs/{name}"), loc))],
                loc,
            ))
        }
        CodecKind::Any => Ok(CodecNode::Dict(Vec::new(), loc)),
        CodecKind::Type => Err(CodecFailure::new(
            "JSON Schema cannot describe Type metadata",
            schema.rule,
            schema.rule,
        )),
        CodecKind::Dyn => Err(CodecFailure::new(
            "JSON Schema cannot describe Dyn",
            schema.rule,
            schema.rule,
        )),
        CodecKind::Int => Ok(schema_dict(
            vec![("type", schema_string("integer", loc))],
            loc,
        )),
        CodecKind::Float => Ok(schema_dict(
            vec![("type", schema_string("number", loc))],
            loc,
        )),
        CodecKind::String => Ok(schema_dict(
            vec![("type", schema_string("string", loc))],
            loc,
        )),
        CodecKind::Atom(tag) if tag == "None" => {
            Ok(schema_dict(vec![("type", schema_string("null", loc))], loc))
        }
        CodecKind::Atom(tag) => Ok(schema_dict(vec![("const", schema_string(tag, loc))], loc)),
        CodecKind::Array(item) => Ok(schema_dict(
            vec![
                ("type", schema_string("array", loc)),
                (
                    "items",
                    generate_json_schema_node(item, properties, data, current, background, links, definitions)?,
                ),
            ],
            loc,
        )),
        CodecKind::Dict(item) => Ok(schema_dict(
            vec![
                ("type", schema_string("object", loc)),
                (
                    "additionalProperties",
                    generate_json_schema_node(item, properties, data, current, background, links, definitions)?,
                ),
            ],
            loc,
        )),
        CodecKind::Tagged { payload, .. } => {
            generate_json_schema_node(payload, properties, data, current, background, links, definitions)
        }
        CodecKind::Tuple(items) => {
            let schemas = items
                .iter()
                .map(|item| {
                    generate_json_schema_node(item, properties, data, current, background, links, definitions)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let length = Val::unknown(DecodedValue::Int(items.len() as i64));
            Ok(schema_dict(
                vec![
                    ("type", schema_string("array", loc)),
                    ("prefixItems", CodecNode::Array(schemas, loc)),
                    ("minItems", CodecNode::Existing(length)),
                    ("maxItems", CodecNode::Existing(length)),
                ],
                loc,
            ))
        }
        CodecKind::Struct(fields) => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            let plan = plan_struct(schema, fields, data, "$", &view)?;
            let (properties, required) = generate_struct_schema_fields(
                &plan,
                properties,
                data,
                current,
                background,
                links,
                definitions,
            )?;
            let mut fields = vec![
                ("type", schema_string("object", loc)),
                ("properties", CodecNode::Dict(properties, loc)),
                (
                    "additionalProperties",
                    CodecNode::Atom(BuiltinAtom::False, loc),
                ),
            ];
            if !required.is_empty() {
                fields.push((
                    "required",
                    CodecNode::Array(
                        required
                            .into_iter()
                            .map(|name| CodecNode::String(name, loc))
                            .collect(),
                        loc,
                    ),
                ));
            }
            Ok(schema_dict(fields, loc))
        }
        CodecKind::Enum(variants) if is_bool_enum(variants) => Ok(schema_dict(
            vec![("type", schema_string("boolean", loc))],
            loc,
        )),
        CodecKind::Enum(variants) => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            let plan = plan_enum(schema, variants, data, "$", &view)?;
            let branches = if plan.untagged {
                plan.variants
                    .iter()
                    .map(|variant| {
                        generate_json_schema_node(
                            variant.payload.as_ref().expect("planned untagged payload"),
                            properties,
                            data,
                            current,
                            background,
                            links,
                            definitions,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                plan.variants
                    .iter()
                    .map(|variant| {
                        if let Some(payload) = &variant.payload {
                            let property = generate_json_schema_node(
                                payload,
                                properties,
                                data,
                                current,
                                background,
                                links,
                                definitions,
                            )?;
                            Ok(schema_dict(
                                vec![
                                    ("type", schema_string("object", variant.rule.loc())),
                                    (
                                        "properties",
                                        CodecNode::Dict(
                                            vec![(variant.external_name.clone(), property)],
                                            variant.rule.loc(),
                                        ),
                                    ),
                                    (
                                        "required",
                                        CodecNode::Array(
                                            vec![schema_string(
                                                &variant.external_name,
                                                variant.rule.loc(),
                                            )],
                                            variant.rule.loc(),
                                        ),
                                    ),
                                    (
                                        "additionalProperties",
                                        CodecNode::Atom(BuiltinAtom::False, variant.rule.loc()),
                                    ),
                                ],
                                variant.rule.loc(),
                            ))
                        } else {
                            Ok(schema_dict(
                                vec![(
                                    "const",
                                    schema_string(&variant.external_name, variant.rule.loc()),
                                )],
                                variant.rule.loc(),
                            ))
                        }
                    })
                    .collect::<Result<Vec<_>, CodecFailure>>()?
            };
            Ok(schema_dict(
                vec![("oneOf", CodecNode::Array(branches, loc))],
                loc,
            ))
        }
        CodecKind::Union(variants) => Ok(schema_dict(
            vec![(
                "anyOf",
                CodecNode::Array(
                    variants
                        .iter()
                        .map(|variant| {
                            generate_json_schema_node(
                                variant,
                                properties,
                                data,
                                current,
                                background,
                                links,
                                definitions,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    loc,
                ),
            )],
            loc,
        )),
        CodecKind::Bytes | CodecKind::Opaque | CodecKind::Function => Err(CodecFailure::new(
            format!(
                "Type {} has no JSON Schema mapping",
                codec_type_name(schema)
            ),
            data,
            schema.rule,
        )),
    }
}

fn generate_struct_schema_fields(
    plan: &StructPlan,
    codec_properties: &CodecProperties,
    data: Val,
    current: &Heap,
    background: &Heap,
    links: &mut HashMap<Handle, String>,
    definitions: &mut BTreeMap<String, CodecNode>,
) -> Result<(SchemaProperties, Vec<String>), CodecFailure> {
    let mut properties = Vec::new();
    let mut required = Vec::new();
    for field in &plan.fields {
        let external = field.external_name.clone();
        let property = generate_json_schema_node(
            &field.schema,
            codec_properties,
            data,
            current,
            background,
            links,
            definitions,
        )?;
        if option_item(&field.schema).is_none() {
            required.push(external.clone());
        }
        properties.push((external, property));
    }
    Ok((properties, required))
}

fn codec_type_name(schema: &CodecType) -> &'static str {
    match &schema.kind {
        CodecKind::TypeSlot(_) | CodecKind::TypeRef(_) => "recursive Type",
        CodecKind::Any => "Any",
        CodecKind::Type => "Type",
        CodecKind::Dyn => "Dyn",
        CodecKind::Int => "Int",
        CodecKind::Float => "Float",
        CodecKind::String => "String",
        CodecKind::Bytes => "Bytes",
        CodecKind::Opaque => "Opaque",
        CodecKind::Atom(_) => "Atom",
        CodecKind::Array(_) => "Array",
        CodecKind::Dict(_) => "Dict",
        CodecKind::Tagged { .. } => "Tagged",
        CodecKind::Tuple(_) => "Tuple",
        CodecKind::Struct(_) => "Struct",
        CodecKind::Enum(variants) => {
            let _ = variants;
            "Enum"
        }
        CodecKind::Union(_) => "Union",
        CodecKind::Function => "Func",
    }
}

fn codec_node_bytes(
    node: &CodecNode,
    current: &Heap,
    background: &Heap,
) -> Result<u64, NativeError> {
    match node {
        CodecNode::Existing(_) | CodecNode::Atom(_, _) => Ok(0),
        CodecNode::SemanticValue { raw, .. } => codec_node_bytes(raw, current, background)?
            .checked_add(semantic_codec_wrapper_bytes(raw, current, background)?)
            .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed")),
        CodecNode::Declared { payload, .. } => codec_node_bytes(payload, current, background),
        CodecNode::NamedAtom(value, _) | CodecNode::String(value, _) => Ok(value.len() as u64),
        CodecNode::Array(items, _) | CodecNode::Tuple(items, _) => {
            let own = logical_value_bytes(items.len())?;
            items.iter().try_fold(own, |total, item| {
                total
                    .checked_add(codec_node_bytes(item, current, background)?)
                    .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed"))
            })
        }
        CodecNode::Tagged { tag, payload, .. } => {
            let tag = codec_node_bytes(tag, current, background)?;
            let payload = codec_node_bytes(payload, current, background)?;
            logical_value_bytes(2)?
                .checked_add(tag)
                .and_then(|total| total.checked_add(payload))
                .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed"))
        }
        CodecNode::Dict(fields, _) => {
            let own = logical_value_bytes(fields.len())?;
            fields.iter().try_fold(own, |total, (name, value)| {
                let value_bytes = codec_node_bytes(value, current, background)?;
                total
                    .checked_add(name.len() as u64)
                    .and_then(|total| total.checked_add(value_bytes))
                    .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed"))
            })
        }
    }
}

fn semantic_codec_wrapper_bytes(
    node: &CodecNode,
    current: &Heap,
    background: &Heap,
) -> Result<u64, NativeError> {
    fn add(left: u64, right: u64) -> Result<u64, NativeError> {
        left.checked_add(right)
            .ok_or_else(|| NativeError::allocation_limit("semantic Value size overflowed"))
    }

    let tagged_bytes = logical_value_bytes(2)?;
    match node {
        CodecNode::Existing(value) => {
            semantic_value_wrapper_bytes(current, Some(background), *value)
                .map_err(|error| NativeError::new(error.to_string()))
        }
        CodecNode::Declared { payload, .. } => {
            semantic_codec_wrapper_bytes(payload, current, background)
        }
        CodecNode::Atom(BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False, _) => Ok(0),
        CodecNode::String(_, _) => Ok(tagged_bytes),
        CodecNode::Array(items, _) => {
            let mut bytes = add(logical_value_bytes(items.len())?, tagged_bytes)?;
            for item in items {
                bytes = add(
                    bytes,
                    semantic_codec_wrapper_bytes(item, current, background)?,
                )?;
            }
            Ok(bytes)
        }
        CodecNode::Dict(fields, _) => {
            let mut bytes = add(logical_value_bytes(fields.len())?, tagged_bytes)?;
            for (_, value) in fields {
                bytes = add(
                    bytes,
                    semantic_codec_wrapper_bytes(value, current, background)?,
                )?;
            }
            Ok(bytes)
        }
        CodecNode::Tagged { tag, payload, .. } => {
            let CodecNode::NamedAtom(tag, _) = tag.as_ref() else {
                return Err(NativeError::new("semantic temporal tag is not an Atom"));
            };
            if !matches!(
                tag.as_str(),
                "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
            ) || !matches!(
                payload.as_ref(),
                CodecNode::String(_, _) | CodecNode::Existing(_)
            ) {
                return Err(NativeError::new(
                    "raw data graph contains unsupported tagged value",
                ));
            }
            Ok(tagged_bytes)
        }
        CodecNode::SemanticValue { .. }
        | CodecNode::NamedAtom(_, _)
        | CodecNode::Tuple(_, _)
        | CodecNode::Atom(_, _) => Err(NativeError::new(
            "raw data graph contains an unsupported semantic Value",
        )),
    }
}

fn materialize_codec_node(node: CodecNode, current: &mut Heap, background: &Heap) -> Val {
    match node {
        CodecNode::Existing(value) => value,
        CodecNode::SemanticValue { owner, raw } => {
            let raw = materialize_codec_node(*raw, current, background);
            wrap_semantic_value(current, Some(background), raw, owner)
                .expect("codec Value owner and raw output were validated")
        }
        CodecNode::Declared {
            owner,
            payload,
            loc,
        } => {
            let payload = materialize_codec_node(*payload, current, background);
            let type_id = HeapView {
                current,
                background: Some(background),
            }
            .declared_type_id(owner)
            .expect("codec declared owner was decoded as a concrete declared Type");
            payload.with_type_id(type_id).with_loc(loc)
        }
        CodecNode::Atom(atom, loc) => Val::new(DecodedValue::BuiltinAtom(atom), loc),
        CodecNode::NamedAtom(value, loc) => Val::new(current.atom(Some(background), &value), loc),
        CodecNode::String(value, loc) => Val::new(current.string(Some(background), &value), loc),
        CodecNode::Array(items, loc) => {
            let items = items
                .into_iter()
                .map(|item| materialize_codec_node(item, current, background))
                .collect::<Box<_>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(items))),
                loc,
            )
        }
        CodecNode::Tuple(items, loc) => {
            let items = items
                .into_iter()
                .map(|item| materialize_codec_node(item, current, background))
                .collect::<Box<_>>();
            Val::new(
                DecodedValue::Tuple(current.allocate(Object::Tuple(items))),
                loc,
            )
        }
        CodecNode::Tagged { tag, payload, loc } => {
            let tag = materialize_codec_node(*tag, current, background);
            let payload = materialize_codec_node(*payload, current, background);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
        CodecNode::Dict(fields, loc) => {
            let mut fields = fields;
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            let (fields, values): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .map(|(name, value)| {
                    (
                        current.intern(&name),
                        materialize_codec_node(value, current, background),
                    )
                })
                .unzip();
            let shape = current.intern_shape(fields);
            Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: values.into(),
                })),
                loc,
            )
        }
    }
}
